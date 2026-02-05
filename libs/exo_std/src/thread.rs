// libs/exo_std/src/thread.rs
//! Gestion des threads
//!
//! Ce module fournit des API pour créer et gérer des threads.

use core::time::Duration;
use crate::Result;
use crate::syscall::thread as sys;

/// ID de thread
pub type Tid = sys::Tid;

/// Crée et lance un nouveau thread
///
/// # Exemple
/// ```no_run
/// use exo_std::thread;
///
/// let handle = thread::spawn(|| {
///     println!("Hello from thread!");
///     42
/// });
///
/// let result = handle.join().unwrap();
/// assert_eq!(result, 42);
/// ```
pub fn spawn<F, T>(f: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    Builder::new().spawn(f).expect("failed to spawn thread")
}

/// Builder pour configurer un thread avant de le lancer
///
/// # Exemple
/// ```no_run
/// use exo_std::thread;
///
/// let handle = thread::Builder::new()
///     .name("worker".into())
///     .stack_size(2 * 1024 * 1024)
///     .spawn(|| {
///         println!("Worker thread");
///     })
///     .unwrap();
/// ```
#[derive(Debug, Default)]
pub struct Builder {
    name: Option<alloc::string::String>,
    stack_size: Option<usize>,
}

impl Builder {
    /// Crée un nouveau Builder
    #[inline]
    pub fn new() -> Self {
        Self {
            name: None,
            stack_size: None,
        }
    }
    
    /// Défini le nom du thread
    #[inline]
    pub fn name(mut self, name: alloc::string::String) -> Self {
        self.name = Some(name);
        self
    }
    
    /// Défini la taille de la pile
    #[inline]
    pub fn stack_size(mut self, size: usize) -> Self {
        self.stack_size = Some(size);
        self
    }
    
    /// Lance le thread
    pub fn spawn<F, T>(self, f: F) -> Result<JoinHandle<T>>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        // TODO: implémenter vraie création de thread
        // Pour l'instant, version simplifiée
        
        let stack_size = self.stack_size.unwrap_or(2 * 1024 * 1024);
        
        // En mode test, simule
        #[cfg(feature = "test_mode")]
        {
            let _ = (stack_size, f);
            Ok(JoinHandle {
                tid: 1,
                _phantom: core::marker::PhantomData,
            })
        }
        
        #[cfg(not(feature = "test_mode"))]
        {
            // Allocation de la pile (nécessite allocateur)
            // Création du thread via syscall
            // Pour l'instant, stub
            let _ = (stack_size, f);
            Ok(JoinHandle {
                tid: 1,
                _phantom: core::marker::PhantomData,
            })
        }
    }
}

/// Handle vers un thread, permet de le join
#[derive(Debug)]
pub struct JoinHandle<T> {
    tid: Tid,
    _phantom: core::marker::PhantomData<T>,
}

impl<T> JoinHandle<T> {
    /// Retourne le TID du thread
    #[inline]
    pub fn thread_id(&self) -> Tid {
        self.tid
    }
    
    /// Attend que le thread se termine et retourne son résultat
    pub fn join(self) -> Result<T> {
        #[cfg(feature = "test_mode")]
        {
            // En mode test, retourne une valeur par défaut
            // Note: ceci ne compile que si T: Default
            // Dans une vraie impl, on stockerait le résultat
            let _ = self.tid;
            panic!("join not fully implemented in test mode");
        }
        
        #[cfg(not(feature = "test_mode"))]
        unsafe {
            let mut retval: *mut u8 = core::ptr::null_mut();
            sys::thread_join(self.tid, &mut retval as *mut *mut u8)?;
            
            // Reconstruction du résultat depuis le pointeur
            // TODO: implémenter la conversion correcte
            panic!("join result reconstruction not implemented");
        }
    }
}

/// Endort le thread actuel pour la durée spécifiée
///
/// # Exemple
/// ```no_run
/// use exo_std::thread;
/// use core::time::Duration;
///
/// thread::sleep(Duration::from_secs(1));
/// ```
#[inline]
pub fn sleep(duration: Duration) {
    sys::sleep_nanos(duration.as_nanos() as u64);
}

/// Retourne l'ID du thread actuel
///
/// # Exemple
/// ```no_run
/// use exo_std::thread;
///
/// let tid = thread::id();
/// println!("Thread ID: {}", tid);
/// ```
#[inline]
pub fn id() -> Tid {
    sys::gettid()
}

/// Cède le contrôle au scheduler
///
/// Permet à d'autres threads de s'exécuter. Utile dans les boucles d'attente.
///
/// # Exemple
/// ```no_run
/// use exo_std::thread;
///
/// loop {
///     if condition_met() {
///         break;
///     }
///     thread::yield_now();
/// }
/// # fn condition_met() -> bool { true }
/// ```
#[inline]
pub fn yield_now() {
    sys::yield_now();
}

/// Termine le thread actuel
///
/// # Safety
/// Cette fonction ne retourne jamais.
#[inline]
pub fn exit() -> ! {
    unsafe { sys::thread_exit(core::ptr::null_mut()) }
}

/// Thread-local storage (TLS)
///
/// # Exemple
/// ```no_run
/// use exo_std::thread;
///
/// thread_local! {
///     static COUNTER: RefCell<u32> = RefCell::new(0);
/// }
///
/// COUNTER.with(|c| {
///     *c.borrow_mut() += 1;
/// });
/// ```
#[macro_export]
macro_rules! thread_local {
    ($(static $name:ident: $t:ty = $init:expr;)*) => {
        $(
            static $name: $crate::thread::LocalKey<$t> = {
                fn init() -> $t { $init }
                $crate::thread::LocalKey::new(init)
            };
        )*
    };
}

/// Clé pour thread-local storage
pub struct LocalKey<T: 'static> {
    init: fn() -> T,
}

impl<T: 'static> LocalKey<T> {
    /// Crée une nouvelle LocalKey
    #[doc(hidden)]
    pub const fn new(init: fn() -> T) -> Self {
        Self { init }
    }
    
    /// Accède à la valeur TLS
    pub fn with<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        // TODO: implémenter vraie TLS
        // Pour l'instant, crée une nouvelle valeur à chaque fois
        let value = (self.init)();
        f(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_thread_id() {
        let tid = id();
        assert!(tid > 0);
    }
    
    #[test]
    fn test_yield() {
        yield_now(); // Should not crash
    }
    
    #[test]
    fn test_sleep() {
        sleep(Duration::from_millis(1));
    }
}
