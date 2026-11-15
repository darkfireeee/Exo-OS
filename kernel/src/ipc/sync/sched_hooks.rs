// kernel/src/ipc/sync/sched_hooks.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SCHED HOOKS — Scheduler injection points for IPC synchronization
// (Exo-OS · IPC Layer 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// This module centralizes scheduler -> IPC integration for actual thread
// blocking. It replaces omnipresent spin-polling in IPC primitives
// (futex, wait_queue, synchronous channel, event, barrier).
//
// ARCHITECTURE:
//   BLOCK_HOOK   : fn() injected by scheduler/ — blocks current thread
//   wake_thread  : calls scheduler/ directly (ipc/ may import it)
//   SLEEP_REGISTRY : tid -> *TCB table to look up TCBs waiting on IPC
//
// RULE: install_block_hook() is called from the kernel init sequence
//       (after scheduler::init), not from the scheduler itself.
//
// LAYER: ipc/ depends on scheduler/ (authorized — see DOC5).
// ═══════════════════════════════════════════════════════════════════════════════

use core::ptr::NonNull;

use crate::scheduler::core::preempt::IrqGuard;
use crate::scheduler::core::preempt::MAX_CPUS;
use crate::scheduler::core::runqueue::run_queue;
use crate::scheduler::core::switch::current_thread_raw;
use crate::scheduler::core::task::{TaskState, ThreadControlBlock};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Blocking hook — provided by scheduler at boot
// ─────────────────────────────────────────────────────────────────────────────

/// Hook type: suspends the current thread, returns after wakeup.
pub type BlockFn = unsafe fn();

/// Hook injected by scheduler::init(). None = spin-poll (before init).
static BLOCK_HOOK: SpinLock<Option<BlockFn>> = SpinLock::new(None);

// ─────────────────────────────────────────────────────────────────────────────
// Registry of sleeping IPC threads (tid -> *TCB)
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of IPC threads that can sleep simultaneously.
pub const MAX_SLEEPING_IPC: usize = 128;

const ENTRY_FREE: usize = 0;
const ENTRY_PREPARING: usize = 1;
const ENTRY_WOKEN_EARLY: usize = usize::MAX;

#[repr(C)]
struct SleepEntry {
    tid: u64,
    tcb_ptr: usize, // ENTRY_FREE / ENTRY_PREPARING / ENTRY_WOKEN_EARLY / *mut ThreadControlBlock
}

impl SleepEntry {
    const fn empty() -> Self {
        Self { tid: 0, tcb_ptr: ENTRY_FREE }
    }
    fn is_free(&self) -> bool {
        self.tcb_ptr == ENTRY_FREE
    }
    fn is_pending_wake(&self) -> bool {
        self.tcb_ptr == ENTRY_WOKEN_EARLY
    }
}

struct SleepRegistry {
    entries: [SleepEntry; MAX_SLEEPING_IPC],
}

enum RegisterResult {
    Registered,
    AlreadyWoken,
    Full,
}

// SAFETY: access protected by SpinLock.
unsafe impl Send for SleepRegistry {}

impl SleepRegistry {
    const fn new() -> Self {
        const E: SleepEntry = SleepEntry::empty();
        Self {
            entries: [E; MAX_SLEEPING_IPC],
        }
    }

    /// Marks `tid` as preparing to sleep before re-checking its wait condition.
    fn prepare(&mut self, tid: u64) {
        let mut free_slot: Option<usize> = None;
        for (i, e) in self.entries.iter_mut().enumerate() {
            if e.tid == tid && !e.is_free() {
                e.tcb_ptr = ENTRY_PREPARING;
                return;
            }
            if free_slot.is_none() && e.is_free() {
                free_slot = Some(i);
            }
        }
        if let Some(i) = free_slot {
            self.entries[i].tid = tid;
            self.entries[i].tcb_ptr = ENTRY_PREPARING;
        }
    }

    /// Removes any prepared/sleeping/early-wake entry for `tid`.
    fn clear_tid(&mut self, tid: u64) {
        for e in self.entries.iter_mut() {
            if e.tid == tid && !e.is_free() {
                *e = SleepEntry::empty();
            }
        }
    }

    /// Registers `(tid, tcb)` and atomically transitions `tcb` to `TaskState::Sleeping`,
    /// or consumes an early wakeup recorded during the prepare window.
    fn commit_sleep(&mut self, tid: u64, tcb: *mut ThreadControlBlock) -> RegisterResult {
        let mut free_slot: Option<usize> = None;
        for (i, e) in self.entries.iter_mut().enumerate() {
            if e.tid == tid && !e.is_free() {
                if e.is_pending_wake() {
                    *e = SleepEntry::empty();
                    return RegisterResult::AlreadyWoken;
                }
                // ENTRY_PREPARING (or already registered) -> commit Sleeping under lock.
                // SAFETY: caller validated non-null `tcb` pointing to static TCB pool.
                unsafe { (*tcb).set_state(TaskState::Sleeping) };
                e.tcb_ptr = tcb as usize;
                return RegisterResult::Registered;
            }
            if free_slot.is_none() && e.is_free() {
                free_slot = Some(i);
            }
        }
        if let Some(i) = free_slot {
            // SAFETY: caller validated non-null `tcb` pointing to static TCB pool.
            unsafe { (*tcb).set_state(TaskState::Sleeping) };
            self.entries[i].tid = tid;
            self.entries[i].tcb_ptr = tcb as usize;
            return RegisterResult::Registered;
        }
        RegisterResult::Full
    }

    /// Pops sleeping TCB for `tid`, or converts `ENTRY_PREPARING` into `ENTRY_WOKEN_EARLY`.
    /// Does NOT create orphan pending-wake records when `tid` has not prepared to sleep.
    fn wake(&mut self, tid: u64) -> *mut ThreadControlBlock {
        for e in self.entries.iter_mut() {
            if e.tid == tid && !e.is_free() {
                if e.tcb_ptr == ENTRY_PREPARING {
                    e.tcb_ptr = ENTRY_WOKEN_EARLY;
                    return core::ptr::null_mut();
                }
                if e.is_pending_wake() {
                    return core::ptr::null_mut();
                }
                let ptr = e.tcb_ptr as *mut ThreadControlBlock;
                *e = SleepEntry::empty();
                return ptr;
            }
        }
        core::ptr::null_mut()
    }
}

static SLEEP_REGISTRY: SpinLock<SleepRegistry> = SpinLock::new(SleepRegistry::new());

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Installs the blocking hook from the scheduler.
///
/// Called ONCE during the kernel initialization sequence,
/// after `scheduler::init()`, before any blocking IPC usage.
pub fn install_block_hook(block: BlockFn) {
    *BLOCK_HOOK.lock() = Some(block);
}

/// Returns `true` if the blocking hook is installed.
#[inline]
pub fn hooks_installed() -> bool {
    BLOCK_HOOK.lock().is_some()
}

/// Returns the ThreadId of the current thread (0 if unknown).
///
/// Used by synchronous channels to record their TID before blocking.
#[inline]
pub fn current_tid() -> u64 {
    let tcb_ptr = current_thread_raw();
    if tcb_ptr.is_null() {
        return 0;
    }
    // SAFETY: tcb_ptr non-null (checked above); points into static pool of TCBs.
    unsafe { (*tcb_ptr).tid }
}

/// Prepares `tid` to block before its final condition check.
#[inline]
pub fn prepare_sleep(tid: u64) {
    {
        let _irq = IrqGuard::new();
        SLEEP_REGISTRY.lock().prepare(tid);
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

/// Cancels a prepared sleep when the wait condition resolved before blocking.
#[inline]
pub fn cancel_sleep(tid: u64) {
    let _irq = IrqGuard::new();
    SLEEP_REGISTRY.lock().clear_tid(tid);
}

/// Blocks the current thread identified by `tid`.
///
/// Registers the `(tid, tcb)` pair in `SLEEP_REGISTRY` and marks `Sleeping`
/// atomically under the registry lock BEFORE invoking the scheduler block hook.
///
/// # Safety
/// - The thread must be in an IPC wait queue (active IpcWaiter, futex waiter, etc.)
/// - The caller should pair `prepare_sleep(tid)` + condition check before calling
///   `block_current(tid)`, or `cancel_sleep(tid)` on early exit.
pub unsafe fn block_current(tid: u64) {
    let tcb_ptr = current_thread_raw();

    if !tcb_ptr.is_null() {
        let registered = {
            let _irq = IrqGuard::new();
            SLEEP_REGISTRY.lock().commit_sleep(tid, tcb_ptr)
        };
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        match registered {
            RegisterResult::Registered => {}
            RegisterResult::AlreadyWoken => {
                // Early wake consumed; ensure thread stays Runnable/Running without enqueueing.
                return;
            }
            RegisterResult::Full => {
                for _ in 0..10_000 {
                    core::hint::spin_loop();
                }
                return;
            }
        }
    }

    // Copy blocking function and release BLOCK_HOOK lock BEFORE switching out.
    let block_fn_opt = {
        let guard = BLOCK_HOOK.lock();
        *guard
    };

    if let Some(block_fn) = block_fn_opt {
        block_fn();
        cancel_sleep(tid);
    } else {
        // Before scheduler init: short spin (~10 us at 3 GHz).
        for _ in 0..10_000 {
            core::hint::spin_loop();
        }
        if !tcb_ptr.is_null() {
            cancel_sleep(tid);
            (*tcb_ptr).set_state(TaskState::Running);
        }
    }
}

/// Wakes the thread identified by `tid`.
///
/// Finds TCB via `SLEEP_REGISTRY`, attempts `TaskState::Sleeping -> Runnable`
/// transition, and enqueues it into the run queue of its CPU.
/// Only marks an early wakeup if the thread is currently in `prepare_sleep`.
pub fn wake_thread(tid: u64) {
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    let tcb_ptr = {
        let _irq = IrqGuard::new();
        SLEEP_REGISTRY.lock().wake(tid)
    };

    if tcb_ptr.is_null() {
        return;
    }
    // SAFETY: tcb_ptr non-null (tested above); TCB in static pool, not freed during execution.
    unsafe {
        let tcb = &mut *tcb_ptr;
        let cpu_id = tcb.current_cpu();
        if (cpu_id.0 as usize) < MAX_CPUS {
            let _irq = IrqGuard::new();
            let mut rq = run_queue(cpu_id);
            // CAS Sleeping -> Runnable protects against double wakeups.
            if tcb.try_transition(TaskState::Sleeping, TaskState::Runnable) {
                // SAFETY: tcb_ptr is valid (lives in static pool of TCBs).
                rq.enqueue(NonNull::new_unchecked(tcb_ptr));
            }
        }
    }
}
