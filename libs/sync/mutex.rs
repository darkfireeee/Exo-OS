// libs/exo_std/src/sync/mutex.rs
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// Un mutex pour protéger des données partagées entre threads
pub struct Mutex<T: ?Sized> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

/// Un guard qui libère le mutex automatiquement
pub struct MutexGuard<'a, T: ?Sized + 'a> {
    lock: &'a Mutex<T>,
}

unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}

impl<T> Mutex<T> {
    /// Crée un nouveau mutex avec les données initiales
    pub const fn new(data: T) -> Self {
        Mutex {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }
    
    /// Acquiert le mutex, bloquant si nécessaire
    pub fn lock(&self) -> MutexGuard<T> {
        while self.locked.swap(true, Ordering::Acquire) {
            // Attente active (spinlock)
            // Dans une implémentation réelle, utiliserait un appel système pour dormir
            core::hint::spin_loop();
        }
        
        MutexGuard { lock: self }
    }
    
    /// Essaie d'acquérir le mutex sans bloquer
    pub fn try_lock(&self) -> Option<MutexGuard<T>> {
        if self.locked.swap(true, Ordering::Acquire) {
            None
        } else {
            Some(MutexGuard { lock: self })
        }
    }
    
    /// Consomme le mutex et retourne les données protégées
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> Mutex<T> {
    /// Force le déverrouillage du mutex
    /// 
    /// ATTENTION: Ne doit être utilisé que dans des cas très spécifiques,
    /// comme la gestion de paniques. Une mauvaise utilisation peut conduire
    /// à des comportements indéfinis.
    pub unsafe fn force_unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

impl<'a, T: ?Sized> Deref for MutexGuard<'a, T> {
    type Target = T;
    
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T: ?Sized> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'a, T: ?Sized> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread;
    use core::sync::atomic::{AtomicUsize, Ordering};
    
    #[test]
    fn test_mutex_basic() {
        let mutex = Mutex::new(42);
        
        {
            let mut guard = mutex.lock();
            *guard = 84;
            assert_eq!(*guard, 84);
        }
        
        let guard = mutex.lock();
        assert_eq!(*guard, 84);
    }
    
    #[test]
    fn test_mutex_try_lock() {
        let mutex = Mutex::new(42);
        
        let guard = mutex.try_lock().unwrap();
        assert!(mutex.try_lock().is_none());
        
        core::mem::drop(guard);
        assert!(mutex.try_lock().is_some());
    }
    
    #[test]
    fn test_mutex_threads() {
        let mutex = Mutex::new(AtomicUsize::new(0));
        let mut threads = Vec::new();
        
        for _ in 0..10 {
            let mutex_clone = &mutex;
            let handle = thread::spawn(move || {
                for _ in 0..100 {
                    let mut guard = mutex_clone.lock();
                    guard.fetch_add(1, Ordering::SeqCst);
                }
            }).unwrap();
            
            threads.push(handle);
        }
        
        for handle in threads {
            handle.join().unwrap();
        }
        
        let guard = mutex.lock();
        assert_eq!(guard.load(Ordering::SeqCst), 1000);
    }
}