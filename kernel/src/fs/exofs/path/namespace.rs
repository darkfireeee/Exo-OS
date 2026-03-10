//! namespace.rs — Table des espaces de nommage ExoFS
//!
//! Règles appliquées :
//!  - OOM-02   : try_reserve systématique
//!  - ONDISK-03: pas d'AtomicU64 dans les structs repr(C)
//!  - ARITH-02 : arithmétique vérifiée


extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, ObjectId};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum d'espaces de nommage simultanés.
pub const NAMESPACE_TABLE_MAX: usize = 16;

/// Taille maximale d'un nom de namespace.
pub const NAMESPACE_NAME_MAX: usize = 64;

/// Identifiant réservé de l'espace de nommage racine.
pub const ROOT_NAMESPACE_ID: u64 = 1;

/// Drapeaux de namespace.
pub const NS_FLAG_READONLY:  u32 = 0x0001;
pub const NS_FLAG_PRIVATE:   u32 = 0x0002;
pub const NS_FLAG_SHARED:    u32 = 0x0004;
pub const NS_FLAG_UNCLONEABLE: u32 = 0x0008;

// ─────────────────────────────────────────────────────────────────────────────
// Namespace
// ─────────────────────────────────────────────────────────────────────────────

/// Un espace de nommage : associe un ID unique à une racine de FS.
#[derive(Clone)]
pub struct Namespace {
    /// Identifiant unique (0 = slot vide).
    pub id:           u64,
    /// OID racine de ce namespace.
    pub root_oid:     ObjectId,
    /// Nom lisible (debug).
    pub name:         [u8; NAMESPACE_NAME_MAX],
    /// Longueur valide du nom.
    pub name_len:     u8,
    /// Drapeaux (NS_FLAG_*).
    pub flags:        u32,
    /// Tick de création.
    pub created_tick: u64,
}

impl Namespace {
    /// Crée un namespace validé.
    pub fn new(
        id:       u64,
        root_oid: ObjectId,
        name:     &[u8],
        flags:    u32,
    ) -> ExofsResult<Self> {
        if name.is_empty() || name.len() > NAMESPACE_NAME_MAX {
            return Err(ExofsError::InvalidArgument);
        }
        let mut name_buf = [0u8; NAMESPACE_NAME_MAX];
        name_buf[..name.len()].copy_from_slice(name);
        Ok(Namespace {
            id,
            root_oid,
            name:         name_buf,
            name_len:     name.len() as u8,
            flags,
            created_tick: crate::arch::time::read_ticks(),
        })
    }

    /// Nom du namespace.
    #[inline]
    pub fn name_str(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }

    /// `true` si aucun processus ne peut cloner ce namespace.
    #[inline]
    pub fn is_uncloneable(&self) -> bool {
        self.flags & NS_FLAG_UNCLONEABLE != 0
    }

    /// `true` si le namespace est en lecture seule.
    #[inline]
    pub fn is_readonly(&self) -> bool {
        self.flags & NS_FLAG_READONLY != 0
    }
}

impl Default for Namespace {
    fn default() -> Self {
        Namespace {
            id:           0,
            root_oid:     ObjectId([0u8; 32]),
            name:         [0u8; NAMESPACE_NAME_MAX],
            name_len:     0,
            flags:        0,
            created_tick: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NamespaceTableInner
// ─────────────────────────────────────────────────────────────────────────────

const EMPTY_NS: Namespace = Namespace {
    id:           0,
    root_oid:     ObjectId([0u8; 32]),
    name:         [0u8; NAMESPACE_NAME_MAX],
    name_len:     0,
    flags:        0,
    created_tick: 0,
};

struct NamespaceTableInner {
    entries:  [Namespace; NAMESPACE_TABLE_MAX],
    count:    usize,
    next_id:  u64,
}

impl NamespaceTableInner {
    const fn new() -> Self {
        NamespaceTableInner {
            entries: [EMPTY_NS; NAMESPACE_TABLE_MAX],
            count:   0,
            next_id: ROOT_NAMESPACE_ID,
        }
    }

    fn free_slot(&self) -> Option<usize> {
        for i in 0..NAMESPACE_TABLE_MAX {
            if self.entries[i].id == 0 { return Some(i); }
        }
        None
    }

    fn find_by_id(&self, id: u64) -> Option<usize> {
        for i in 0..NAMESPACE_TABLE_MAX {
            if self.entries[i].id == id { return Some(i); }
        }
        None
    }

    fn find_by_name(&self, name: &[u8]) -> Option<usize> {
        for i in 0..NAMESPACE_TABLE_MAX {
            let ns = &self.entries[i];
            if ns.id != 0 && ns.name_str() == name {
                return Some(i);
            }
        }
        None
    }

    fn insert(&mut self, mut ns: Namespace) -> ExofsResult<u64> {
        if self.count >= NAMESPACE_TABLE_MAX {
            return Err(ExofsError::NoSpace);
        }
        let slot = self.free_slot().ok_or(ExofsError::NoSpace)?;
        let id = self.next_id;
        self.next_id = self.next_id
            .checked_add(1)
            .ok_or(ExofsError::OffsetOverflow)?;
        ns.id = id;
        self.entries[slot] = ns;
        self.count = self.count
            .checked_add(1)
            .ok_or(ExofsError::OffsetOverflow)?;
        Ok(id)
    }

    fn remove_by_id(&mut self, id: u64) -> ExofsResult<()> {
        if id == ROOT_NAMESPACE_ID {
            return Err(ExofsError::PermissionDenied);
        }
        let slot = self.find_by_id(id).ok_or(ExofsError::ObjectNotFound)?;
        self.entries[slot] = EMPTY_NS;
        self.count = self.count.saturating_sub(1);
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NamespaceTable (publique, thread-safe)
// ─────────────────────────────────────────────────────────────────────────────

/// Table globale des espaces de nommage, protégée par SpinLock.
pub struct NamespaceTable {
    inner: SpinLock<NamespaceTableInner>,
}

impl NamespaceTable {
    /// Constructeur `const` pour initialisation statique.
    pub const fn new_const() -> Self {
        NamespaceTable {
            inner: SpinLock::new(NamespaceTableInner::new()),
        }
    }

    /// Initialise le namespace racine (à appeler une seule fois au boot).
    pub fn init_root(&self, root_oid: ObjectId) -> ExofsResult<()> {
        let mut guard = self.inner.lock();
        // Chercher si la racine existe déjà
        if guard.find_by_id(ROOT_NAMESPACE_ID).is_some() {
            return Ok(());
        }
        let ns = Namespace::new(ROOT_NAMESPACE_ID, root_oid, b"root", 0)?;
        let slot = guard.free_slot().ok_or(ExofsError::NoSpace)?;
        guard.entries[slot] = ns;
        guard.count = guard.count
            .checked_add(1)
            .ok_or(ExofsError::OffsetOverflow)?;
        // next_id doit être > ROOT_NAMESPACE_ID
        if guard.next_id <= ROOT_NAMESPACE_ID {
            guard.next_id = ROOT_NAMESPACE_ID
                .checked_add(1)
                .ok_or(ExofsError::OffsetOverflow)?;
        }
        Ok(())
    }

    /// Enregistre un nouveau namespace.
    pub fn register(
        &self,
        name:     &[u8],
        root_oid: ObjectId,
        flags:    u32,
    ) -> ExofsResult<u64> {
        let ns = Namespace::new(0, root_oid, name, flags)?;
        let mut guard = self.inner.lock();
        guard.insert(ns)
    }

    /// Supprime un namespace par son ID.  Le namespace racine ne peut pas
    /// être supprimé (retourne `PermissionDenied`).
    pub fn unregister(&self, id: u64) -> ExofsResult<()> {
        let mut guard = self.inner.lock();
        guard.remove_by_id(id)
    }

    /// Retourne l'OID racine d'un namespace.
    pub fn root_of(&self, id: u64) -> ExofsResult<ObjectId> {
        let guard = self.inner.lock();
        let i = guard.find_by_id(id).ok_or(ExofsError::ObjectNotFound)?;
        Ok(guard.entries[i].root_oid.clone())
    }

    /// Cherche un namespace par ID et retourne une copie.
    pub fn get(&self, id: u64) -> Option<Namespace> {
        let guard = self.inner.lock();
        guard.find_by_id(id).map(|i| guard.entries[i].clone())
    }

    /// Cherche un namespace par nom.
    pub fn find_by_name(&self, name: &[u8]) -> Option<Namespace> {
        let guard = self.inner.lock();
        guard.find_by_name(name).map(|i| guard.entries[i].clone())
    }

    /// Nombre de namespaces actifs.
    pub fn count(&self) -> usize {
        self.inner.lock().count
    }

    /// Itère sur tous les namespaces actifs.
    pub fn for_each<F>(&self, mut f: F)
    where
        F: FnMut(&Namespace),
    {
        let guard = self.inner.lock();
        for i in 0..NAMESPACE_TABLE_MAX {
            if guard.entries[i].id != 0 {
                f(&guard.entries[i]);
            }
        }
    }

    /// Vide la table (ne supprime pas la racine).
    pub fn flush_non_root(&self) {
        let mut guard = self.inner.lock();
        for i in 0..NAMESPACE_TABLE_MAX {
            if guard.entries[i].id != 0
                && guard.entries[i].id != ROOT_NAMESPACE_ID
            {
                guard.entries[i] = EMPTY_NS;
                guard.count = guard.count.saturating_sub(1);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Singleton global
// ─────────────────────────────────────────────────────────────────────────────

/// Table globale des espaces de nommage.
pub static NAMESPACE_TABLE: NamespaceTable = NamespaceTable::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions de commodité
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise le namespace racine.
pub fn init_namespaces(root_oid: ObjectId) -> ExofsResult<()> {
    NAMESPACE_TABLE.init_root(root_oid)
}

/// Enregistre un namespace dans la table globale.
#[inline]
pub fn register_namespace(name: &[u8], root_oid: ObjectId, flags: u32)
    -> ExofsResult<u64>
{
    NAMESPACE_TABLE.register(name, root_oid, flags)
}

/// Retourne l'OID racine d'un namespace.
#[inline]
pub fn root_of(ns_id: u64) -> ExofsResult<ObjectId> {
    NAMESPACE_TABLE.root_of(ns_id)
}

/// Cherche un namespace par son identifiant.
#[inline]
pub fn lookup_namespace_by_id(id: u64) -> Option<Namespace> {
    NAMESPACE_TABLE.get(id)
}

/// Vérifie qu'un namespace est accessible en écriture.
pub fn assert_writable(ns_id: u64) -> ExofsResult<()> {
    let ns = NAMESPACE_TABLE.get(ns_id).ok_or(ExofsError::ObjectNotFound)?;
    if ns.is_readonly() { Err(ExofsError::PermissionDenied) } else { Ok(()) }
}

/// Collecte tous les namespaces dans un Vec (OOM-02 : try_reserve).
pub fn all_namespaces() -> ExofsResult<Vec<Namespace>> {
    let mut v: Vec<Namespace> = Vec::new();
    NAMESPACE_TABLE.for_each(|ns| {
        // Ignorer OOM ici — best-effort dans un contexte d iter
        let _ = v.try_reserve(1).map(|_| v.push(ns.clone()));
    });
    Ok(v)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(b: u8) -> ObjectId { let mut a = [0u8; 32]; a[0] = b; ObjectId(a) }

    #[test] fn test_init_root() {
        let tbl = NamespaceTable::new_const();
        tbl.init_root(oid(1)).unwrap();
        let r = tbl.root_of(ROOT_NAMESPACE_ID).unwrap();
        assert_eq!(r.0[0], 1);
    }

    #[test] fn test_register_find() {
        let tbl = NamespaceTable::new_const();
        tbl.init_root(oid(1)).unwrap();
        let id = tbl.register(b"ns1", oid(10), 0).unwrap();
        let n = tbl.get(id).unwrap();
        assert_eq!(n.name_str(), b"ns1");
    }

    #[test] fn test_unregister_root_denied() {
        let tbl = NamespaceTable::new_const();
        tbl.init_root(oid(1)).unwrap();
        assert!(tbl.unregister(ROOT_NAMESPACE_ID).is_err());
    }

    #[test] fn test_by_name() {
        let tbl = NamespaceTable::new_const();
        tbl.init_root(oid(1)).unwrap();
        tbl.register(b"myns", oid(5), 0).unwrap();
        let n = tbl.find_by_name(b"myns").unwrap();
        assert_eq!(n.root_oid.0[0], 5);
    }

    #[test] fn test_count() {
        let tbl = NamespaceTable::new_const();
        tbl.init_root(oid(1)).unwrap();
        tbl.register(b"a", oid(2), 0).unwrap();
        tbl.register(b"b", oid(3), 0).unwrap();
        assert_eq!(tbl.count(), 3); // root + a + b
    }

    #[test] fn test_flush_non_root() {
        let tbl = NamespaceTable::new_const();
        tbl.init_root(oid(1)).unwrap();
        tbl.register(b"x", oid(20), 0).unwrap();
        tbl.flush_non_root();
        assert_eq!(tbl.count(), 1); // seule la racine reste
    }
}
