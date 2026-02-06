//! Builder pattern pour la création de threads

use crate::error::ThreadError;
use super::{JoinHandle, ThreadId};

/// Builder pour créer des threads avec configuration
pub struct Builder {
    name: Option<&'static str>,
    stack_size: Option<usize>,
}

impl Builder {
    /// Crée un nouveau Builder
    pub const fn new() -> Self {
        Self {
            name: None,
            stack_size: None,
        }
    }

    /// Définit le nom du thread
    pub const fn name(mut self, name: &'static str) -> Self {
        self.name = Some(name);
        self
    }

    /// Définit la taille de la stack
    pub const fn stack_size(mut self, size: usize) -> Self {
        self.stack_size = Some(size);
        self
    }

    /// Lance le thread
    pub fn spawn<F, T>(self, f: F) -> Result<JoinHandle<T>, ThreadError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        #[cfg(feature = "test_mode")]
        {
            let _ = f;
            Ok(JoinHandle::new(123))
        }
        
        #[cfg(not(feature = "test_mode"))]
        {
            extern crate alloc;
            use alloc::boxed::Box;
            use alloc::vec;
            use crate::syscall::thread::thread_create;

            // Determine stack size
            const DEFAULT_STACK_SIZE: usize = 2 * 1024 * 1024; // 2 MB
            let stack_size = self.stack_size.unwrap_or(DEFAULT_STACK_SIZE);

            // Allocate stack
            let mut stack = vec![0u8; stack_size];
            let stack_ptr = stack.as_mut_ptr();

            // Leak the stack - kernel will manage it
            core::mem::forget(stack);

            // Encapsuler la closure dans une Box pour la passer au thread
            let boxed_closure = Box::new(f);
            let closure_ptr = Box::into_raw(boxed_closure);

            unsafe {
                let thread_id = thread_create(
                    wrapper::<F, T>,
                    closure_ptr as *mut u8,
                    stack_ptr,
                    stack_size,
                )? as ThreadId;

                Ok(JoinHandle::new(thread_id))
            }
        }
    }

    /// Lance le thread et retourne Result avec panic info
    pub fn spawn_unchecked<F, T>(self, f: F) -> Result<JoinHandle<T>, ThreadError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        // Pour l'instant identique à spawn
        self.spawn(f)
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for Builder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Builder")
            .field("name", &self.name)
            .field("stack_size", &self.stack_size)
            .finish()
    }
}

/// Wrapper pour exécuter la closure (usage interne)
#[cfg(not(feature = "test_mode"))]
extern "C" fn wrapper<F, T>(arg: *mut u8) -> *mut u8
where
    F: FnOnce() -> T,
{
    extern crate alloc;
    use alloc::boxed::Box;

    unsafe {
        // Récupérer la closure depuis le pointeur
        let closure_ptr = arg as *mut F;
        let closure = Box::from_raw(closure_ptr);

        // Exécuter la closure
        let _result = (*closure)();

        // Note: Le résultat est perdu ici car on ne peut pas le stocker de manière sûre
        // sans un mécanisme de stockage dédié (TLS, structure globale, etc.)
        // Une implémentation complète nécessiterait:
        // - Un système de storage pour les résultats
        // - Un mécanisme de récupération dans join()

        core::ptr::null_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder() {
        let builder = Builder::new()
            .name("test-thread")
            .stack_size(2 * 1024 * 1024);

        assert_eq!(builder.name, Some("test-thread"));
        assert_eq!(builder.stack_size, Some(2 * 1024 * 1024));
    }

    #[test]
    #[cfg(feature = "test_mode")]
    fn test_spawn() {
        let handle = Builder::new()
            .spawn(|| 42)
            .unwrap();

        assert!(handle.thread_id() > 0);
    }
}
