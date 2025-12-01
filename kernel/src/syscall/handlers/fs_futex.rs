//! Futex System Calls
//!
//! Implements the Fast Userspace Mutex (futex) system call.
//! This is critical for threading support in musl libc.

use crate::scheduler::SCHEDULER;
use crate::sync::WaitQueue;
use crate::syscall::handlers::time::TimeSpec;
use crate::time::Duration;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use lazy_static::lazy_static;
use spin::Mutex;

// Global futex registry
// Address -> WaitQueue
lazy_static! {
    static ref FUTEX_QUEUES: Mutex<BTreeMap<usize, Arc<WaitQueue>>> = Mutex::new(BTreeMap::new());
}

// Futex operations
pub const FUTEX_WAIT: i32 = 0;
pub const FUTEX_WAKE: i32 = 1;
pub const FUTEX_FD: i32 = 2;
pub const FUTEX_REQUEUE: i32 = 3;
pub const FUTEX_CMP_REQUEUE: i32 = 4;
pub const FUTEX_WAKE_OP: i32 = 5;
pub const FUTEX_LOCK_PI: i32 = 6;
pub const FUTEX_UNLOCK_PI: i32 = 7;
pub const FUTEX_TRYLOCK_PI: i32 = 8;
pub const FUTEX_WAIT_BITSET: i32 = 9;

pub const FUTEX_PRIVATE_FLAG: i32 = 128;
pub const FUTEX_CLOCK_REALTIME: i32 = 256;

/// sys_futex - Fast Userspace Mutex
///
/// uaddr: Address of the futex word
/// futex_op: Operation to perform
/// val: Value to compare against (for WAIT)
/// timeout: Timeout for WAIT (or val2 for other ops)
/// uaddr2: Second address (for REQUEUE)
/// val3: Third value (for REQUEUE)
pub fn sys_futex(
    uaddr: *mut u32,
    futex_op: i32,
    val: u32,
    timeout: *const TimeSpec,
    _uaddr2: *mut u32,
    _val3: u32,
) -> i32 {
    let cmd = futex_op & !FUTEX_PRIVATE_FLAG;

    match cmd {
        FUTEX_WAIT => {
            if uaddr.is_null() {
                return -14; // EFAULT
            }

            // Read current value atomically
            // In a real implementation, we need to be careful about user memory access
            // For now, we assume direct access is okay in this kernel model
            let current_val = unsafe { core::ptr::read_volatile(uaddr) };

            if current_val != val {
                return -11; // EAGAIN (EWOULDBLOCK)
            }

            // Get or create wait queue
            let addr = uaddr as usize;
            let q = {
                let mut queues = (*FUTEX_QUEUES).lock();
                queues
                    .entry(addr)
                    .or_insert_with(|| Arc::new(WaitQueue::new()))
                    .clone()
            };

            // Calculate timeout if provided
            let duration = if !timeout.is_null() {
                let ts = unsafe { *timeout };
                Some(
                    Duration::from_secs(ts.seconds as u64)
                        + Duration::from_ns(ts.nanoseconds as u64),
                )
            } else {
                None
            };

            // Wait
            if let Some(d) = duration {
                if !q.wait_timeout(d) {
                    return -110; // ETIMEDOUT
                }
            } else {
                q.wait();
            }

            // Return 0 (success) or -4 (EINTR)
            0
        }
        FUTEX_WAKE => {
            if uaddr.is_null() {
                return -14; // EFAULT
            }

            let addr = uaddr as usize;
            let q_opt = {
                let queues = (*FUTEX_QUEUES).lock();
                queues.get(&addr).cloned()
            };

            if let Some(q) = q_opt {
                if val == 1 {
                    q.notify_one();
                    1
                } else {
                    // Wake all (or val number of threads)
                    // WaitQueue only has notify_all, so we use that for > 1
                    q.notify_all();
                    val as i32 // Approximate
                }
            } else {
                0
            }
        }
        _ => {
            // log::warn!("sys_futex: unimplemented op {}", cmd);
            -38 // ENOSYS
        }
    }
}

/// sys_set_tid_address - Set pointer to thread ID
///
/// This is used by the threading library to detect thread death.
/// The kernel writes 0 to this address when the thread exits, then wakes the futex at this address.
pub fn sys_set_tid_address(tidptr: *mut i32) -> i32 {
    SCHEDULER
        .with_current_thread(|thread| {
            // TODO: Store tidptr in thread struct
            // thread.set_clear_child_tid(tidptr);

            // Return current TID
            thread.id() as i32
        })
        .unwrap_or(-3) // ESRCH
}

/// sys_set_robust_list - Set list of robust futexes
pub fn sys_set_robust_list(head: *mut u8, len: usize) -> i32 {
    if len != 24 {
        // sizeof(struct robust_list_head)
        return -22; // EINVAL
    }

    SCHEDULER
        .with_current_thread(|_thread| {
            // TODO: Store robust list head in thread struct
            // thread.set_robust_list(head);
            0
        })
        .unwrap_or(-3)
}
