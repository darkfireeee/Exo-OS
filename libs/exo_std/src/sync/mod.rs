// libs/exo_std/src/sync/mod.rs
//! Primitives de synchronisation optimisées et robustes
//!
//! Ce module fournit des primitives de synchronisation thread-safe pour
//! la coordination entre threads. Toutes les implémentations utilisent
//! des optimisations avancées (backoff exponentiel, fast-paths, etc.)

pub mod mutex;
pub mod rwlock;
pub mod condvar;
pub mod barrier;
pub mod once;
pub mod atomic;
pub mod semaphore;

pub use mutex::{Mutex, MutexGuard};
pub use rwlock::{RwLock, RwLockReadGuard, RwLockWriteGuard};
pub use condvar::Condvar;
pub use barrier::Barrier;
pub use once::{Once, OnceLock};
pub use atomic::{AtomicCell, Ordering};
pub use semaphore::Semaphore;

/// Result pour les opérations de synchronisation
pub type SyncResult<T> = Result<T, crate::error::SyncError>;

/// Guard empoisonné (lock acquis d'un mutex qui a panicked)
#[derive(Debug)]
pub struct PoisonError<T> {
    guard: T,
}

impl<T> PoisonError<T> {
    /// Crée une nouvelle erreur de poison
    #[inline]
    pub fn new(guard: T) -> Self {
        Self { guard }
    }
    
    /// Récupère le guard malgré le poison
    #[inline]
    pub fn into_inner(self) -> T {
        self.guard
    }
    
    /// Référence au guard
    #[inline]
    pub fn get_ref(&self) -> &T {
        &self.guard
    }
    
    /// Référence mutable au guard
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

/// Type pour mutex potentiellement empoisonné
pub type LockResult<Guard> = Result<Guard, PoisonError<Guard>>;
