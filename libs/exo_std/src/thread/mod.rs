//! Gestion des threads
//!
//! Ce module fournit des outils pour créer et gérer des threads.

extern crate alloc;
use alloc::boxed::Box;

pub mod builder;
pub mod local;
pub mod park;

// Réexportations
pub use builder::Builder;
pub use local::{LocalKey, AccessError};

use crate::error::ThreadError;
use core::any::Any;

/// ID de thread
pub type ThreadId = u64;

/// Handle sur un thread
pub struct JoinHandle<T> {
    thread_id: ThreadId,
    _phantom: core::marker::PhantomData<T>,
}

impl<T> JoinHandle<T> {
    /// Crée un nouveau handle (usage interne)
    const fn new(thread_id: ThreadId) -> Self {
        Self {
            thread_id,
            _phantom: core::marker::PhantomData,
        }
    }

    /// Attend que le thread se termine et retourne son résultat
    pub fn join(self) -> core::result::Result<T, ThreadError>
    where
        T: 'static,
    {
        #[cfg(feature = "test_mode")]
        {
            // En mode test, impossible de retourner T sans le stocker
            // Cette limitation est acceptable car test_mode est pour les tests unitaires
            panic!("JoinHandle::join not fully available in test mode - use integration tests");
        }
        
        #[cfg(not(feature = "test_mode"))]
        {
            use crate::syscall::thread::thread_join;
            
            // Appel du syscall pour attendre le thread
            unsafe {
                thread_join(self.thread_id)?;
            }
            
            // Note: Dans une implémentation complète, il faudrait:
            // 1. Un mécanisme pour stocker le résultat du thread (Box, TLS, etc.)
            // 2. Récupérer ce résultat après le join
            // Pour l'instant, on ne peut pas retourner T de manière sûre
            Err(ThreadError::JoinFailed)
        }
    }

    /// Retourne l'ID du thread
    pub const fn thread_id(&self) -> ThreadId {
        self.thread_id
    }
}

impl<T> core::fmt::Debug for JoinHandle<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("JoinHandle")
            .field("thread_id", &self.thread_id)
            .finish()
    }
}

/// Lance un nouveau thread
pub fn spawn<F, T>(f: F) -> core::result::Result<JoinHandle<T>, ThreadError>
where
    F: FnOnce() -> T +Send + 'static,
    T: Send + 'static,
{
    Builder::new().spawn(f)
}

/// Yield le CPU au scheduler
pub fn yield_now() {
    crate::syscall::thread::thread_yield();
}

/// Dort pendant une durée
pub fn sleep(dur: core::time::Duration) {
    unsafe {
        crate::syscall::thread::thread_sleep(dur.as_nanos() as u64);
    }
}

/// Retourne l'ID du thread courant
pub fn current_id() -> ThreadId {
    crate::syscall::thread::get_tid()
}

/// Informations sur le thread courant
pub struct Thread {
    id: ThreadId,
    name: Option<&'static str>,
}

impl Thread {
    /// Retourne le thread courant
    pub fn current() -> Self {
        Self {
            id: current_id(),
            // Note: Le nom nécessite un système de TLS complet pour être stocké/récupéré
            // Pour l'instant, None est la seule valeur sûre sans allocations globales
            name: None,
        }
    }

    /// Retourne l'ID
    pub const fn id(&self) -> ThreadId {
        self.id
    }

    /// Retourne le nom
    pub const fn name(&self) -> Option<&'static str> {
        self.name
    }
}

/// Result type pour les threads
pub type Result<T> = core::result::Result<T, Box<dyn Any + Send + 'static>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_id() {
        let id = current_id();
        assert!(id > 0);
    }

    #[test]
    fn test_yield() {
        yield_now(); // Ne devrait pas crasher
    }
}
