// kernel/src/process/thread/pthread_compat.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// POSIX pthread interface — kernel wrappers (Exo-OS Layer 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// This module exports the "pthread_*" functions that are called from
// the syscall layer (syscall/thread.rs) via Linux syscall numbers:
//   sys_clone   -> PTHREAD_CREATE
//   sys_exit    -> PTHREAD_EXIT
//   sys_futex   -> PTHREAD_MTX impl (mutex/condvar)
//

#![allow(non_snake_case)]
// RETURN CODES:
//   0         = success
//   EAGAIN    = resources temporarily unavailable
//   EPERM     = permission denied
//   EINVAL    = invalid argument
//   EDEADLK   = deadlock detected
// ═══════════════════════════════════════════════════════════════════════════════

use crate::process::core::pcb::ProcessControlBlock;
use crate::process::core::tcb::ProcessThread;
use crate::process::thread::creation::{create_thread, ThreadAttr, ThreadCreateParams};
use crate::process::thread::detach::thread_detach;
use crate::process::thread::join::thread_join;
use crate::scheduler::sync::wait_queue::WaitQueue;
use core::sync::atomic::{AtomicU32, Ordering};

/// Global wait queue for pthread mutexes (slow path).
static MUTEX_WQ: WaitQueue = WaitQueue::new();

// ─────────────────────────────────────────────────────────────────────────────
// POSIX error codes (subset)
// ─────────────────────────────────────────────────────────────────────────────

pub const ESUCCESS: i32 = 0;
pub const EAGAIN: i32 = 11;
pub const ENOMEM: i32 = 12;
pub const EBUSY: i32 = 16;
pub const EINVAL: i32 = 22;
pub const EDEADLK: i32 = 35;
pub const ETIMEDOUT: i32 = 110;

// ─────────────────────────────────────────────────────────────────────────────
// PTHREAD_CREATE — syscall clone() wrapper
// ─────────────────────────────────────────────────────────────────────────────

/// Kernel implementation of pthread_create().
///
/// # Arguments
/// `pcb`          — calling process PCB.
/// `start_func`   — userspace entry point address.
/// `arg`          — argument passed to the thread.
/// `stack_ptr`    — stack address supplied by pthread_create() (userspace).
/// `stack_size`   — stack size in bytes.
/// `pthread_out`  — userspace address of output pthread_t struct.
/// `target_cpu`   — target CPU (pinned to current CPU by default).
///
/// # Returns
/// 0 on success, or negative errno code.
///
/// # Safety
/// `pcb` must point to the calling thread's PCB.
pub fn PTHREAD_CREATE(
    pcb: *const ProcessControlBlock,
    start_func: u64,
    arg: u64,
    stack_ptr: u64,
    stack_size: u64,
    pthread_out: u64,
    target_cpu: u32,
) -> i32 {
    let params = ThreadCreateParams {
        pcb,
        attr: ThreadAttr {
            stack_size,
            stack_addr: stack_ptr,
            policy: crate::scheduler::core::task::SchedPolicy::Normal,
            priority: crate::scheduler::core::task::Priority::NORMAL_DEFAULT,
            detached: false,
            cpu_affinity: -1,
            sigaltstack_size: 0,
        },
        start_func,
        arg,
        initial_rsp: 0,
        tls_base: 0,
        target_cpu,
        pthread_out,
    };
    match create_thread(&params) {
        Ok(handle) => {
            // Write TID into pthread_t struct (offset 0).
            if pthread_out != 0 {
                // SAFETY: pthread_out = userspace address validated by syscall prior to this call.
                unsafe {
                    let p = pthread_out as *mut u32;
                    *p = handle.tid.0;
                }
            }
            ESUCCESS
        }
        Err(e) => match e {
            crate::process::thread::creation::ThreadCreateError::TidExhausted => -EAGAIN,
            crate::process::thread::creation::ThreadCreateError::OutOfMemory => -ENOMEM,
            crate::process::thread::creation::ThreadCreateError::ProcessExiting => -EINVAL,
            _ => -EINVAL,
        },
    }
}

/// Kernel implementation of pthread_join().
///
/// # Safety
/// `thread_ptr` must point to a valid ProcessThread.
/// `caller_tcb` must be the calling thread's TCB.
/// `retval_out` must be writable or null.
pub unsafe fn PTHREAD_JOIN(
    thread_ptr: *const ProcessThread,
    caller_tcb: &crate::scheduler::core::task::ThreadControlBlock,
    retval_out: *mut u64,
) -> i32 {
    match thread_join(thread_ptr, caller_tcb) {
        Ok(val) => {
            if !retval_out.is_null() {
                // SAFETY: retval_out validated by syscall.
                unsafe {
                    *retval_out = val;
                }
            }
            ESUCCESS
        }
        Err(e) => match e {
            crate::process::thread::join::JoinError::Detached => -EINVAL,
            crate::process::thread::join::JoinError::Interrupted => -4, // EINTR
            _ => -EINVAL,
        },
    }
}

/// Kernel implementation of pthread_detach().
///
/// # Safety
/// `thread_ptr` must point to a valid ProcessThread.
pub unsafe fn PTHREAD_DETACH(thread_ptr: *mut ProcessThread) -> i32 {
    match thread_detach(thread_ptr) {
        Ok(_) => ESUCCESS,
        Err(_) => -EINVAL,
    }
}

/// Returns the TID of the current thread (pthread_self).
/// Called via GS:[0] in userspace; implemented here for the kernel bridge.
pub fn PTHREAD_SELF(thread: &ProcessThread) -> u64 {
    thread.tid.0 as u64
}

/// Kernel implementation of pthread_exit() — calls do_exit_thread.
/// NEVER returns.
///
/// # Safety
/// `thread` and `pcb` must correspond to the current thread.
pub fn PTHREAD_EXIT(thread: &mut ProcessThread, pcb: &ProcessControlBlock, retval: u64) -> ! {
    crate::process::lifecycle::exit::do_exit_thread(thread, pcb, retval)
}

// ─────────────────────────────────────────────────────────────────────────────
// PthreadMutex — simple kernel mutex (spinlock + wait_queue)
// ───────────────────────────────────────────────────────────────────────────══

/// Shared kernel/userspace mutex structure.
/// Placed in userspace memory space, accessed via futex syscalls.
#[repr(C, align(16))]
pub struct PthreadMutex {
    /// 0 = free, 1 = acquired, 2 = acquired with waiting threads.
    pub state: AtomicU32,
    /// Current owner TID (debugging).
    pub owner: AtomicU32,
    /// Number of waiting threads.
    pub waiters: AtomicU32,
    /// Type: 0=NORMAL, 1=ERRORCHECK, 2=RECURSIVE.
    pub mtype: u32,
    /// Recursive lock count (RECURSIVE type only).
    pub recursive_count: AtomicU32,
    _pad: [u32; 3],
}

const _: () = assert!(
    core::mem::size_of::<PthreadMutex>() == 32,
    "PthreadMutex must be 32B"
);

/// Initializes a PthreadMutex.
///
/// # Safety
/// `mutex_ptr` = userspace address of a PthreadMutex validated by syscall.
pub unsafe fn PTHREAD_MUTEX_INIT(mutex_ptr: *mut PthreadMutex, mtype: u32) -> i32 {
    if mutex_ptr.is_null() {
        return -EINVAL;
    }
    // SAFETY: mutex_ptr validated by syscall via userspace check.
    (*mutex_ptr).state.store(0, Ordering::Release);
    (*mutex_ptr).owner.store(0, Ordering::Relaxed);
    (*mutex_ptr).waiters.store(0, Ordering::Relaxed);
    (*(mutex_ptr as *mut u32).add(3)) = mtype;
    (*mutex_ptr).recursive_count.store(0, Ordering::Relaxed);
    ESUCCESS
}

/// Locks a PthreadMutex (blocking if already held).
///
/// # Safety
/// `mutex_ptr` = userspace address of validated PthreadMutex, `owner_tid` = calling thread TID.
pub unsafe fn PTHREAD_MUTEX_LOCK(
    mutex_ptr: *mut PthreadMutex,
    owner_tid: u32,
    caller_tcb: &crate::scheduler::core::task::ThreadControlBlock,
) -> i32 {
    if mutex_ptr.is_null() {
        return -EINVAL;
    }
    let m = &*mutex_ptr;
    // Fast path: CAS 0 -> 1.
    if m.state
        .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
    {
        m.owner.store(owner_tid, Ordering::Relaxed);
        return ESUCCESS;
    }
    // ERRORCHECK detection: already owner.
    if m.mtype == 1 && m.owner.load(Ordering::Relaxed) == owner_tid {
        return -EDEADLK;
    }
    // Slow path: wait on futex (simplified impl via wait_queue).
    m.waiters.fetch_add(1, Ordering::Relaxed);
    loop {
        if caller_tcb.has_signal_pending() {
            m.waiters.fetch_sub(1, Ordering::Relaxed);
            return -4; // EINTR
        }
        if m.state
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            m.owner.store(owner_tid, Ordering::Relaxed);
            m.waiters.fetch_sub(1, Ordering::Relaxed);
            return ESUCCESS;
        }
        // SAFETY: MUTEX_WQ EmergencyPool (WAITQ-01); caller_tcb is current TCB, no active &mut alias.
        unsafe {
            MUTEX_WQ.wait_interruptible(caller_tcb as *const _ as *mut _);
        }
    }
}

/// Unlocks a PthreadMutex.
///
/// # Safety
/// `mutex_ptr` validated by syscall.
pub unsafe fn PTHREAD_MUTEX_UNLOCK(mutex_ptr: *mut PthreadMutex, caller_tid: u32) -> i32 {
    if mutex_ptr.is_null() {
        return -EINVAL;
    }
    let m = &*mutex_ptr;
    if m.mtype == 1 && m.owner.load(Ordering::Relaxed) != caller_tid {
        return -EPERM;
    }
    m.owner.store(0, Ordering::Relaxed);
    m.state.store(0, Ordering::Release);
    // Wake a waiter if necessary.
    if m.waiters.load(Ordering::Relaxed) > 0 {
        MUTEX_WQ.notify_one();
    }
    ESUCCESS
}

/// Destroys a PthreadMutex.
pub unsafe fn PTHREAD_MUTEX_DESTROY(mutex_ptr: *mut PthreadMutex) -> i32 {
    if mutex_ptr.is_null() {
        return -EINVAL;
    }
    let m = &*mutex_ptr;
    if m.state.load(Ordering::Relaxed) != 0 {
        return -EBUSY; // Do not destroy a held mutex.
    }
    ESUCCESS
}

const EPERM: i32 = 1;
