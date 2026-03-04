//! object_fd.rs — Table de descripteurs de fichiers ExoFS (no_std).
//!
//! Remplace `SpinLock<BTreeMap<>>` par un tableau plat `[ObjectFdSlot; MAX_FDS]`
//! protégé par un spinlock maison (UnsafeCell + AtomicU64).
//!
//! RECUR-01 : zéro `for`, uniquement `while`.
//! OOM-02   : pas d'allocation en hot path (tableau statique).
//! ARITH-02 : saturating_*/wrapping_*.
//!
//! Capacité : 65 532 descripteurs simultanés (fd 4 … 65535).

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Fd réservés : 0 (stdin), 1 (stdout), 2 (stderr), 3 (exofs-ctrl).
pub const FD_RESERVED: u32  = 4;
/// Fd maximum inclusif.
pub const FD_MAX:      u32  = 65_535;
/// Nombre de slots (fd 4…65535 → index 0…65531).
pub const MAX_FDS:     usize = (FD_MAX - FD_RESERVED + 1) as usize;
/// Marqueur de slot libre.
const SLOT_FREE: u32 = u32::MAX;

// ─────────────────────────────────────────────────────────────────────────────
// Flags d'ouverture
// ─────────────────────────────────────────────────────────────────────────────

pub mod open_flags {
    pub const O_RDONLY: u32 = 0x0000;
    pub const O_WRONLY: u32 = 0x0001;
    pub const O_RDWR:   u32 = 0x0002;
    pub const O_CREAT:  u32 = 0x0040;
    pub const O_EXCL:   u32 = 0x0080;
    pub const O_TRUNC:  u32 = 0x0200;
    pub const O_APPEND: u32 = 0x0400;

    /// Retourne true si les flags autorisent la lecture.
    #[inline]
    pub fn can_read(flags: u32) -> bool {
        let rw = flags & 0x0003;
        rw == O_RDONLY || rw == O_RDWR
    }

    /// Retourne true si les flags autorisent l'écriture.
    #[inline]
    pub fn can_write(flags: u32) -> bool {
        let rw = flags & 0x0003;
        rw == O_WRONLY || rw == O_RDWR
    }

    /// Valide les flags d'ouverture (seuls les bits connus sont acceptés).
    #[inline]
    pub fn validate(flags: u32) -> bool {
        flags & !0x07FF == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ObjectFd — descripteur d'objet ouvert
// ─────────────────────────────────────────────────────────────────────────────

/// Un descripteur de fichier ExoFS ouvert.
#[derive(Clone, Copy)]
pub struct ObjectFdEntry {
    /// Numéro de fd (= index + FD_RESERVED).
    pub fd:        u32,
    /// BlobId du blob associé.
    pub blob_id:   BlobId,
    /// Flags d'ouverture (O_RDONLY, O_WRONLY, O_RDWR, …).
    pub flags:     u32,
    /// Curseur de lecture/écriture courant.
    pub cursor:    u64,
    /// Taille connue du blob en octets (0 = inconnue).
    pub size:      u64,
    /// Compteur de références (fork/dup).
    pub ref_count: u32,
    /// Epoch au moment de l'ouverture.
    pub epoch_id:  u64,
    /// Uid de l'appelant (pour vérification de droits ultérieure).
    pub owner_uid: u64,
}

impl ObjectFdEntry {
    const fn empty() -> Self {
        Self {
            fd:        SLOT_FREE,
            blob_id:   BlobId([0u8; 32]),
            flags:     0,
            cursor:    0,
            size:      0,
            ref_count: 0,
            epoch_id:  0,
            owner_uid: 0,
        }
    }

    #[inline]
    pub fn is_free(&self) -> bool { self.fd == SLOT_FREE }

    #[inline]
    pub fn can_read(&self) -> bool { open_flags::can_read(self.flags) }

    #[inline]
    pub fn can_write(&self) -> bool { open_flags::can_write(self.flags) }

    /// Avance le curseur de `n` octets (saturating).
    #[inline]
    pub fn advance_cursor(&mut self, n: u64) {
        self.cursor = self.cursor.saturating_add(n);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ObjectFdTableInner — données internes protégées par spinlock
// ─────────────────────────────────────────────────────────────────────────────

struct ObjectFdTableInner {
    /// Tableau plat de slots (remplace BTreeMap).
    slots: [ObjectFdEntry; MAX_FDS],
    /// Nombre de fds ouverts.
    open_count: u32,
}

impl ObjectFdTableInner {
    const fn new() -> Self {
        Self {
            slots:      [const { ObjectFdEntry::empty() }; MAX_FDS],
            open_count: 0,
        }
    }

    /// Trouve un slot libre (RECUR-01 : while).
    fn find_free_slot(&self) -> Option<usize> {
        let mut i = 0usize;
        while i < MAX_FDS {
            if self.slots[i].is_free() { return Some(i); }
            i = i.wrapping_add(1);
        }
        None
    }

    /// Trouve le slot correspondant à un fd (RECUR-01 : while).
    fn slot_of_fd(&self, fd: u32) -> Option<usize> {
        if fd < FD_RESERVED || fd > FD_MAX { return None; }
        let idx = (fd - FD_RESERVED) as usize;
        if idx < MAX_FDS && !self.slots[idx].is_free() { Some(idx) } else { None }
    }

    /// Ouvre un nouveau fd→BlobId.
    fn open_slot(
        &mut self,
        blob_id:   BlobId,
        flags:     u32,
        size:      u64,
        epoch_id:  u64,
        owner_uid: u64,
        next_fd:   u32,
    ) -> ExofsResult<u32> {
        let idx = self.find_free_slot().ok_or(ExofsError::NoSpace)?;
        let fd = FD_RESERVED.saturating_add(idx as u32);
        self.slots[idx] = ObjectFdEntry {
            fd,
            blob_id,
            flags,
            cursor:    0,
            size,
            ref_count: 1,
            epoch_id,
            owner_uid,
        };
        self.open_count = self.open_count.saturating_add(1);
        Ok(fd)
    }

    /// Ferme un fd (retourne true si trouvé et fermé).
    fn close_slot(&mut self, fd: u32) -> bool {
        if let Some(idx) = self.slot_of_fd(fd) {
            let rc = self.slots[idx].ref_count;
            if rc <= 1 {
                self.slots[idx] = ObjectFdEntry::empty();
                self.open_count = self.open_count.saturating_sub(1);
            } else {
                self.slots[idx].ref_count = rc.wrapping_sub(1);
            }
            true
        } else {
            false
        }
    }

    /// Lit l'entrée d'un fd (clonée).
    fn get_entry(&self, fd: u32) -> ExofsResult<ObjectFdEntry> {
        let idx = self.slot_of_fd(fd).ok_or(ExofsError::ObjectNotFound)?;
        Ok(self.slots[idx])
    }

    /// Met à jour le curseur d'un fd.
    fn set_cursor(&mut self, fd: u32, cursor: u64) -> ExofsResult<()> {
        let idx = self.slot_of_fd(fd).ok_or(ExofsError::ObjectNotFound)?;
        self.slots[idx].cursor = cursor;
        Ok(())
    }

    /// Met à jour la taille connue d'un fd.
    fn set_size(&mut self, fd: u32, size: u64) -> ExofsResult<()> {
        let idx = self.slot_of_fd(fd).ok_or(ExofsError::ObjectNotFound)?;
        self.slots[idx].size = size;
        Ok(())
    }

    /// Duplique un fd (incrémente ref_count).
    fn dup_fd(&mut self, fd: u32) -> ExofsResult<u32> {
        let idx = self.slot_of_fd(fd).ok_or(ExofsError::ObjectNotFound)?;
        let new_rc = self.slots[idx].ref_count.saturating_add(1);
        self.slots[idx].ref_count = new_rc;
        Ok(fd) // Même fd (sémantique simplifiée ; un dup complet créerait un nouveau slot).
    }

    /// Retourne le nombre de fds ouverts.
    #[inline]
    fn open_count(&self) -> u32 { self.open_count }
}

// ─────────────────────────────────────────────────────────────────────────────
// ObjectFdTable — façade thread-safe avec spinlock maison
// ─────────────────────────────────────────────────────────────────────────────

/// Table de descripteurs ExoFS.
///
/// Thread-safety : UnsafeCell<ObjectFdTableInner> + AtomicU64 spinlock.
/// Pas de SpinLock<BTreeMap> (violation règle NO-STD + pérf).
pub struct ObjectFdTable {
    inner:    UnsafeCell<ObjectFdTableInner>,
    lock:     AtomicU64,
    next_hint:AtomicU32,   // Hint de départ de parcours (non bloquant).
    opens:    AtomicU64,   // Compteur total d'ouvertures.
    closes:   AtomicU64,   // Compteur total de fermetures.
    errors:   AtomicU64,   // Erreurs (NoSpace, NotFound, …).
}

unsafe impl Sync for ObjectFdTable {}
unsafe impl Send for ObjectFdTable {}

impl ObjectFdTable {
    pub const fn new_const() -> Self {
        Self {
            inner:     UnsafeCell::new(ObjectFdTableInner::new()),
            lock:      AtomicU64::new(0),
            next_hint: AtomicU32::new(FD_RESERVED),
            opens:     AtomicU64::new(0),
            closes:    AtomicU64::new(0),
            errors:    AtomicU64::new(0),
        }
    }

    // ── Spinlock ─────────────────────────────────────────────────────────────

    fn acquire(&self) {
        while self.lock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }

    fn release(&self) {
        self.lock.store(0, Ordering::Release);
    }

    // ── API publique ──────────────────────────────────────────────────────────

    /// Ouvre un fd pour le BlobId donné.
    pub fn open(
        &self,
        blob_id:   BlobId,
        flags:     u32,
        size:      u64,
        epoch_id:  u64,
        owner_uid: u64,
    ) -> ExofsResult<u32> {
        if !open_flags::validate(flags) {
            self.errors.fetch_add(1, Ordering::Relaxed);
            return Err(ExofsError::InvalidArgument);
        }
        self.acquire();
        let r = unsafe { &mut *self.inner.get() }
            .open_slot(blob_id, flags, size, epoch_id, owner_uid, self.next_hint.load(Ordering::Relaxed));
        self.release();
        match &r {
            Ok(_)  => { self.opens.fetch_add(1, Ordering::Relaxed); }
            Err(_) => { self.errors.fetch_add(1, Ordering::Relaxed); }
        }
        r
    }

    /// Ferme un fd. Retourne true si le fd existait.
    pub fn close(&self, fd: u32) -> bool {
        self.acquire();
        let found = unsafe { &mut *self.inner.get() }.close_slot(fd);
        self.release();
        if found {
            self.closes.fetch_add(1, Ordering::Relaxed);
        } else {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
        found
    }

    /// Lit l'entrée d'un fd (snapshot copié).
    pub fn get(&self, fd: u32) -> ExofsResult<ObjectFdEntry> {
        self.acquire();
        let r = unsafe { &*self.inner.get() }.get_entry(fd);
        self.release();
        if r.is_err() {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
        r
    }

    /// Retourne le BlobId d'un fd.
    pub fn blob_id_of(&self, fd: u32) -> ExofsResult<BlobId> {
        self.get(fd).map(|e| e.blob_id)
    }

    /// Met à jour le curseur d'un fd.
    pub fn set_cursor(&self, fd: u32, cursor: u64) -> ExofsResult<()> {
        self.acquire();
        let r = unsafe { &mut *self.inner.get() }.set_cursor(fd, cursor);
        self.release();
        r
    }

    /// Met à jour la taille connue d'un fd.
    pub fn set_size(&self, fd: u32, size: u64) -> ExofsResult<()> {
        self.acquire();
        let r = unsafe { &mut *self.inner.get() }.set_size(fd, size);
        self.release();
        r
    }

    /// Avance le curseur d'un fd de `n` octets.
    pub fn advance_cursor(&self, fd: u32, n: u64) -> ExofsResult<u64> {
        self.acquire();
        let inner = unsafe { &mut *self.inner.get() };
        let idx = inner.slot_of_fd(fd).ok_or_else(|| {
            self.release();
            ExofsError::ObjectNotFound
        });
        match idx {
            Err(e) => { self.release(); Err(e) }
            Ok(i) => {
                inner.slots[i].advance_cursor(n);
                let new_cursor = inner.slots[i].cursor;
                self.release();
                Ok(new_cursor)
            }
        }
    }

    /// Duplique un fd.
    pub fn dup(&self, fd: u32) -> ExofsResult<u32> {
        self.acquire();
        let r = unsafe { &mut *self.inner.get() }.dup_fd(fd);
        self.release();
        r
    }

    /// Nombre de fds actuellement ouverts.
    pub fn open_count(&self) -> u32 {
        self.acquire();
        let n = unsafe { &*self.inner.get() }.open_count();
        self.release();
        n
    }

    /// Statistiques (opens, closes, errors).
    pub fn stats(&self) -> (u64, u64, u64) {
        (
            self.opens.load(Ordering::Relaxed),
            self.closes.load(Ordering::Relaxed),
            self.errors.load(Ordering::Relaxed),
        )
    }

    /// Vérifie qu'un fd est ouvert et que les flags permettent la lecture.
    pub fn check_readable(&self, fd: u32) -> ExofsResult<()> {
        let entry = self.get(fd)?;
        if !entry.can_read() {
            return Err(ExofsError::PermissionDenied);
        }
        Ok(())
    }

    /// Vérifie qu'un fd est ouvert et que les flags permettent l'écriture.
    pub fn check_writable(&self, fd: u32) -> ExofsResult<()> {
        let entry = self.get(fd)?;
        if !entry.can_write() {
            return Err(ExofsError::PermissionDenied);
        }
        Ok(())
    }

    /// Remet toute la table à zéro (usage en tests ou shutdown propre).
    pub fn reset_all(&self) {
        self.acquire();
        let inner = unsafe { &mut *self.inner.get() };
        let mut i = 0usize;
        while i < MAX_FDS {
            inner.slots[i] = ObjectFdEntry::empty();
            i = i.wrapping_add(1);
        }
        inner.open_count = 0;
        self.release();
        self.opens.store(0, Ordering::Relaxed);
        self.closes.store(0, Ordering::Relaxed);
        self.errors.store(0, Ordering::Relaxed);
    }

    /// Pseudo-verrou : retourne `Ok(&self)` (ObjectFdTable est déjà thread-safe via UnsafeCell+acquire).
    #[inline]
    pub fn lock(&self) -> Result<&Self, ExofsError> { Ok(self) }

    /// Nombre de fd ouverts pour un BlobId donné.
    pub fn open_count_for(&self, id: &BlobId) -> usize {
        self.acquire();
        let inner = unsafe { &*self.inner.get() };
        let mut count = 0usize;
        let mut i = 0usize;
        while i < MAX_FDS {
            if !inner.slots[i].is_free() && inner.slots[i].blob_id == *id {
                count = count.saturating_add(1);
            }
            i = i.wrapping_add(1);
        }
        self.release();
        count
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Singleton global
// ─────────────────────────────────────────────────────────────────────────────

pub static OBJECT_TABLE: ObjectFdTable = ObjectFdTable::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_blob(b: u8) -> BlobId {
        BlobId([b; 32])
    }

    #[test]
    fn test_open_close_basic() {
        let t = ObjectFdTable::new_const();
        let fd = t.open(make_blob(1), open_flags::O_RDWR, 0, 0, 0).unwrap();
        assert!(fd >= FD_RESERVED);
        assert!(t.close(fd));
    }

    #[test]
    fn test_open_not_found_after_close() {
        let t = ObjectFdTable::new_const();
        let fd = t.open(make_blob(2), open_flags::O_RDONLY, 0, 0, 0).unwrap();
        t.close(fd);
        assert!(t.get(fd).is_err());
    }

    #[test]
    fn test_blob_id_of() {
        let t = ObjectFdTable::new_const();
        let blob = make_blob(3);
        let fd = t.open(blob, open_flags::O_RDONLY, 0, 0, 0).unwrap();
        assert_eq!(t.blob_id_of(fd).unwrap().0, blob.0);
        t.close(fd);
    }

    #[test]
    fn test_open_count() {
        let t = ObjectFdTable::new_const();
        assert_eq!(t.open_count(), 0);
        let fd1 = t.open(make_blob(4), open_flags::O_RDWR, 0, 0, 0).unwrap();
        let fd2 = t.open(make_blob(5), open_flags::O_RDWR, 0, 0, 0).unwrap();
        assert_eq!(t.open_count(), 2);
        t.close(fd1);
        assert_eq!(t.open_count(), 1);
        t.close(fd2);
        assert_eq!(t.open_count(), 0);
    }

    #[test]
    fn test_set_cursor() {
        let t = ObjectFdTable::new_const();
        let fd = t.open(make_blob(6), open_flags::O_RDWR, 1024, 1, 0).unwrap();
        t.set_cursor(fd, 512).unwrap();
        assert_eq!(t.get(fd).unwrap().cursor, 512);
        t.close(fd);
    }

    #[test]
    fn test_advance_cursor() {
        let t = ObjectFdTable::new_const();
        let fd = t.open(make_blob(7), open_flags::O_RDWR, 4096, 1, 0).unwrap();
        t.advance_cursor(fd, 100).unwrap();
        assert_eq!(t.get(fd).unwrap().cursor, 100);
        t.advance_cursor(fd, 200).unwrap();
        assert_eq!(t.get(fd).unwrap().cursor, 300);
        t.close(fd);
    }

    #[test]
    fn test_check_readable_rdonly() {
        let t = ObjectFdTable::new_const();
        let fd = t.open(make_blob(8), open_flags::O_RDONLY, 0, 0, 0).unwrap();
        assert!(t.check_readable(fd).is_ok());
        assert!(t.check_writable(fd).is_err());
        t.close(fd);
    }

    #[test]
    fn test_check_writable_wronly() {
        let t = ObjectFdTable::new_const();
        let fd = t.open(make_blob(9), open_flags::O_WRONLY, 0, 0, 0).unwrap();
        assert!(t.check_writable(fd).is_ok());
        assert!(t.check_readable(fd).is_err());
        t.close(fd);
    }

    #[test]
    fn test_invalid_flags_rejected() {
        let t = ObjectFdTable::new_const();
        let r = t.open(make_blob(10), 0xDEAD_BEEF, 0, 0, 0);
        assert!(r.is_err());
    }

    #[test]
    fn test_close_nonexistent_returns_false() {
        let t = ObjectFdTable::new_const();
        assert!(!t.close(9999));
    }

    #[test]
    fn test_get_invalid_fd_returns_err() {
        let t = ObjectFdTable::new_const();
        assert!(t.get(0).is_err());
        assert!(t.get(3).is_err());
    }

    #[test]
    fn test_dup_increments_refcount() {
        let t = ObjectFdTable::new_const();
        let fd = t.open(make_blob(11), open_flags::O_RDWR, 0, 0, 0).unwrap();
        t.dup(fd).unwrap();
        // Après dup, une fermeture ne libère pas le slot.
        t.close(fd);
        assert!(t.get(fd).is_ok(), "fd still open after first close (ref=2)");
        t.close(fd);
        assert!(t.get(fd).is_err(), "fd closed after second close (ref=0)");
    }

    #[test]
    fn test_stats_tracking() {
        let t = ObjectFdTable::new_const();
        let fd = t.open(make_blob(12), open_flags::O_RDWR, 0, 0, 0).unwrap();
        t.close(fd);
        let (o, c, _) = t.stats();
        assert_eq!(o, 1);
        assert_eq!(c, 1);
    }

    #[test]
    fn test_reset_all() {
        let t = ObjectFdTable::new_const();
        let fd = t.open(make_blob(13), open_flags::O_RDWR, 0, 0, 0).unwrap();
        assert_eq!(t.open_count(), 1);
        t.reset_all();
        assert_eq!(t.open_count(), 0);
        assert!(t.get(fd).is_err());
    }

    #[test]
    fn test_set_size() {
        let t = ObjectFdTable::new_const();
        let fd = t.open(make_blob(14), open_flags::O_RDWR, 0, 0, 0).unwrap();
        t.set_size(fd, 8192).unwrap();
        assert_eq!(t.get(fd).unwrap().size, 8192);
        t.close(fd);
    }

    #[test]
    fn test_open_flags_can_read_write() {
        assert!(open_flags::can_read(open_flags::O_RDONLY));
        assert!(!open_flags::can_write(open_flags::O_RDONLY));
        assert!(open_flags::can_read(open_flags::O_RDWR));
        assert!(open_flags::can_write(open_flags::O_RDWR));
        assert!(!open_flags::can_read(open_flags::O_WRONLY));
        assert!(open_flags::can_write(open_flags::O_WRONLY));
    }
}
