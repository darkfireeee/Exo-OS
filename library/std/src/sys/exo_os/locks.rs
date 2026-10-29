//! `std::sys::exo_os::locks` — Futex-backed `Mutex`, `RwLock`, and `Condvar` (`SYS_FUTEX = 202`).

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU32, Ordering};
use exo_libc::platform::PalThread;
use exo_libc::ExoOs;

const UNLOCKED: u32 = 0;
const LOCKED: u32 = 1;
const CONTENDED: u32 = 2;

/// Futex-backed mutual exclusion primitive (`std::sync::Mutex`).
pub struct Mutex<T: ?Sized> {
    state: AtomicU32,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    #[inline]
    pub const fn new(val: T) -> Self {
        Self {
            state: AtomicU32::new(UNLOCKED),
            data: UnsafeCell::new(val),
        }
    }

    #[inline]
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> Mutex<T> {
    #[inline]
    pub fn lock(&self) -> Result<MutexGuard<'_, T>, ()> {
        if self
            .state
            .compare_exchange(UNLOCKED, LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            self.lock_contended();
        }
        Ok(MutexGuard { lock: self })
    }

    #[cold]
    fn lock_contended(&self) {
        let mut state = self.spin();
        if state == UNLOCKED {
            match self.state.compare_exchange(
                UNLOCKED,
                LOCKED,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(s) => state = s,
            }
        }
        loop {
            if state != CONTENDED && self.state.swap(CONTENDED, Ordering::Acquire) == UNLOCKED {
                return;
            }
            let _ = ExoOs::futex_wait(&self.state, CONTENDED, None);
            state = self.spin();
        }
    }

    fn spin(&self) -> u32 {
        let mut spin = 64;
        loop {
            let state = self.state.load(Ordering::Relaxed);
            if state != LOCKED || spin == 0 {
                return state;
            }
            core::hint::spin_loop();
            spin -= 1;
        }
    }

    #[inline]
    fn unlock(&self) {
        if self.state.swap(UNLOCKED, Ordering::Release) == CONTENDED {
            let _ = ExoOs::futex_wake(&self.state, 1);
        }
    }
}

pub struct MutexGuard<'a, T: ?Sized + 'a> {
    lock: &'a Mutex<T>,
}

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.unlock();
    }
}

/// Futex-backed reader-writer lock (`std::sync::RwLock`).
pub struct RwLock<T: ?Sized> {
    /// `0` = unlocked, `0xFFFF_FFFF` = write-locked, `1..0xFFFF_FFFE` = reader count.
    state: AtomicU32,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Send for RwLock<T> {}
unsafe impl<T: ?Sized + Send + Sync> Sync for RwLock<T> {}

const WRITE_LOCKED: u32 = u32::MAX;

impl<T> RwLock<T> {
    #[inline]
    pub const fn new(val: T) -> Self {
        Self {
            state: AtomicU32::new(0),
            data: UnsafeCell::new(val),
        }
    }
}

impl<T: ?Sized> RwLock<T> {
    pub fn read(&self) -> Result<RwLockReadGuard<'_, T>, ()> {
        loop {
            let s = self.state.load(Ordering::Relaxed);
            if s != WRITE_LOCKED {
                if self
                    .state
                    .compare_exchange_weak(s, s + 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return Ok(RwLockReadGuard { lock: self });
                }
            } else {
                let _ = ExoOs::futex_wait(&self.state, WRITE_LOCKED, None);
            }
        }
    }

    pub fn write(&self) -> Result<RwLockWriteGuard<'_, T>, ()> {
        loop {
            let s = self.state.load(Ordering::Relaxed);
            if s == 0 {
                if self
                    .state
                    .compare_exchange_weak(0, WRITE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return Ok(RwLockWriteGuard { lock: self });
                }
            } else {
                let _ = ExoOs::futex_wait(&self.state, s, None);
            }
        }
    }
}

pub struct RwLockReadGuard<'a, T: ?Sized + 'a> {
    lock: &'a RwLock<T>,
}

impl<T: ?Sized> Deref for RwLockReadGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwLockReadGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        if self.lock.state.fetch_sub(1, Ordering::Release) == 1 {
            let _ = ExoOs::futex_wake(&self.lock.state, i32::MAX as u32);
        }
    }
}

pub struct RwLockWriteGuard<'a, T: ?Sized + 'a> {
    lock: &'a RwLock<T>,
}

impl<T: ?Sized> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for RwLockWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwLockWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.state.store(0, Ordering::Release);
        let _ = ExoOs::futex_wake(&self.lock.state, i32::MAX as u32);
    }
}

/// Futex-backed condition variable (`std::sync::Condvar`).
pub struct Condvar {
    seq: AtomicU32,
}

impl Default for Condvar {
    fn default() -> Self {
        Self::new()
    }
}

impl Condvar {
    #[inline]
    pub const fn new() -> Self {
        Self {
            seq: AtomicU32::new(0),
        }
    }

    pub fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> Result<MutexGuard<'a, T>, ()> {
        let seq = self.seq.load(Ordering::Relaxed);
        let mutex = guard.lock;
        drop(guard);
        let _ = ExoOs::futex_wait(&self.seq, seq, None);
        mutex.lock()
    }

    #[inline]
    pub fn notify_one(&self) {
        self.seq.fetch_add(1, Ordering::Release);
        let _ = ExoOs::futex_wake(&self.seq, 1);
    }

    #[inline]
    pub fn notify_all(&self) {
        self.seq.fetch_add(1, Ordering::Release);
        let _ = ExoOs::futex_wake(&self.seq, i32::MAX as u32);
    }
}
