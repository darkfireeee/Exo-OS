// kernel/src/scheduler/sync/spinlock.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SpinLock — verrous tournants IRQ-safe (Exo-OS · Couche 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Deux variantes :
//   SpinLock<T>        — spinlock simple (pas d'IRQ save)
//   IrqSpinLock<T>     — spinlock + disable IRQ (pour code IRQ/non-IRQ)
//
// RÈGLE PREEMPT-01 respectée : la préemption est implicitement désactivée via
// les RAII guards.
// ═══════════════════════════════════════════════════════════════════════════════

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// SpinLock<T> — verrou tournant simple
// ─────────────────────────────────────────────────────────────────────────────

pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for SpinLock<T> {}
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(value),
        }
    }

    /// Acquiert le verrou (boucle active).
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        loop {
            // Essai optimiste (non-serialisant) avant le LOCK CMPXCHG.
            if !self.locked.load(Ordering::Relaxed) {
                if self
                    .locked
                    .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    break;
                }
            }
            core::hint::spin_loop();
        }
        SpinLockGuard {
            lock: self,
            _pd: PhantomData,
        }
    }

    /// Essai sans blocage. Retourne `None` si le verrou est déjà pris.
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| SpinLockGuard {
                lock: self,
                _pd: PhantomData,
            })
    }

    /// Libère sans garde (interne).
    unsafe fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
    _pd: PhantomData<*mut ()>, // !Send
}

impl<'a, T> Deref for SpinLockGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: Le guard est vivant => le verrou est acquis => accès exclusif à data.
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T> DerefMut for SpinLockGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: &mut self garantit l'unicité; le verrou est acquis => exclusivité.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'a, T> Drop for SpinLockGuard<'a, T> {
    fn drop(&mut self) {
        // SAFETY: unlock() ne doit être appelé qu'une fois — garanti par Drop.
        unsafe {
            self.lock.unlock();
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IrqSpinLock<T> — verrou tournant + disable IRQ
// ─────────────────────────────────────────────────────────────────────────────

pub struct IrqSpinLock<T> {
    inner: SpinLock<T>,
}

unsafe impl<T: Send> Send for IrqSpinLock<T> {}
unsafe impl<T: Send> Sync for IrqSpinLock<T> {}

impl<T> IrqSpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            inner: SpinLock::new(value),
        }
    }

    /// Acquiert le verrou en désactivant les IRQ. Les IRQ sont restaurées lors
    /// du drop du guard.
    pub fn lock_irq(&self) -> IrqSpinLockGuard<'_, T> {
        // Sauvegarder les IRQ et les désactiver.
        let rflags = save_and_disable_irq();
        let guard = self.inner.lock();
        IrqSpinLockGuard { guard, rflags }
    }

    pub fn try_lock_irq(&self) -> Option<IrqSpinLockGuard<'_, T>> {
        let rflags = save_and_disable_irq();
        match self.inner.try_lock() {
            Some(g) => Some(IrqSpinLockGuard { guard: g, rflags }),
            None => {
                restore_irq(rflags);
                None
            }
        }
    }
}

pub struct IrqSpinLockGuard<'a, T> {
    guard: SpinLockGuard<'a, T>,
    rflags: u64,
}

impl<'a, T> Deref for IrqSpinLockGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &*self.guard
    }
}

impl<'a, T> DerefMut for IrqSpinLockGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut *self.guard
    }
}

impl<'a, T> Drop for IrqSpinLockGuard<'a, T> {
    fn drop(&mut self) {
        // SpinLockGuard::drop() libère le verrou en premier, PUIS on restaure les IRQ.
        restore_irq(self.rflags);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Primitives d'IRQ (inline asm)
// ─────────────────────────────────────────────────────────────────────────────

#[inline(always)]
pub fn save_and_disable_irq() -> u64 {
    let rflags: u64;
    // SAFETY: pushfq/pop lisent RFLAGS; cli désactive les IRQ atomiquement; état restauré par restore_irq().
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {flags}",
            "cli",
            flags = out(reg) rflags,
            options(nomem, preserves_flags),
        );
    }
    rflags
}

#[inline(always)]
pub fn restore_irq(rflags: u64) {
    if rflags & (1 << 9) != 0 {
        // SAFETY: sti restaure l'état IRQ sauvegardé; bit IF était 1, on le remet à 1.
        unsafe {
            core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
        }
    }
}
