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
            use super::storage;

            // Alloue un ID de thread et stocke le résultat immédiatement
            let thread_id = storage::allocate_slot();
            let result = f();
            storage::store_result(thread_id, result);

            Ok(JoinHandle::new(thread_id))
        }

        #[cfg(not(feature = "test_mode"))]
        {
            extern crate alloc;
            use alloc::boxed::Box;
            use alloc::vec;
            use crate::syscall::thread::thread_create;
            use super::storage;

            // Determine stack size
            const DEFAULT_STACK_SIZE: usize = 2 * 1024 * 1024; // 2 MB
            let stack_size = self.stack_size.unwrap_or(DEFAULT_STACK_SIZE);

            // Allocate stack
            let mut stack = vec![0u8; stack_size];
            let stack_ptr = stack.as_mut_ptr();

            // Leak the stack - kernel will manage it
            core::mem::forget(stack);

            // Alloue un thread ID depuis le système de stockage
            let thread_id = storage::allocate_slot();

            // Encapsuler la closure ET le thread_id dans une Box
            let boxed_data = Box::new((f, thread_id));
            let data_ptr = Box::into_raw(boxed_data);

            unsafe {
                let kernel_tid = thread_create(
                    wrapper::<F, T>,
                    data_ptr as *mut u8,
                    stack_ptr,
                    stack_size,
                )? as ThreadId;

                // Note: On utilise notre thread_id alloué, pas celui du kernel
                // pour garantir l'unicité dans le système de stockage
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
    T: Send + 'static,
{
    extern crate alloc;
    use alloc::boxed::Box;
    use super::storage;

    unsafe {
        // Récupérer le pointeur vers la paire (closure, thread_id)
        let ptr = arg as *mut (F, super::ThreadId);
        let (closure, thread_id) = *Box::from_raw(ptr);

        // Exécuter la closure et stocker le résultat
        let result = closure();
        storage::store_result(thread_id, result);

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
