// kernel/src/scheduler/sync/spinlock.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SpinLock — IRQ-safe spinlocks (Exo-OS · Layer 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Two variants:
//   SpinLock<T>        — simple spinlock (no IRQ save)
//   IrqSpinLock<T>     — spinlock + disable IRQ (for IRQ/non-IRQ code)
//
// RULE PREEMPT-01 respected: preemption is implicitly disabled via
// RAII guards.
// ═══════════════════════════════════════════════════════════════════════════════

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::scheduler::core::preempt::PreemptGuard;

// ─────────────────────────────────────────────────────────────────────────────
// SpinLock<T> — simple spinlock
// ─────────────────────────────────────────────────────────────────────────────

pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for SpinLock<T> {}
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T: Default> Default for SpinLock<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(value),
        }
    }

    /// Acquires lock (busy spin).
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        let preempt = PreemptGuard::new();
        loop {
            // Optimistic check (non-serializing) before LOCK CMPXCHG.
            if !self.locked.load(Ordering::Relaxed)
                && self
                    .locked
                    .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                break;
            }
            core::hint::spin_loop();
        }
        SpinLockGuard {
            lock: self,
            _preempt: preempt,
            _pd: PhantomData,
        }
    }

    /// Non-blocking attempt. Returns `None` if lock is already held.
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        let preempt = PreemptGuard::new();
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| SpinLockGuard {
                lock: self,
                _preempt: preempt,
                _pd: PhantomData,
            })
    }

    /// Unlocks without guard (internal).
    unsafe fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
    _preempt: PreemptGuard,
    _pd: PhantomData<*mut ()>, // !Send
}

impl<'a, T> Deref for SpinLockGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: Guard is alive => lock is held => exclusive access to data.
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T> DerefMut for SpinLockGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: &mut self guarantees uniqueness; lock is held => exclusivity.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'a, T> Drop for SpinLockGuard<'a, T> {
    fn drop(&mut self) {
        // SAFETY: unlock() must be called only once — guaranteed by Drop.
        unsafe {
            self.lock.unlock();
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IrqSpinLock<T> — spinlock + disable IRQ
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

    /// Acquires lock disabling IRQs. IRQs are restored upon
    /// guard drop.
    pub fn lock_irq(&self) -> IrqSpinLockGuard<'_, T> {
        // Save IRQs and disable them.
        let rflags = save_and_disable_irq();
        let guard = self.inner.lock();
        IrqSpinLockGuard {
            guard: core::mem::ManuallyDrop::new(guard),
            rflags,
        }
    }

    pub fn try_lock_irq(&self) -> Option<IrqSpinLockGuard<'_, T>> {
        let rflags = save_and_disable_irq();
        match self.inner.try_lock() {
            Some(g) => Some(IrqSpinLockGuard {
                guard: core::mem::ManuallyDrop::new(g),
                rflags,
            }),
            None => {
                restore_irq(rflags);
                None
            }
        }
    }
}

pub struct IrqSpinLockGuard<'a, T> {
    guard: core::mem::ManuallyDrop<SpinLockGuard<'a, T>>,
    rflags: u64,
}

impl<'a, T> Deref for IrqSpinLockGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<'a, T> DerefMut for IrqSpinLockGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

impl<'a, T> Drop for IrqSpinLockGuard<'a, T> {
    fn drop(&mut self) {
        // AUDIT-SCHED2: release lock FIRST, THEN restore IRQs.
        // Otherwise, an IRQ arriving with lock held could deadlock with same lock in ISR.
        unsafe {
            core::mem::ManuallyDrop::drop(&mut self.guard);
        }
        restore_irq(self.rflags);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IRQ primitives (inline asm)
// ─────────────────────────────────────────────────────────────────────────────

#[inline(always)]
pub fn save_and_disable_irq() -> u64 {
    let rflags: u64;
    // SAFETY: pushfq/pop read RFLAGS; cli atomically disables IRQs; state restored by restore_irq().
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
        // SAFETY: sti restores saved IRQ state; IF bit was 1, setting back to 1.
        unsafe {
            core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
        }
    }
}
