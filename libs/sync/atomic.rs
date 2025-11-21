// libs/exo_std/src/sync/atomic.rs
pub use core::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering};
pub use core::sync::atomic::{AtomicI8, AtomicI16, AtomicI32, AtomicI64};
pub use core::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, AtomicU64};

/// Atomic pointer with fence operations
#[repr(transparent)]
pub struct AtomicPtr<T> {
    inner: core::sync::atomic::AtomicPtr<T>,
}

impl<T> AtomicPtr<T> {
    /// Crée un nouveau AtomicPtr avec la valeur initiale
    pub fn new(ptr: *mut T) -> Self {
        Self {
            inner: core::sync::atomic::AtomicPtr::new(ptr),
        }
    }
    
    /// Charge la valeur avec la barrière mémoire spécifiée
    pub fn load(&self, order: Ordering) -> *mut T {
        self.inner.load(order)
    }
    
    /// Stocke la valeur avec la barrière mémoire spécifiée
    pub fn store(&self, ptr: *mut T, order: Ordering) {
        self.inner.store(ptr, order);
    }
    
    /// Échange la valeur avec une nouvelle valeur
    pub fn swap(&self, ptr: *mut T, order: Ordering) -> *mut T {
        self.inner.swap(ptr, order)
    }
    
    /// Compare et échange la valeur
    pub fn compare_exchange(
        &self,
        current: *mut T,
        new: *mut T,
        success: Ordering,
        failure: Ordering,
    ) -> Result<*mut T, *mut T> {
        self.inner.compare_exchange(current, new, success, failure)
    }
    
    /// Compare et échange faible (peut échouer spurious)
    pub fn compare_exchange_weak(
        &self,
        current: *mut T,
        new: *mut T,
        success: Ordering,
        failure: Ordering,
    ) -> Result<*mut T, *mut T> {
        self.inner.compare_exchange_weak(current, new, success, failure)
    }
    
    /// Ajoute un offset au pointeur
    pub fn fetch_add(&self, offset: isize, order: Ordering) -> *mut T {
        self.inner.fetch_add(offset, order)
    }
    
    /// Soustrait un offset au pointeur
    pub fn fetch_sub(&self, offset: isize, order: Ordering) -> *mut T {
        self.inner.fetch_sub(offset, order)
    }
}

unsafe impl<T: Send> Send for AtomicPtr<T> {}
unsafe impl<T: Sync> Sync for AtomicPtr<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::Ordering;
    
    #[test]
    fn test_atomic_bool() {
        let atomic = AtomicBool::new(false);
        
        assert!(!atomic.load(Ordering::SeqCst));
        
        atomic.store(true, Ordering::SeqCst);
        assert!(atomic.load(Ordering::SeqCst));
        
        let old = atomic.swap(false, Ordering::SeqCst);
        assert!(old);
        assert!(!atomic.load(Ordering::SeqCst));
        
        let result = atomic.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst);
        assert!(result.is_ok());
        assert!(atomic.load(Ordering::SeqCst));
    }
    
    #[test]
    fn test_atomic_usize() {
        let atomic = AtomicUsize::new(42);
        
        assert_eq!(atomic.load(Ordering::SeqCst), 42);
        
        atomic.store(84, Ordering::SeqCst);
        assert_eq!(atomic.load(Ordering::SeqCst), 84);
        
        let old = atomic.swap(126, Ordering::SeqCst);
        assert_eq!(old, 84);
        assert_eq!(atomic.load(Ordering::SeqCst), 126);
        
        let result = atomic.compare_exchange(126, 168, Ordering::SeqCst, Ordering::SeqCst);
        assert!(result.is_ok());
        assert_eq!(atomic.load(Ordering::SeqCst), 168);
        
        atomic.fetch_add(1, Ordering::SeqCst);
        assert_eq!(atomic.load(Ordering::SeqCst), 169);
        
        atomic.fetch_sub(69, Ordering::SeqCst);
        assert_eq!(atomic.load(Ordering::SeqCst), 100);
    }
    
    #[test]
    fn test_atomic_ptr() {
        let mut value1 = 42;
        let mut value2 = 84;
        
        let atomic = AtomicPtr::new(&mut value1 as *mut _);
        
        assert_eq!(unsafe { *atomic.load(Ordering::SeqCst) }, 42);
        
        let old_ptr = atomic.swap(&mut value2 as *mut _, Ordering::SeqCst);
        assert_eq!(unsafe { *old_ptr }, 42);
        assert_eq!(unsafe { *atomic.load(Ordering::SeqCst) }, 84);
        
        let result = atomic.compare_exchange(
            &mut value2 as *mut _,
            &mut value1 as *mut _,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        assert!(result.is_ok());
        assert_eq!(unsafe { *atomic.load(Ordering::SeqCst) }, 42);
    }
}