//! mount_point.rs — Table des points de montage ExoFS
//!
//! Règles appliquées :
//!  - OOM-02   : try_reserve systématique
//!  - ONDISK-03: pas d'AtomicU64 dans les structs repr(C)
//!  - ARITH-02 : arithmétique vérifiée


extern crate alloc;

use crate::fs::exofs::core::{ExofsError, ExofsResult, ObjectId};
use crate::fs::exofs::path::path_component::NAME_MAX;
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum de points de montage simultanés.
pub const MOUNT_TABLE_MAX: usize = 64;

/// Drapeaux de montage.
pub const MOUNT_FLAG_READONLY: u32  = 0x0001;
pub const MOUNT_FLAG_NOEXEC:   u32  = 0x0002;
pub const MOUNT_FLAG_NOSUID:   u32  = 0x0004;
pub const MOUNT_FLAG_BIND:     u32  = 0x0008;

// ─────────────────────────────────────────────────────────────────────────────
// MountPoint
// ─────────────────────────────────────────────────────────────────────────────

/// Un point de montage : répertoire hôte → système de fichiers monté.
#[derive(Clone)]
pub struct MountPoint {
    /// OID du répertoire sur lequel le montage s'effectue.
    pub dir_oid:      ObjectId,
    /// Nom du point de montage (souvent le dernier composant du chemin).
    pub name:         [u8; NAME_MAX + 1],
    /// Longueur valide du nom.
    pub name_len:     u16,
    /// OID racine du système de fichiers monté.
    pub mounted_oid:  ObjectId,
    /// Drapeaux du montage (MOUNT_FLAG_*).
    pub flags:        u32,
    /// Identifiant unique de ce montage.
    pub mount_id:     u32,
    /// Tick de création.
    pub created_tick: u64,
}

impl MountPoint {
    /// Constructeur.
    pub fn new(
        dir_oid:     ObjectId,
        name:        &[u8],
        mounted_oid: ObjectId,
        flags:       u32,
        mount_id:    u32,
    ) -> ExofsResult<Self> {
        if name.is_empty() || name.len() > NAME_MAX {
            return Err(ExofsError::InvalidPathComponent);
        }
        let mut name_buf = [0u8; NAME_MAX + 1];
        name_buf[..name.len()].copy_from_slice(name);
        Ok(MountPoint {
            dir_oid,
            name:         name_buf,
            name_len:     name.len() as u16,
            mounted_oid,
            flags,
            mount_id,
            created_tick: crate::arch::time::read_ticks(),
        })
    }

    /// Nom du point de montage.
    #[inline]
    pub fn name_bytes(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }

    /// `true` si ce point de montage est en lecture seule.
    #[inline]
    pub fn is_readonly(&self) -> bool {
        self.flags & MOUNT_FLAG_READONLY != 0
    }

    /// `true` si les exécutables sont interdits.
    #[inline]
    pub fn is_noexec(&self) -> bool {
        self.flags & MOUNT_FLAG_NOEXEC != 0
    }

    /// `true` si setuid est interdit.
    #[inline]
    pub fn is_nosuid(&self) -> bool {
        self.flags & MOUNT_FLAG_NOSUID != 0
    }
}

// `Default` pour initialisation de tableau `const`.
impl Default for MountPoint {
    fn default() -> Self {
        MountPoint {
            dir_oid:      ObjectId([0u8; 32]),
            name:         [0u8; NAME_MAX + 1],
            name_len:     0,
            mounted_oid:  ObjectId([0u8; 32]),
            flags:        0,
            mount_id:     0,
            created_tick: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MountTableInner
// ─────────────────────────────────────────────────────────────────────────────

const EMPTY_MOUNT: MountPoint = MountPoint {
    dir_oid:      ObjectId([0u8; 32]),
    name:         [0u8; NAME_MAX + 1],
    name_len:     0,
    mounted_oid:  ObjectId([0u8; 32]),
    flags:        0,
    mount_id:     0,
    created_tick: 0,
};

struct MountTableInner {
    entries:  [MountPoint; MOUNT_TABLE_MAX],
    count:    usize,
    next_id:  u32,
}

impl MountTableInner {
    const fn new() -> Self {
        // On ne peut pas encore appeler `Default::default()` en const fn
        // donc on initialise manuellement.
        MountTableInner {
            entries:  [EMPTY_MOUNT; MOUNT_TABLE_MAX],
            count:    0,
            next_id:  1,
        }
    }

    /// Cherche un slot libre (name_len == 0).
    fn free_slot(&self) -> Option<usize> {
        for i in 0..MOUNT_TABLE_MAX {
            if self.entries[i].name_len == 0 {
                return Some(i);
            }
        }
        None
    }

    /// Trouve le point de montage dont le dir_oid correspond.
    fn find_by_dir(&self, oid: &ObjectId) -> Option<usize> {
        for i in 0..MOUNT_TABLE_MAX {
            if self.entries[i].name_len != 0
                && self.entries[i].dir_oid.0 == oid.0
            {
                return Some(i);
            }
        }
        None
    }

    /// Trouve par mount_id.
    fn find_by_id(&self, id: u32) -> Option<usize> {
        for i in 0..MOUNT_TABLE_MAX {
            if self.entries[i].name_len != 0
                && self.entries[i].mount_id == id
            {
                return Some(i);
            }
        }
        None
    }

    fn insert(&mut self, mp: MountPoint) -> ExofsResult<u32> {
        if self.count >= MOUNT_TABLE_MAX {
            return Err(ExofsError::NoSpace);
        }
        let slot = self.free_slot().ok_or(ExofsError::NoSpace)?;
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
        let mut mp = mp;
        mp.mount_id = id;
        self.entries[slot] = mp;
        self.count = self.count.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
        Ok(id)
    }

    fn remove_by_dir(&mut self, oid: &ObjectId) -> ExofsResult<()> {
        let slot = self.find_by_dir(oid).ok_or(ExofsError::ObjectNotFound)?;
        self.entries[slot] = EMPTY_MOUNT;
        self.count = self.count.saturating_sub(1);
        Ok(())
    }

    fn remove_by_id(&mut self, id: u32) -> ExofsResult<()> {
        let slot = self.find_by_id(id).ok_or(ExofsError::ObjectNotFound)?;
        self.entries[slot] = EMPTY_MOUNT;
        self.count = self.count.saturating_sub(1);
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MountTable (publique, thread-safe)
// ─────────────────────────────────────────────────────────────────────────────

/// Table des points de montage globale, protégée par SpinLock.
pub struct MountTable {
    inner: SpinLock<MountTableInner>,
}

impl MountTable {
    /// Constructeur `const` pour initialisation statique.
    pub const fn new_const() -> Self {
        MountTable {
            inner: SpinLock::new(MountTableInner::new()),
        }
    }

    /// Enregistre un nouveau point de montage.
    ///
    /// Retourne le `mount_id` attribué.
    pub fn register(
        &self,
        dir_oid:     ObjectId,
        name:        &[u8],
        mounted_oid: ObjectId,
        flags:       u32,
    ) -> ExofsResult<u32> {
        let mp = MountPoint::new(dir_oid, name, mounted_oid, flags, 0)?;
        let mut guard = self.inner.lock();
        guard.insert(mp)
    }

    /// Supprime le point de montage sur `dir_oid`.
    pub fn unregister_by_dir(&self, dir_oid: &ObjectId) -> ExofsResult<()> {
        let mut guard = self.inner.lock();
        guard.remove_by_dir(dir_oid)
    }

    /// Supprime le point de montage par son identifiant.
    pub fn unregister_by_id(&self, mount_id: u32) -> ExofsResult<()> {
        let mut guard = self.inner.lock();
        guard.remove_by_id(mount_id)
    }

    /// Cherche un point de montage couvrant `dir_oid`.
    ///
    /// Retourne l'OID monté si trouvé.
    pub fn lookup_mount(&self, dir_oid: &ObjectId) -> Option<ObjectId> {
        let guard = self.inner.lock();
        guard.find_by_dir(dir_oid)
            .map(|i| guard.entries[i].mounted_oid.clone())
    }

    /// Retourne une copie clonée de l'entrée pour `dir_oid`.
    pub fn get_entry(&self, dir_oid: &ObjectId) -> Option<MountPoint> {
        let guard = self.inner.lock();
        guard.find_by_dir(dir_oid)
            .map(|i| guard.entries[i].clone())
    }

    /// Nombre de montages actifs.
    pub fn count(&self) -> usize {
        self.inner.lock().count
    }

    /// Vérifie si un OID est couvert par un montage (lecture seule).
    pub fn is_readonly(&self, dir_oid: &ObjectId) -> bool {
        let guard = self.inner.lock();
        match guard.find_by_dir(dir_oid) {
            None    => false,
            Some(i) => guard.entries[i].is_readonly(),
        }
    }

    /// Itère sur tous les points de montage actifs.
    ///
    /// Appelle `f(mp)` pour chaque entrée valide.
    pub fn for_each<F>(&self, mut f: F)
    where
        F: FnMut(&MountPoint),
    {
        let guard = self.inner.lock();
        for i in 0..MOUNT_TABLE_MAX {
            if guard.entries[i].name_len != 0 {
                f(&guard.entries[i]);
            }
        }
    }

    /// Remet la table à zéro (shutdown).
    pub fn flush(&self) {
        let mut guard = self.inner.lock();
        for i in 0..MOUNT_TABLE_MAX {
            guard.entries[i] = EMPTY_MOUNT;
        }
        guard.count = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Singleton global
// ─────────────────────────────────────────────────────────────────────────────

/// Table globale des points de montage.
pub static MOUNT_TABLE: MountTable = MountTable::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions de commodité
// ─────────────────────────────────────────────────────────────────────────────

/// Enregistre un point de montage dans la table globale.
#[inline]
pub fn register_mount(
    dir_oid:     ObjectId,
    name:        &[u8],
    mounted_oid: ObjectId,
    flags:       u32,
) -> ExofsResult<u32> {
    MOUNT_TABLE.register(dir_oid, name, mounted_oid, flags)
}

/// Supprime un montage par son répertoire.
#[inline]
pub fn unregister_mount_by_dir(dir_oid: &ObjectId) -> ExofsResult<()> {
    MOUNT_TABLE.unregister_by_dir(dir_oid)
}

/// Cherche si un répertoire est un point de montage.
#[inline]
pub fn lookup_mount(dir_oid: &ObjectId) -> Option<ObjectId> {
    MOUNT_TABLE.lookup_mount(dir_oid)
}

/// `true` si `dir_oid` est un point de montage (toutes flags confondues).
#[inline]
pub fn is_mount_point(dir_oid: &ObjectId) -> bool {
    MOUNT_TABLE.lookup_mount(dir_oid).is_some()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(b: u8) -> ObjectId { let mut a = [0u8; 32]; a[0] = b; ObjectId(a) }

    #[test] fn test_register_lookup() {
        let tbl = MountTable::new_const();
        tbl.register(oid(1), b"mnt", oid(99), 0).unwrap();
        let r = tbl.lookup_mount(&oid(1)).unwrap();
        assert_eq!(r.0[0], 99);
    }

    #[test] fn test_unregister() {
        let tbl = MountTable::new_const();
        tbl.register(oid(2), b"mnt2", oid(88), 0).unwrap();
        tbl.unregister_by_dir(&oid(2)).unwrap();
        assert!(tbl.lookup_mount(&oid(2)).is_none());
    }

    #[test] fn test_readonly_flag() {
        let tbl = MountTable::new_const();
        tbl.register(oid(3), b"ro", oid(77), MOUNT_FLAG_READONLY).unwrap();
        assert!(tbl.is_readonly(&oid(3)));
    }

    #[test] fn test_table_full() {
        let tbl = MountTable::new_const();
        for i in 0u8..64 {
            tbl.register(oid(i), b"x", oid(i.wrapping_add(100)), 0).unwrap();
        }
        let extra = tbl.register(oid(200), b"z", oid(201), 0);
        assert!(extra.is_err());
    }

    #[test] fn test_count() {
        let tbl = MountTable::new_const();
        tbl.register(oid(5), b"a", oid(50), 0).unwrap();
        tbl.register(oid(6), b"b", oid(60), 0).unwrap();
        assert_eq!(tbl.count(), 2);
    }

    #[test] fn test_flush() {
        let tbl = MountTable::new_const();
        tbl.register(oid(7), b"c", oid(70), 0).unwrap();
        tbl.flush();
        assert_eq!(tbl.count(), 0);
    }
}
