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

#[repr(C)]
struct SleepEntry {
    tid: u64,
    tcb_ptr: usize, // *mut ThreadControlBlock encoded as usize
}

impl SleepEntry {
    const fn empty() -> Self {
        Self { tid: 0, tcb_ptr: 0 }
    }
    fn is_free(&self) -> bool {
        self.tcb_ptr == 0
    }
    fn is_pending_wake(&self) -> bool {
        self.tcb_ptr == usize::MAX
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

    /// Registers (tid, tcb), or consumes an early wakeup.
    fn register(&mut self, tid: u64, tcb: *mut ThreadControlBlock) -> RegisterResult {
        for e in self.entries.iter_mut() {
            if e.tid == tid && e.is_pending_wake() {
                *e = SleepEntry::empty();
                return RegisterResult::AlreadyWoken;
            }
        }
        for e in self.entries.iter_mut() {
            if e.is_free() {
                e.tid = tid;
                e.tcb_ptr = tcb as usize;
                return RegisterResult::Registered;
            }
        }
        RegisterResult::Full
    }

    /// Unregisters and returns the TCB for `tid`, or null if absent.
    fn pop(&mut self, tid: u64) -> *mut ThreadControlBlock {
        for e in self.entries.iter_mut() {
            if e.tid == tid && !e.is_free() && !e.is_pending_wake() {
                let ptr = e.tcb_ptr as *mut ThreadControlBlock;
                *e = SleepEntry::empty();
                return ptr;
            }
        }
        core::ptr::null_mut()
    }

    fn mark_pending_wake(&mut self, tid: u64) {
        for e in self.entries.iter_mut() {
            if e.tid == tid && e.is_pending_wake() {
                return;
            }
        }
        for e in self.entries.iter_mut() {
            if e.is_free() {
                e.tid = tid;
                e.tcb_ptr = usize::MAX;
                return;
            }
        }
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

/// Blocks the current thread identified by `tid`.
///
/// Registers the (tid, tcb) pair in SLEEP_REGISTRY BEFORE blocking,
/// allowing `wake_thread(tid)` to find the TCB.
///
/// After wakeup (by `wake_thread`), removes from registry and returns.
/// If BLOCK_HOOK is not installed, performs a short spin (before scheduler init).
///
/// # Safety
/// - The thread must be in an IPC wait queue (active IpcWaiter, futex waiter, etc.)
/// - The caller must check the `woken` flag BEFORE calling this function
///   to avoid blocking if wakeup already occurred.
pub unsafe fn block_current(tid: u64) {
    let tcb_ptr = current_thread_raw();

    if !tcb_ptr.is_null() {
        let tcb = &mut *tcb_ptr;
        tcb.set_state(TaskState::Sleeping);
        let registered = SLEEP_REGISTRY.lock().register(tid, tcb_ptr);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        match registered {
            RegisterResult::Registered => {
                if tcb.state() == TaskState::Runnable {
                    SLEEP_REGISTRY.lock().pop(tid);
                    return;
                }
            }
            RegisterResult::AlreadyWoken => {
                tcb.set_state(TaskState::Runnable);
                return;
            }
            RegisterResult::Full => {
                tcb.set_state(TaskState::Running);
                for _ in 0..10_000 {
                    core::hint::spin_loop();
                }
                return;
            }
        }
    }

    // FIX-BLOCKHOOK (IPC deadlock): copy blocking function and release
    // the BLOCK_HOOK lock BEFORE blocking.
    //
    // Old code `if let Some(block_fn) = *BLOCK_HOOK.lock() { block_fn() }`
    // kept the SpinLockGuard alive during the ENTIRE body of `if let` — thus
    // ACROSS the context switch performed by block_fn(). Consequences:
    //   1. Any other thread calling block_current() (or hooks_installed())
    //      meanwhile spun indefinitely -> deadlock as soon as TWO threads
    //      sleep in IPC (vfs_server + tty_server during shell boot).
    //   2. The PreemptGuard held by the guard violated the contract
    //      `PreemptGuard::depth() == 0` of block_current_thread() (panic in
    //      debug build).
    let block_fn_opt = {
        let guard = BLOCK_HOOK.lock();
        match *guard {
            Some(f) => Some(f),
            None => None,
        }
    }; // ← BLOCK_HOOK guard released here, BEFORE blocking.

    if let Some(block_fn) = block_fn_opt {
        block_fn();
        // After wakeup: unregister if not yet done by wake_thread.
        if !tcb_ptr.is_null() {
            SLEEP_REGISTRY.lock().pop(tid);
        }
    } else {
        // Before scheduler init: short spin (~10 us at 3 GHz).
        for _ in 0..10_000 {
            core::hint::spin_loop();
        }
        if !tcb_ptr.is_null() {
            SLEEP_REGISTRY.lock().pop(tid);
            (*tcb_ptr).set_state(TaskState::Running);
        }
    }
}

/// Wakes the thread identified by `tid`.
///
/// Finds TCB via SLEEP_REGISTRY, attempts TaskState::Sleeping ->
/// Runnable transition, and enqueues it into the run queue of its CPU.
/// No-op if the thread is not registered (early wakeup or called twice).
pub fn wake_thread(tid: u64) {
    let tcb_ptr = {
        let mut reg = SLEEP_REGISTRY.lock();
        reg.pop(tid)
    };

    if tcb_ptr.is_null() {
        let mut reg = SLEEP_REGISTRY.lock();
        reg.mark_pending_wake(tid);
        return;
    }
    // SAFETY: tcb_ptr non-null (tested above); TCB in static pool, not freed during execution.
    unsafe {
        let tcb = &mut *tcb_ptr;
        let cpu_id = tcb.current_cpu();
        if (cpu_id.0 as usize) < MAX_CPUS {
            let _irq = IrqGuard::new();
            let rq = run_queue(cpu_id);
            // CAS Sleeping -> Runnable protects against double wakeups.
            if tcb.try_transition(TaskState::Sleeping, TaskState::Runnable) {
                // SAFETY: tcb_ptr is valid (lives in static pool of TCBs).
                rq.enqueue(NonNull::new_unchecked(tcb_ptr));
            }
        }
    }
}
