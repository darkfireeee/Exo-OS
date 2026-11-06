// kernel/src/scheduler/sync/rwlock.rs
//
// RwLock — multiple concurrent readers, single exclusive writer.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicI32, Ordering};

/// Counter: > 0 = reader count, -1 = write lock, 0 = free.
pub struct KRwLock<T> {
    state: AtomicI32,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for KRwLock<T> {}
unsafe impl<T: Send> Sync for KRwLock<T> {}

impl<T: Default> Default for KRwLock<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

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
            if s >= 0
                && self
                    .state
                    .compare_exchange_weak(s, s + 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                break;
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
// Compatibility aliases (some modules use the generic RwLock name)
// ─────────────────────────────────────────────────────────────────────────────
/// Public alias to [`KRwLock`] for crates using the generic name.
pub type RwLock<T> = KRwLock<T>;
/// Alias to [`KReadGuard`].
pub type RwLockReadGuard<'a, T> = KReadGuard<'a, T>;
/// Alias to [`KWriteGuard`].
pub type RwLockWriteGuard<'a, T> = KWriteGuard<'a, T>;
