//! # fs/exofs/syscall/object_fd/mod.rs — Table de descripteurs de fichiers ExoFS (no_std)
//!
//! Remplace `SpinLock<BTreeMap<>>` par un tableau plat `[ObjectFdSlot; MAX_FDS]`
//! protégé par un spinlock atomique (UnsafeCell + AtomicU64).
//!
//! RECUR-01 : zéro `for`, uniquement `while`.
//! OOM-02   : pas d'allocation en hot path (tableau statique).
//! ARITH-02 : saturating_*/wrapping_*.
//!
//! Capacité : 4 096 descripteurs simultanés (fd 4 … 4099).

pub mod entry;
pub mod flags;
pub mod table_inner;
#[cfg(test)]
mod tests;

pub use entry::{ObjectFdEntry, FD_MAX, FD_RESERVED, MAX_FDS};
pub use flags as open_flags;

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::scheduler::sync::spinlock::SpinLock;
use entry::{current_owner_pid, owner_matches};
use table_inner::ObjectFdTableInner;

/// Verrou de cycle de vie objet.
///
/// Il sérialise les chemins `open/create-delete` qui testent l'existence,
/// consultent la table FD, puis mutent le cache ou le catalogue disque.
pub static OBJECT_LIFECYCLE_LOCK: SpinLock<()> = SpinLock::new(());

/// Table de descripteurs ExoFS.
///
/// Thread-safety : UnsafeCell<ObjectFdTableInner> + AtomicU64 spinlock.
/// Pas de SpinLock<BTreeMap> (violation règle NO-STD + perf).
pub struct ObjectFdTable {
    inner: UnsafeCell<ObjectFdTableInner>,
    lock: AtomicU64,
    next_hint: AtomicU32, // Hint de départ de parcours (non bloquant).
    opens: AtomicU64,     // Compteur total d'ouvertures.
    closes: AtomicU64,    // Compteur total de fermetures.
    errors: AtomicU64,    // Erreurs (NoSpace, NotFound, …).
}

// SAFETY: `inner` n'est jamais accessible sans avoir pris `lock`; toutes les
// mutations du tableau de FD sont donc sérialisées.
unsafe impl Sync for ObjectFdTable {}
// SAFETY: la table n'expose que des copies d'entrées et peut être déplacée
// entre threads sous le même invariant de spinlock.
unsafe impl Send for ObjectFdTable {}

impl ObjectFdTable {
    pub const fn new_const() -> Self {
        Self {
            inner: UnsafeCell::new(ObjectFdTableInner::new()),
            lock: AtomicU64::new(0),
            next_hint: AtomicU32::new(FD_RESERVED),
            opens: AtomicU64::new(0),
            closes: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        }
    }

    // ── Spinlock ─────────────────────────────────────────────────────────────

    fn acquire(&self) {
        while self
            .lock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
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
        blob_id: BlobId,
        flags: u32,
        size: u64,
        epoch_id: u64,
        owner_uid: u64,
    ) -> ExofsResult<u32> {
        if !open_flags::validate(flags) {
            self.errors.fetch_add(1, Ordering::Relaxed);
            return Err(ExofsError::InvalidArgument);
        }
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let r = unsafe { &mut *self.inner.get() }.open_slot(
            blob_id,
            flags,
            size,
            epoch_id,
            owner_uid,
            self.next_hint.load(Ordering::Relaxed),
        );
        self.release();
        match &r {
            Ok(_) => {
                self.opens.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        r
    }

    /// Ferme un fd. Retourne true si le fd existait.
    pub fn close(&self, fd: u32) -> bool {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
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
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let r = unsafe { &*self.inner.get() }.get_entry(fd);
        self.release();
        match r {
            Ok(entry) => {
                if !owner_matches(entry, current_owner_pid()) {
                    self.errors.fetch_add(1, Ordering::Relaxed);
                    return Err(ExofsError::PermissionDenied);
                }
                Ok(entry)
            }
            Err(err) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                Err(err)
            }
        }
    }

    /// Retourne le BlobId d'un fd.
    pub fn blob_id_of(&self, fd: u32) -> ExofsResult<BlobId> {
        self.get(fd).map(|e| e.blob_id)
    }

    /// Met à jour le curseur d'un fd.
    pub fn set_cursor(&self, fd: u32, cursor: u64) -> ExofsResult<()> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let r = unsafe { &mut *self.inner.get() }.set_cursor(fd, cursor);
        self.release();
        r
    }

    /// Met à jour la taille connue d'un fd.
    pub fn set_size(&self, fd: u32, size: u64) -> ExofsResult<()> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let r = unsafe { &mut *self.inner.get() }.set_size(fd, size);
        self.release();
        r
    }

    /// Avance le curseur d'un fd de `n` octets.
    pub fn advance_cursor(&self, fd: u32, n: u64) -> ExofsResult<u64> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let inner = unsafe { &mut *self.inner.get() };
        let idx = inner.canonical_slot_of_fd(fd);
        match idx {
            Err(e) => {
                self.release();
                Err(e)
            }
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
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let owner = current_owner_pid();
        let inner = unsafe { &mut *self.inner.get() };
        let r = match inner.get_entry(fd) {
            Ok(entry) if owner_matches(entry, owner) => inner.dup_fd(fd),
            Ok(_) => Err(ExofsError::PermissionDenied),
            Err(err) => Err(err),
        };
        self.release();
        r
    }

    /// Duplique un fd dans le numéro demandé.
    pub fn dup2(&self, oldfd: u32, newfd: u32) -> ExofsResult<u32> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let owner = current_owner_pid();
        let inner = unsafe { &mut *self.inner.get() };
        let r = match inner.get_entry(oldfd) {
            Ok(entry) if owner_matches(entry, owner) => match inner.get_entry(newfd) {
                Ok(existing) if !owner_matches(existing, owner) => {
                    Err(ExofsError::PermissionDenied)
                }
                _ => inner.dup2_fd(oldfd, newfd),
            },
            Ok(_) => Err(ExofsError::PermissionDenied),
            Err(err) => Err(err),
        };
        self.release();
        r
    }

    /// Duplique un fd vers le premier numéro >= `min_fd`.
    pub fn dup_from(&self, fd: u32, min_fd: u32) -> ExofsResult<u32> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let owner = current_owner_pid();
        let inner = unsafe { &mut *self.inner.get() };
        let r = match inner.get_entry(fd) {
            Ok(entry) if owner_matches(entry, owner) => inner.dup_from_fd(fd, min_fd),
            Ok(_) => Err(ExofsError::PermissionDenied),
            Err(err) => Err(err),
        };
        self.release();
        r
    }

    /// Met à jour les flags d'état d'un descripteur.
    pub fn set_status_flags(&self, fd: u32, flags: u32) -> ExofsResult<u32> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let owner = current_owner_pid();
        let inner = unsafe { &mut *self.inner.get() };
        let r = match inner.get_entry(fd) {
            Ok(entry) if owner_matches(entry, owner) => inner.set_status_flags(fd, flags),
            Ok(_) => Err(ExofsError::PermissionDenied),
            Err(err) => Err(err),
        };
        self.release();
        r
    }

    /// Nombre de fds actuellement ouverts.
    pub fn open_count(&self) -> u32 {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
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
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
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
    pub fn lock(&self) -> Result<&Self, ExofsError> {
        Ok(self)
    }

    /// Nombre de fd ouverts pour un BlobId donné.
    pub fn open_count_for(&self, id: &BlobId) -> usize {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
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

    /// Nombre de descripteurs lisibles ouverts pour un BlobId donné.
    pub fn readable_count_for(&self, id: &BlobId) -> usize {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let inner = unsafe { &*self.inner.get() };
        let mut count = 0usize;
        let mut i = 0usize;
        while i < MAX_FDS {
            let entry = inner.slots[i];
            if !entry.is_free() && entry.blob_id == *id && entry.can_read() {
                count = count.saturating_add(1);
            }
            i = i.wrapping_add(1);
        }
        self.release();
        count
    }

    /// Retourne une entrée ouverte portant ce BlobId, si elle existe.
    pub fn entry_for_blob(&self, id: &BlobId) -> Option<ObjectFdEntry> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let inner = unsafe { &*self.inner.get() };
        let mut found = None;
        let mut i = 0usize;
        while i < MAX_FDS {
            let entry = inner.slots[i];
            if !entry.is_free() && entry.blob_id == *id {
                found = Some(entry);
                break;
            }
            i = i.wrapping_add(1);
        }
        self.release();
        found
    }
}

/// Singleton global
pub static OBJECT_TABLE: ObjectFdTable = ObjectFdTable::new_const();
