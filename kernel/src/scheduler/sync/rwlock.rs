// kernel/src/scheduler/sync/rwlock.rs
//
// RwLock — plusieurs lecteurs simultanés, un seul écrivain exclusif.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicI32, Ordering};

/// Compteur : > 0 = nb lecteurs, -1 = verrou écriture, 0 = libre.
pub struct KRwLock<T> {
    state: AtomicI32,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for KRwLock<T> {}
unsafe impl<T: Send> Sync for KRwLock<T> {}

impl<T> KRwLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            state: AtomicI32::new(0),
            data: UnsafeCell::new(value),
        }
    }

    pub fn read(&self) -> KReadGuard<'_, T> {
        loop {
            let s = self.state.load(Ordering::Acquire);
            if s >= 0 {
                if self
                    .state
                    .compare_exchange_weak(s, s + 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    break;
                }
            }
            core::hint::spin_loop();
        }
        KReadGuard { rw: self }
    }

    pub fn write(&self) -> KWriteGuard<'_, T> {
        loop {
            if self
                .state
                .compare_exchange_weak(0, -1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }
            core::hint::spin_loop();
        }
        KWriteGuard { rw: self }
    }
}

pub struct KReadGuard<'a, T> {
    rw: &'a KRwLock<T>,
}
impl<'a, T> core::ops::Deref for KReadGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.rw.data.get() }
    }
}
impl<'a, T> Drop for KReadGuard<'a, T> {
    fn drop(&mut self) {
        self.rw.state.fetch_sub(1, Ordering::Release);
    }
}

pub struct KWriteGuard<'a, T> {
    rw: &'a KRwLock<T>,
}
impl<'a, T> core::ops::Deref for KWriteGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.rw.data.get() }
    }
}
impl<'a, T> core::ops::DerefMut for KWriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.rw.data.get() }
    }
}
impl<'a, T> Drop for KWriteGuard<'a, T> {
    fn drop(&mut self) {
        self.rw.state.store(0, Ordering::Release);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Alias de compatibilité (certains modules utilisent le nom générique RwLock)
// ─────────────────────────────────────────────────────────────────────────────
/// Alias public vers [`KRwLock`] pour les crates qui utilisent le nom générique.
pub type RwLock<T> = KRwLock<T>;
/// Alias vers [`KReadGuard`].
pub type RwLockReadGuard<'a, T> = KReadGuard<'a, T>;
/// Alias vers [`KWriteGuard`].
pub type RwLockWriteGuard<'a, T> = KWriteGuard<'a, T>;
