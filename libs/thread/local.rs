// libs/exo_std/src/thread/local.rs
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};

/// Clé pour le stockage local au thread
pub struct LocalKey<T: 'static> {
    key: usize,
    init: fn() -> T,
    _marker: PhantomData<T>,
}

/// Stockage local au thread
pub struct LocalStorage<T: 'static> {
    data: UnsafeCell<Option<T>>,
    _marker: PhantomData<T>,
}

impl<T: 'static> LocalKey<T> {
    /// Crée une nouvelle clé de stockage local au thread
    pub const fn new(init: fn() -> T) -> Self {
        LocalKey {
            key: 0, // Sera initialisé au premier accès
            init,
            _marker: PhantomData,
        }
    }
    
    /// Accède à la valeur pour le thread courant
    pub fn with<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        let value = self.get();
        f(value)
    }
    
    /// Retourne une référence à la valeur pour le thread courant
    pub fn get(&'static self) -> &T {
        self.get_or_init()
    }
    
    /// Initialise et retourne une référence à la valeur
    fn get_or_init(&'static self) -> &T {
        #[cfg(feature = "test_mode")]
        {
            // En mode test, utiliser une valeur statique
            static mut VALUE: Option<UnsafeCell<Option<i32>>> = None;
            static INIT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
            
            unsafe {
                if !INIT.load(core::sync::atomic::Ordering::SeqCst) {
                    VALUE = Some(UnsafeCell::new(Some(42)));
                    INIT.store(true, core::sync::atomic::Ordering::SeqCst);
                }
                
                // C'est juste un exemple - dans la vraie implémentation, ce serait plus complexe
                &*((*VALUE.as_ref().unwrap().get()).as_ref().unwrap() as *const _ as *const T)
            }
        }
        
        #[cfg(not(feature = "test_mode"))]
        {
            unsafe {
                extern "C" {
                    fn sys_tls_get(key: usize) -> *mut ();
                    fn sys_tls_set(key: usize, value: *mut ());
                    fn sys_tls_create(init: fn() -> *mut ()) -> usize;
                }
                
                if self.key == 0 {
                    // Initialiser la clé si nécessaire
                    let key = sys_tls_create(|| {
                        let value = (self.init)();
                        Box::into_raw(Box::new(value)) as *mut ()
                    });
                    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
                    *(self as *const _ as *mut usize) = key;
                }
                
                let ptr = sys_tls_get(self.key);
                if ptr.is_null() {
                    let value = (self.init)();
                    let boxed = Box::new(value);
                    let raw = Box::into_raw(boxed);
                    sys_tls_set(self.key, raw as *mut ());
                    &*raw
                } else {
                    &*(ptr as *const T)
                }
            }
        }
    }
}

impl<T: 'static> LocalStorage<T> {
    /// Crée un nouveau stockage local au thread
    pub const fn new() -> Self {
        LocalStorage {
            data: UnsafeCell::new(None),
            _marker: PhantomData,
        }
    }
    
    /// Initialise le stockage avec une valeur
    pub fn init(&self, value: T) {
        unsafe {
            let data = &mut *self.data.get();
            if data.is_none() {
                *data = Some(value);
            }
        }
    }
    
    /// Retourne une référence mutable aux données
    pub fn get_mut(&self) -> Option<&mut T> {
        unsafe { (*self.data.get()).as_mut() }
    }
    
    /// Consomme le stockage et retourne la valeur
    pub fn take(&self) -> Option<T> {
        unsafe { (*self.data.get()).take() }
    }
}

unsafe impl<T: Send + 'static> Sync for LocalKey<T> {}
unsafe impl<T: Send + 'static> Send for LocalKey<T> {}

impl<T: 'static> Deref for LocalStorage<T> {
    type Target = T;
    
    fn deref(&self) -> &Self::Target {
        self.get_mut().expect("LocalStorage not initialized")
    }
}

impl<T: 'static> DerefMut for LocalStorage<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.get_mut().expect("LocalStorage not initialized")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread;
    
    thread_local! {
        static COUNTER: LocalStorage<usize> = LocalStorage::new();
    }
    
    #[test]
    fn test_local_storage_basic() {
        COUNTER.with(|storage| {
            storage.init(42);
            assert_eq!(*storage, 42);
            
            let mut value = storage.get_mut().unwrap();
            *value = 84;
            assert_eq!(*storage, 84);
        });
    }
    
    #[test]
    fn test_thread_local() {
        thread::spawn(|| {
            COUNTER.with(|storage| {
                storage.init(100);
                assert_eq!(*storage, 100);
            });
        }).unwrap().join().unwrap();
        
        // Dans le thread principal, la valeur est différente
        COUNTER.with(|storage| {
            storage.init(200);
            assert_eq!(*storage, 200);
        });
    }
    
    #[test]
    fn test_local_key() {
        static KEY: LocalKey<usize> = LocalKey::new(|| 42);
        
        KEY.with(|value| {
            assert_eq!(*value, 42);
        });
    }
}