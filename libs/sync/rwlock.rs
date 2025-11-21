// libs/exo_std/src/sync/rwlock.rs
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicIsize, Ordering};

/// Un lock de lecture-écriture (readers-writer lock)
pub struct RwLock<T: ?Sized> {
    state: AtomicIsize, // Négatif si écriture en cours, positif = nombre de lecteurs
    data: UnsafeCell<T>,
}

/// Un guard pour une lecture
pub struct RwLockReadGuard<'a, T: ?Sized + 'a> {
    lock: &'a RwLock<T>,
}

/// Un guard pour une écriture
pub struct RwLockWriteGuard<'a, T: ?Sized + 'a> {
    lock: &'a RwLock<T>,
}

unsafe impl<T: ?Sized + Send> Sync for RwLock<T> {}
unsafe impl<T: ?Sized + Send> Send for RwLock<T> {}

const WRITE_LOCKED: isize = -1;
const MAX_READERS: isize = WRITE_LOCKED - 1;

impl<T> RwLock<T> {
    /// Crée un nouveau RwLock avec les données initiales
    pub const fn new(data: T) -> Self {
        RwLock {
            state: AtomicIsize::new(0),
            data: UnsafeCell::new(data),
        }
    }
    
    /// Acquiert le lock en lecture, bloquant si nécessaire
    pub fn read(&self) -> RwLockReadGuard<T> {
        while !self.try_lock_read() {
            core::hint::spin_loop();
        }
        
        RwLockReadGuard { lock: self }
    }
    
    /// Acquiert le lock en écriture, bloquant si nécessaire
    pub fn write(&self) -> RwLockWriteGuard<T> {
        while !self.try_lock_write() {
            core::hint::spin_loop();
        }
        
        RwLockWriteGuard { lock: self }
    }
    
    /// Essaie d'acquérir le lock en lecture sans bloquer
    pub fn try_read(&self) -> Option<RwLockReadGuard<T>> {
        if self.try_lock_read() {
            Some(RwLockReadGuard { lock: self })
        } else {
            None
        }
    }
    
    /// Essaie d'acquérir le lock en écriture sans bloquer
    pub fn try_write(&self) -> Option<RwLockWriteGuard<T>> {
        if self.try_lock_write() {
            Some(RwLockWriteGuard { lock: self })
        } else {
            None
        }
    }
    
    /// Consomme le RwLock et retourne les données protégées
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
    
    /// Tente d'acquérir le lock en lecture
    fn try_lock_read(&self) -> bool {
        let mut state = self.state.load(Ordering::Relaxed);
        
        loop {
            if state < 0 {
                // Un writer détient le lock
                return false;
            }
            
            if state == MAX_READERS {
                // Trop de lecteurs
                return false;
            }
            
            match self.state.compare_exchange_weak(
                state,
                state + 1,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(new_state) => state = new_state,
            }
        }
    }
    
    /// Tente d'acquérir le lock en écriture
    fn try_lock_write(&self) -> bool {
        match self.state.compare_exchange(
            0,
            WRITE_LOCKED,
            Ordering::Acquire,
            Ordering::Relaxed,
        ) {
            Ok(_) => true,
            Err(_) => false,
        }
    }
}

impl<T: ?Sized> RwLock<T> {
    /// Force le déverrouillage du RwLock
    /// 
    /// ATTENTION: Ne doit être utilisé que dans des cas très spécifiques,
    /// comme la gestion de paniques. Une mauvaise utilisation peut conduire
    /// à des comportements indéfinis.
    pub unsafe fn force_unlock(&self) {
        self.state.store(0, Ordering::Release);
    }
}

impl<'a, T: ?Sized> Deref for RwLockReadGuard<'a, T> {
    type Target = T;
    
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T: ?Sized> Drop for RwLockReadGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.state.fetch_sub(1, Ordering::Release);
    }
}

impl<'a, T: ?Sized> Deref for RwLockWriteGuard<'a, T> {
    type Target = T;
    
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T: ?Sized> DerefMut for RwLockWriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'a, T: ?Sized> Drop for RwLockWriteGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.state.store(0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread;
    use core::sync::atomic::{AtomicUsize, Ordering};
    
    #[test]
    fn test_rwlock_basic() {
        let rwlock = RwLock::new(42);
        
        {
            let read_guard = rwlock.read();
            assert_eq!(*read_guard, 42);
        }
        
        {
            let mut write_guard = rwlock.write();
            *write_guard = 84;
            assert_eq!(*write_guard, 84);
        }
        
        let read_guard = rwlock.read();
        assert_eq!(*read_guard, 84);
    }
    
    #[test]
    fn test_rwlock_multiple_readers() {
        let rwlock = RwLock::new(42);
        
        {
            let r1 = rwlock.read();
            let r2 = rwlock.read();
            let r3 = rwlock.read();
            
            assert_eq!(*r1, 42);
            assert_eq!(*r2, 42);
            assert_eq!(*r3, 42);
        }
        
        let mut w = rwlock.write();
        *w = 84;
        assert_eq!(*w, 84);
    }
    
    #[test]
    fn test_rwlock_threads() {
        let rwlock = RwLock::new(AtomicUsize::new(0));
        let mut threads = Vec::new();
        
        // Créer des threads lecteurs et écrivains
        for i in 0..20 {
            let rwlock_clone = &rwlock;
            let handle = if i % 3 == 0 {
                // Thread écrivain
                thread::spawn(move || {
                    for _ in 0..50 {
                        let mut guard = rwlock_clone.write();
                        guard.fetch_add(1, Ordering::SeqCst);
                    }
                })
            } else {
                // Thread lecteur
                thread::spawn(move || {
                    for _ in 0..100 {
                        let guard = rwlock_clone.read();
                        let _ = guard.load(Ordering::SeqCst);
                    }
                })
            }.unwrap();
            
            threads.push(handle);
        }
        
        for handle in threads {
            handle.join().unwrap();
        }
        
        let guard = rwlock.read();
        assert_eq!(guard.load(Ordering::SeqCst), 333); // 7 écrivains * 50 incréments
    }
}