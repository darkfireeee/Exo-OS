// kernel/src/ipc/sync/futex.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// FUTEX IPC — PURE DELEGATION SHIM -> memory::utils::futex_table
// (Exo-OS · IPC Layer 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RULE IPC-02: THIS MODULE CONTAINS NO LOCAL FUTEX LOGIC.
//   Pure delegation to memory::utils::futex_table::FUTEX_TABLE.
//   The SINGLE futex table resides in memory/ (LAYER 0) and is shared
//   by all subsystems (ipc/, scheduler/, process/).
//
//   REASON: single source of truth — avoids cross-subsystem races.
//           TLA+/Coq proof scope limited to the memory table.
//
// This module provides:
//   • FutexKey    — IPC key (physmap virtual address of the AtomicU32)
//   • futex_wait  — WAIT with spin-poll on waiter.woken from memory/
//   • futex_wake  — WAKE (delegates directly)
//   • futex_stats — statistics read from memory::utils::FUTEX_STATS
//
// Bucket management, the linked list of waiters, and wake logic
// reside EXCLUSIVELY in memory::utils::futex_table.
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, Ordering};

use crate::ipc::core::types::IpcError;
use crate::memory::utils::futex_table::{
    futex_cancel as mem_futex_cancel, futex_requeue as mem_futex_requeue,
    futex_wait as mem_futex_wait, futex_wake as mem_futex_wake, futex_wake_n as mem_futex_wake_n,
    FutexWaitResult, FutexWaiter, WakeFn, FUTEX_STATS,
};

// ─────────────────────────────────────────────────────────────────────────────
// FutexKey — key identifying an IPC futex by virtual address (physmap)
// ─────────────────────────────────────────────────────────────────────────────

/// Key of an IPC futex = virtual address of the shared AtomicU32.
/// For an AtomicU32 in an SHM page, the physmap virtual address
/// is shared across all address spaces (same physmap).
/// Compatible with memory::utils::futex_table (virt_addr as key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct FutexKey(pub u64);

impl FutexKey {
    /// Constructs a key from a reference to an AtomicU32.
    #[inline(always)]
    pub fn from_addr(addr: &AtomicU32) -> Self {
        Self(addr as *const _ as u64)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WaiterState — result of a futex_wait
// ─────────────────────────────────────────────────────────────────────────────

/// Wakeup cause of an IPC waiter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WaiterState {
    /// Woken normally by futex_wake().
    Woken = 0,
    /// *addr != expected — no blocking (immediate return).
    ValueMismatch = 1,
    /// Timeout or cancellation (futex_cancel).
    Cancelled = 2,
}

// ─────────────────────────────────────────────────────────────────────────────
// FutexIpcStats — counter snapshot from memory/utils/FUTEX_STATS
// ─────────────────────────────────────────────────────────────────────────────

/// IPC futex statistics — mirror of memory::utils::futex_table::FUTEX_STATS.
#[derive(Debug, Clone, Copy)]
pub struct FutexIpcStats {
    /// Total WAIT calls since boot.
    pub waits_total: u64,
    /// Total WAKE calls since boot.
    pub wakes_total: u64,
    /// Total timeouts (futex_cancel following exceeded spin_max).
    pub timeouts_total: u64,
    /// Total WAITs where *addr != expected (immediate return).
    pub value_mismatches: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// IPC wakeup function — delegates to sched_hooks::wake_thread (scheduler)
// ─────────────────────────────────────────────────────────────────────────────

/// WakeFn injected into FutexWaiter: actually wakes the thread via scheduler.
/// Signature: fn(tid: u64, code: i32) — compatible with memory::utils::futex_table.
fn ipc_futex_wake_fn(tid: u64, _code: i32) {
    super::sched_hooks::wake_thread(tid);
}

/// Wakeup no-op (before hook installation or for callers without scheduler).
#[allow(dead_code)]
fn nop_wake_fn(_tid: u64, _code: i32) {}

// ─────────────────────────────────────────────────────────────────────────────
// Public API — SHIM to memory::utils::futex_table
// ─────────────────────────────────────────────────────────────────────────────

/// IPC WAIT: if *addr == expected, enqueues this thread into global table
/// memory::utils::futex_table then spin-polls until wakeup or timeout.
///
/// DELEGATES TO: memory::utils::futex_table::futex_wait() (enqueueing)
///               then spin-poll on waiter.woken (global waiter AtomicBool)
///
/// # Parameters
/// - `addr`      : reference to shared AtomicU32 in SHM page.
/// - `key`       : FutexKey = virtual address of `addr` (physmap).
/// - `expected`  : expected value in `*addr`.
/// - `thread_id` : thread TID (for targeted wake correlation).
/// - `spin_max`  : max spin iterations before timeout (0 = infinite).
/// - `wake_fn`   : injected scheduler callback; `None` -> nop_wake_fn.
///
/// # Safety
/// - `addr` must point to a valid atomic u32 in an SHM page.
/// - Lifetime of stack-allocated waiter is guaranteed by the
///   spin loop: the function does not return as long as the waiter is active.
pub unsafe fn futex_wait(
    _addr: &AtomicU32,
    key: FutexKey,
    expected: u32,
    thread_id: u64,
    spin_max: u64,
    wake_fn: Option<WakeFn>,
) -> Result<WaiterState, IpcError> {
    // Use ipc_futex_wake_fn by default: actually wakes the thread.
    // If caller provides an explicit wake_fn, use it instead.
    let wfn = wake_fn.unwrap_or(ipc_futex_wake_fn);

    // Allocate FutexWaiter on stack — its lifetime = wait duration.
    let mut waiter = FutexWaiter::new(key.0, expected, thread_id, wfn);
    let wptr = &mut waiter as *mut FutexWaiter;
    struct WaiterGuard {
        ptr: *mut FutexWaiter,
        active: bool,
    }

    impl WaiterGuard {
        fn disarm(&mut self) {
            self.active = false;
        }
    }

    impl Drop for WaiterGuard {
        fn drop(&mut self) {
            if self.active {
                unsafe {
                    if !(*self.ptr).woken.load(Ordering::Acquire) {
                        mem_futex_cancel(self.ptr);
                    }
                }
            }
        }
    }

    // Delegate enqueueing to memory::utils::futex_table (RULE IPC-02).
    let result = mem_futex_wait(key.0, expected, wptr, wfn);

    match result {
        FutexWaitResult::ValueMismatch => Ok(WaiterState::ValueMismatch),

        FutexWaitResult::Waiting => {
            let mut guard = WaiterGuard {
                ptr: wptr,
                active: true,
            };
            // Waiter is in memory/utils/futex_table bucket.
            // Strategy: short spin then real blocking via sched_hooks.
            const SPIN_BEFORE_BLOCK: u64 = 64;
            let mut phase_spins: u64 = 0;
            let mut total_spins: u64 = 0;

            loop {
                core::hint::spin_loop();
                phase_spins += 1;
                total_spins += 1;

                if waiter.woken.load(Ordering::Acquire) {
                    guard.disarm();
                    return Ok(WaiterState::Woken);
                }

                // Explicit timeout (non-zero spin_max) — T8-013 FIX: cumulative over total_spins
                if spin_max != 0 && total_spins >= spin_max {
                    mem_futex_cancel(wptr);
                    guard.disarm();
                    return Err(IpcError::Timeout);
                }

                // After short spin phase: block via scheduler.
                if phase_spins >= SPIN_BEFORE_BLOCK {
                    // Check again before blocking (avoids missed wakeup).
                    if waiter.woken.load(Ordering::Acquire) {
                        guard.disarm();
                        return Ok(WaiterState::Woken);
                    }
                    // Real blocking — returns after ipc_futex_wake_fn has been called.
                    // RULE PREEMPT-BLOCK (B6): blocking with active PreemptGuard = guaranteed deadlock.
                    debug_assert!(
                        crate::scheduler::core::preempt::PreemptGuard::depth() == 0,
                        "futex_wait: block_current() appelé avec PreemptGuard actif — deadlock garanti"
                    );
                    super::sched_hooks::block_current(thread_id);
                    // Reset spin of current phase only.
                    phase_spins = 0;
                }
            }
        }
    }
}

/// IPC WAKE: wakes up to `n` threads waiting on `key`.
///
/// DELEGATES TO: memory::utils::futex_table::futex_wake()
///
/// Returns the number of threads actually woken.
///
/// # Safety
/// - `key` must correspond to valid waiters in the global table.
pub unsafe fn futex_wake(key: FutexKey, n: u32) -> u32 {
    mem_futex_wake(key.0, n, 0)
}

/// IPC WAKE ALL: wakes all threads on `key`.
///
/// DELEGATES TO: memory::utils::futex_table::futex_wake_n()
pub unsafe fn futex_wake_all(key: FutexKey) -> u32 {
    mem_futex_wake_n(key.0, u32::MAX)
}

/// IPC CANCEL: cancels an enrolled waiter (timeout, channel closure).
///
/// DELEGATES TO: memory::utils::futex_table::futex_cancel()
///
/// # Safety: `waiter` must be in the global table.
pub unsafe fn futex_cancel(waiter: *mut FutexWaiter) {
    mem_futex_cancel(waiter);
}

/// IPC REQUEUE: wakes `max_wake` threads on `src`, requeues `max_requeue`
/// to `dst`. Useful for condvars (pthread_cond_broadcast IPC).
///
/// DELEGATES TO: memory::utils::futex_table::futex_requeue()
pub unsafe fn futex_requeue(
    src: FutexKey,
    dst: FutexKey,
    max_wake: u32,
    max_requeue: u32,
) -> (u32, u32) {
    mem_futex_requeue(src.0, dst.0, max_wake, max_requeue, 0)
}

/// Returns a snapshot of statistics from memory::utils::FUTEX_STATS.
/// The source of truth is the memory table — no local duplicate.
pub fn futex_stats() -> FutexIpcStats {
    FutexIpcStats {
        waits_total: FUTEX_STATS.wait_calls.load(Ordering::Relaxed),
        wakes_total: FUTEX_STATS.wake_calls.load(Ordering::Relaxed),
        timeouts_total: FUTEX_STATS.timeouts.load(Ordering::Relaxed),
        value_mismatches: FUTEX_STATS.value_mismatches.load(Ordering::Relaxed),
    }
}
