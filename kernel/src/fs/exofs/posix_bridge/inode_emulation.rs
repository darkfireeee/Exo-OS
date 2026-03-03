//! inode_emulation.rs — Émulation d'inodes POSIX pour ExoFS
//!
//! Fournit un mapping bidirectionnel stable `object_id ↔ ino_t` conforme POSIX.
//! Les numéros d'inodes sont alloués de façon monotone croissante.
//! L'ino 1 est réservé à la racine. Gestion d'un cache taille-limitée avec
//! éviction LRU-approché (round-robin).
//!
//! RECUR-01 / OOM-02 / ARITH-02 — ExofsError exclusivement.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use core::cell::UnsafeCell;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const INO_ROOT:      u64   = 1;
pub const INO_RESERVED:  u64   = 2;  // premier utilisable
pub const INO_MAX_CACHE: usize = 4096;
pub const INO_VER:       u8    = 1;
pub const INO_MAGIC:     u32   = 0x494E_4F45; // "INOE"

// ─────────────────────────────────────────────────────────────────────────────
// Types publics
// ─────────────────────────────────────────────────────────────────────────────

/// Numéro d'inode POSIX.
pub type ObjectIno = u64;

/// Flags d'attributs associés à un inode émulé.
pub mod inode_flags {
    pub const DIRECTORY: u32 = 0x0001;
    pub const SYMLINK:   u32 = 0x0002;
    pub const REGULAR:   u32 = 0x0004;
    pub const SNAPSHOT:  u32 = 0x0008;
    pub const READ_ONLY: u32 = 0x0010;
}

/// Entrée de la table inode.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct InodeEntry {
    pub ino:       ObjectIno,
    pub object_id: u64,
    pub flags:     u32,
    pub link_count:u32,
    pub size:      u64,
    pub uid:       u64,
    pub epoch_id:  u64,
    pub access_ts: u64,
}

const _: () = assert!(core::mem::size_of::<InodeEntry>() == 56);

// ─────────────────────────────────────────────────────────────────────────────
// Table principale — spinlock via AtomicU64 + UnsafeCell
// ─────────────────────────────────────────────────────────────────────────────

pub struct InodeEmulation {
    /// forward: (object_id, ino, entry) — tableau linéaire
    fwd:      UnsafeCell<Vec<InodeEntry>>,
    spinlock: AtomicU64,
    next_ino: AtomicU64,
    evict_cursor: AtomicU64,
}

unsafe impl Sync for InodeEmulation {}
unsafe impl Send for InodeEmulation {}

pub static INODE_EMULATION: InodeEmulation = InodeEmulation::new_const();

impl InodeEmulation {
    pub const fn new_const() -> Self {
        Self {
            fwd:          UnsafeCell::new(Vec::new()),
            spinlock:     AtomicU64::new(0),
            next_ino:     AtomicU64::new(INO_RESERVED),
            evict_cursor: AtomicU64::new(0),
        }
    }

    fn lock_acquire(&self) {
        while self.spinlock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }

    fn lock_release(&self) {
        self.spinlock.store(0, Ordering::Release);
    }

    // ─── Recherche interne par object_id (RECUR-01 : while) ───

    fn find_by_oid(table: &[InodeEntry], oid: u64) -> Option<usize> {
        let mut i = 0usize;
        while i < table.len() {
            if table[i].object_id == oid { return Some(i); }
            i = i.wrapping_add(1);
        }
        None
    }

    // ─── Recherche interne par ino (RECUR-01 : while) ───

    fn find_by_ino(table: &[InodeEntry], ino: ObjectIno) -> Option<usize> {
        let mut i = 0usize;
        while i < table.len() {
            if table[i].ino == ino { return Some(i); }
            i = i.wrapping_add(1);
        }
        None
    }

    /// Éviction round-robin si le cache est plein.
    fn evict_one(table: &mut Vec<InodeEntry>) {
        if table.is_empty() { return; }
        // Retire la première entrée (FIFO simple)
        table.remove(0);
    }

    // ─── API publique ───

    /// Retourne ou alloue un ino stable pour un object_id.
    /// OOM-02 : try_reserve. RECUR-01 : while.
    pub fn get_or_alloc(&self, object_id: u64) -> ExofsResult<ObjectIno> {
        self.get_or_alloc_flags(object_id, inode_flags::REGULAR, 0, 0)
    }

    /// Retourne ou alloue un ino avec métadonnées complètes.
    /// OOM-02 : try_reserve.
    pub fn get_or_alloc_flags(&self, object_id: u64, flags: u32, size: u64, uid: u64) -> ExofsResult<ObjectIno> {
        self.lock_acquire();
        let result = self.get_or_alloc_inner(object_id, flags, size, uid);
        self.lock_release();
        result
    }

    fn get_or_alloc_inner(&self, object_id: u64, flags: u32, size: u64, uid: u64) -> ExofsResult<ObjectIno> {
        let table = unsafe { &mut *self.fwd.get() };
        if let Some(idx) = Self::find_by_oid(table, object_id) {
            return Ok(table[idx].ino);
        }
        // Alloue un nouveau ino.
        if table.len() >= INO_MAX_CACHE { Self::evict_one(table); }
        let ino = self.next_ino.fetch_add(1, Ordering::Relaxed);
        let entry = InodeEntry { ino, object_id, flags, link_count: 1, size, uid, epoch_id: 0, access_ts: 0 };
        table.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        table.push(entry);
        Ok(ino)
    }

    /// Retourne le object_id correspondant à un ino.
    pub fn ino_to_object(&self, ino: ObjectIno) -> Option<u64> {
        self.lock_acquire();
        let table = unsafe { &*self.fwd.get() };
        let r = Self::find_by_ino(table, ino).map(|i| table[i].object_id);
        self.lock_release();
        r
    }

    /// Retourne l'entrée complète pour un ino.
    pub fn get_entry(&self, ino: ObjectIno) -> Option<InodeEntry> {
        self.lock_acquire();
        let table = unsafe { &*self.fwd.get() };
        let r = Self::find_by_ino(table, ino).map(|i| table[i]);
        self.lock_release();
        r
    }

    /// Retourne l'entrée complète pour un object_id.
    pub fn get_entry_by_oid(&self, oid: u64) -> Option<InodeEntry> {
        self.lock_acquire();
        let table = unsafe { &*self.fwd.get() };
        let r = Self::find_by_oid(table, oid).map(|i| table[i]);
        self.lock_release();
        r
    }

    /// Met à jour la taille d'un inode.
    pub fn update_size(&self, ino: ObjectIno, new_size: u64) -> ExofsResult<()> {
        self.lock_acquire();
        let table = unsafe { &mut *self.fwd.get() };
        let result = if let Some(idx) = Self::find_by_ino(table, ino) {
            table[idx].size = new_size;
            Ok(())
        } else {
            Err(ExofsError::ObjectNotFound)
        };
        self.lock_release();
        result
    }

    /// Met à jour le link_count d'un inode.
    /// ARITH-02 : saturating_add/sub.
    pub fn update_link_count(&self, ino: ObjectIno, delta: i64) -> ExofsResult<u32> {
        self.lock_acquire();
        let table = unsafe { &mut *self.fwd.get() };
        let result = if let Some(idx) = Self::find_by_ino(table, ino) {
            let cur = table[idx].link_count as i64;
            let new_lc = cur.saturating_add(delta).max(0) as u32;
            table[idx].link_count = new_lc;
            Ok(new_lc)
        } else {
            Err(ExofsError::ObjectNotFound)
        };
        self.lock_release();
        result
    }

    /// Met à jour les flags d'un inode.
    pub fn update_flags(&self, ino: ObjectIno, flags: u32) -> ExofsResult<()> {
        self.lock_acquire();
        let table = unsafe { &mut *self.fwd.get() };
        let result = if let Some(idx) = Self::find_by_ino(table, ino) {
            table[idx].flags = flags; Ok(())
        } else { Err(ExofsError::ObjectNotFound) };
        self.lock_release();
        result
    }

    /// Met à jour l'epoch_id d'un inode.
    pub fn update_epoch(&self, ino: ObjectIno, epoch_id: u64) -> ExofsResult<()> {
        self.lock_acquire();
        let table = unsafe { &mut *self.fwd.get() };
        let result = if let Some(idx) = Self::find_by_ino(table, ino) {
            table[idx].epoch_id = epoch_id; Ok(())
        } else { Err(ExofsError::ObjectNotFound) };
        self.lock_release();
        result
    }

    /// Invalide un inode (suppression du mapping).
    pub fn release(&self, object_id: u64) {
        self.lock_acquire();
        let table = unsafe { &mut *self.fwd.get() };
        if let Some(idx) = Self::find_by_oid(table, object_id) {
            table.remove(idx);
        }
        self.lock_release();
    }

    /// Invalide un inode par ino.
    pub fn release_ino(&self, ino: ObjectIno) {
        self.lock_acquire();
        let table = unsafe { &mut *self.fwd.get() };
        if let Some(idx) = Self::find_by_ino(table, ino) {
            table.remove(idx);
        }
        self.lock_release();
    }

    /// Vide toute la table (reset).
    pub fn clear(&self) {
        self.lock_acquire();
        let table = unsafe { &mut *self.fwd.get() };
        table.clear();
        self.lock_release();
    }

    /// Retourne le nombre d'inodes en cache.
    pub fn count(&self) -> usize {
        self.lock_acquire();
        let n = unsafe { (*self.fwd.get()).len() };
        self.lock_release();
        n
    }

    /// Retourne l'ino suivant qui serait alloué (sans l'allouer).
    pub fn peek_next_ino(&self) -> ObjectIno {
        self.next_ino.load(Ordering::Relaxed)
    }

    /// Retourne vrai si un ino est en cache.
    pub fn contains_ino(&self, ino: ObjectIno) -> bool {
        self.lock_acquire();
        let table = unsafe { &*self.fwd.get() };
        let r = Self::find_by_ino(table, ino).is_some();
        self.lock_release();
        r
    }

    /// Retourne vrai si un object_id est en cache.
    pub fn contains_oid(&self, oid: u64) -> bool {
        self.lock_acquire();
        let table = unsafe { &*self.fwd.get() };
        let r = Self::find_by_oid(table, oid).is_some();
        self.lock_release();
        r
    }

    /// Collecte tous les inos actuellement en cache.
    /// OOM-02 : try_reserve. RECUR-01 : while.
    pub fn all_inos(&self) -> ExofsResult<Vec<ObjectIno>> {
        self.lock_acquire();
        let table = unsafe { &*self.fwd.get() };
        let mut out: Vec<ObjectIno> = Vec::new();
        out.try_reserve(table.len()).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < table.len() { out.push(table[i].ino); i = i.wrapping_add(1); }
        self.lock_release();
        Ok(out)
    }

    /// Alias pour la racine.
    pub fn root_ino() -> ObjectIno { INO_ROOT }

    /// Garantit que la racine est bien enregistrée.
    pub fn ensure_root(&self) -> ExofsResult<()> {
        self.lock_acquire();
        let table = unsafe { &mut *self.fwd.get() };
        if Self::find_by_ino(table, INO_ROOT).is_none() {
            let entry = InodeEntry { ino: INO_ROOT, object_id: 1, flags: inode_flags::DIRECTORY, link_count: 2, size: 0, uid: 0, epoch_id: 0, access_ts: 0 };
            table.try_reserve(1).map_err(|_| { self.lock_release(); ExofsError::NoMemory })?;
            table.push(entry);
        }
        self.lock_release();
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sérialisation compacte d'un InodeEntry (pour cache blob)
// ─────────────────────────────────────────────────────────────────────────────

/// Encode un InodeEntry en 56 octets.
pub fn encode_inode_entry(e: &InodeEntry) -> [u8; 56] {
    let mut buf = [0u8; 56];
    let fields: [u64; 7] = [e.ino, e.object_id, (e.flags as u64) | ((e.link_count as u64) << 32), e.size, e.uid, e.epoch_id, e.access_ts];
    let mut i = 0usize;
    while i < 7 {
        let b = fields[i].to_le_bytes();
        let mut j = 0usize;
        while j < 8 { buf[i * 8 + j] = b[j]; j = j.wrapping_add(1); }
        i = i.wrapping_add(1);
    }
    buf
}

/// Décode un InodeEntry depuis 56 octets.
pub fn decode_inode_entry(buf: &[u8]) -> Option<InodeEntry> {
    if buf.len() < 56 { return None; }
    let mut fields = [0u64; 7];
    let mut i = 0usize;
    while i < 7 {
        fields[i] = u64::from_le_bytes([buf[i*8],buf[i*8+1],buf[i*8+2],buf[i*8+3],buf[i*8+4],buf[i*8+5],buf[i*8+6],buf[i*8+7]]);
        i = i.wrapping_add(1);
    }
    Some(InodeEntry {
        ino:        fields[0],
        object_id:  fields[1],
        flags:      (fields[2] & 0xFFFF_FFFF) as u32,
        link_count: (fields[2] >> 32) as u32,
        size:       fields[3],
        uid:        fields[4],
        epoch_id:   fields[5],
        access_ts:  fields[6],
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn emu() -> InodeEmulation { InodeEmulation::new_const() }

    #[test]
    fn test_inode_entry_size() { assert_eq!(core::mem::size_of::<InodeEntry>(), 56); }

    #[test]
    fn test_get_or_alloc_deterministic() {
        let e = emu();
        let ino1 = e.get_or_alloc(42).unwrap();
        let ino2 = e.get_or_alloc(42).unwrap();
        assert_eq!(ino1, ino2);
    }

    #[test]
    fn test_different_oids_different_inos() {
        let e = emu();
        let a = e.get_or_alloc(1).unwrap();
        let b = e.get_or_alloc(2).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn test_ino_to_object() {
        let e = emu();
        let ino = e.get_or_alloc(77).unwrap();
        assert_eq!(e.ino_to_object(ino), Some(77));
    }

    #[test]
    fn test_release() {
        let e = emu();
        let ino = e.get_or_alloc(55).unwrap();
        e.release(55);
        assert!(e.ino_to_object(ino).is_none());
    }

    #[test]
    fn test_update_size() {
        let e = emu();
        let ino = e.get_or_alloc(10).unwrap();
        e.update_size(ino, 1024).unwrap();
        assert_eq!(e.get_entry(ino).unwrap().size, 1024);
    }

    #[test]
    fn test_link_count_increment() {
        let e = emu();
        let ino = e.get_or_alloc(20).unwrap();
        let lc = e.update_link_count(ino, 2).unwrap();
        assert_eq!(lc, 3);
    }

    #[test]
    fn test_link_count_no_underflow() {
        let e = emu();
        let ino = e.get_or_alloc(30).unwrap();
        let lc = e.update_link_count(ino, -100).unwrap();
        assert_eq!(lc, 0);
    }

    #[test]
    fn test_ensure_root() {
        let e = emu();
        e.ensure_root().unwrap();
        assert!(e.contains_ino(INO_ROOT));
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let entry = InodeEntry { ino: 42, object_id: 99, flags: inode_flags::REGULAR, link_count: 3, size: 4096, uid: 1000, epoch_id: 7, access_ts: 12345 };
        let buf = encode_inode_entry(&entry);
        let decoded = decode_inode_entry(&buf).unwrap();
        assert_eq!(decoded.ino, 42);
        assert_eq!(decoded.object_id, 99);
        assert_eq!(decoded.size, 4096);
        assert_eq!(decoded.link_count, 3);
    }

    #[test]
    fn test_count() {
        let e = emu();
        e.get_or_alloc(1).unwrap();
        e.get_or_alloc(2).unwrap();
        e.get_or_alloc(3).unwrap();
        assert_eq!(e.count(), 3);
    }

    #[test]
    fn test_all_inos() {
        let e = emu();
        e.get_or_alloc(5).unwrap();
        e.get_or_alloc(6).unwrap();
        let inos = e.all_inos().unwrap();
        assert_eq!(inos.len(), 2);
    }
}
