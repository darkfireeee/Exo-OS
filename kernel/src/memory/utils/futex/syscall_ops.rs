//! `sys_futex` syscall entry points, error codes, and scheduler hook injection.

use core::sync::atomic::{AtomicPtr, Ordering};

use super::bucket_table::{
    private_space_id, FutexWaiter, WakeFn, FUTEX_BITSET_MATCH_ANY, SHARED_SPACE_ID,
};
use super::wait_wake::{
    check_expired_wait, futex_cancel, futex_requeue_in_space, futex_wait_in_space_bitset,
    futex_wake_in_space_bitset, FutexWaitResult,
};

const FUTEX_WAIT: u32 = 0;
const FUTEX_WAKE: u32 = 1;
const FUTEX_REQUEUE: u32 = 3;
const FUTEX_CMP_REQUEUE: u32 = 4;
const FUTEX_WAIT_BITSET: u32 = 9;
const FUTEX_WAKE_BITSET: u32 = 10;
const FUTEX_PRIVATE_FLAG: u32 = 128;
const FUTEX_CLOCK_REALTIME: u32 = 256;

/// Possible errors from `sys_futex`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutexError {
    InvalidOp,
    InvalidAddr,
    ValueMismatch,
    Interrupted,
    Timeout,
    NoMemory,
}

impl FutexError {
    pub fn to_kernel_errno(self) -> i64 {
        match self {
            FutexError::InvalidOp => -22,     // EINVAL
            FutexError::InvalidAddr => -14,   // EFAULT
            FutexError::ValueMismatch => -11, // EAGAIN
            FutexError::Interrupted => -4,    // EINTR
            FutexError::Timeout => -110,      // ETIMEDOUT
            FutexError::NoMemory => -12,      // ENOMEM
        }
    }
}

pub type FutexSleepHook = fn(waiter: *mut FutexWaiter, timeout_ns: u64) -> i32;
pub type FutexTidHook = fn() -> u64;

static FUTEX_SLEEP_HOOK: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static FUTEX_WAKE_HOOK: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static FUTEX_TID_HOOK: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

pub fn register_sleep_hook(hook: FutexSleepHook) {
    FUTEX_SLEEP_HOOK.store(hook as *mut (), Ordering::Release);
}

pub fn register_wake_hook(hook: WakeFn) {
    FUTEX_WAKE_HOOK.store(hook as *mut (), Ordering::Release);
}

pub fn register_tid_hook(hook: FutexTidHook) {
    FUTEX_TID_HOOK.store(hook as *mut (), Ordering::Release);
}

fn get_sleep_hook() -> Option<FutexSleepHook> {
    let ptr = FUTEX_SLEEP_HOOK.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { core::mem::transmute::<*mut (), FutexSleepHook>(ptr) })
    }
}

fn get_wake_hook() -> Option<WakeFn> {
    let ptr = FUTEX_WAKE_HOOK.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { core::mem::transmute::<*mut (), WakeFn>(ptr) })
    }
}

fn get_tid_hook() -> Option<FutexTidHook> {
    let ptr = FUTEX_TID_HOOK.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { core::mem::transmute::<*mut (), FutexTidHook>(ptr) })
    }
}

fn nop_wake_sys(_tid: u64, _code: i32) {}

#[inline]
fn resolve_user_futex_key(uaddr: u64, is_private: bool) -> Result<(u64, u64), FutexError> {
    if uaddr == 0
        || !uaddr.is_multiple_of(4)
        || !crate::memory::core::address::is_user_canonical(uaddr)
        || uaddr > crate::syscall::validation::USER_ADDR_MAX - 4
    {
        return Err(FutexError::InvalidAddr);
    }
    let (_, phys) = crate::syscall::validation::resolve_user_read_u32_and_phys(uaddr)
        .map_err(|_| FutexError::InvalidAddr)?;
    if is_private {
        let pid = crate::syscall::validation::current_user_pid().unwrap_or(0);
        Ok((private_space_id(pid), uaddr))
    } else {
        Ok((SHARED_SPACE_ID, phys))
    }
}

/// Entry point of syscall `futex(uaddr, op, val, timeout, uaddr2, val3)` with normalized timeout.
pub fn sys_futex_with_timeout(
    uaddr: u64,
    op: u32,
    val: u32,
    timeout_ns: Option<u64>,
    val2_requeue: u32,
    uaddr2: u64,
    val3: u32,
) -> Result<i64, FutexError> {
    let is_private = (op & FUTEX_PRIVATE_FLAG) != 0;
    let cmd = op & !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);

    match cmd {
        FUTEX_WAIT | FUTEX_WAIT_BITSET => {
            if cmd == FUTEX_WAIT_BITSET && val3 == 0 {
                return Err(FutexError::InvalidOp);
            }
            if cmd == FUTEX_WAIT && (op & FUTEX_CLOCK_REALTIME) != 0 {
                return Err(FutexError::InvalidOp);
            }
            let (space_id, key_addr) = resolve_user_futex_key(uaddr, is_private)?;
            if timeout_ns == Some(0) {
                return Err(check_expired_wait(uaddr, true, val));
            }
            let bitset = if cmd == FUTEX_WAIT_BITSET {
                val3
            } else {
                FUTEX_BITSET_MATCH_ANY
            };
            let tid = get_tid_hook().map(|f| f()).unwrap_or(0);
            let wake_fn = get_wake_hook().unwrap_or(nop_wake_sys);
            let mut waiter = FutexWaiter::new(key_addr, val, tid, wake_fn);

            let result = unsafe {
                futex_wait_in_space_bitset(
                    space_id,
                    key_addr,
                    uaddr,
                    true,
                    val,
                    bitset,
                    &mut waiter,
                    wake_fn,
                )
            }?;

            match result {
                FutexWaitResult::ValueMismatch => Err(FutexError::ValueMismatch),
                FutexWaitResult::Waiting => match get_sleep_hook() {
                    Some(sleep) => {
                        let rc = sleep(&mut waiter, timeout_ns.unwrap_or(0));
                        if rc == -4 {
                            unsafe { futex_cancel(&mut waiter) };
                            Err(FutexError::Interrupted)
                        } else if rc == -110 {
                            unsafe { futex_cancel(&mut waiter) };
                            Err(FutexError::Timeout)
                        } else if !waiter.woken.load(Ordering::Acquire) {
                            unsafe { futex_cancel(&mut waiter) };
                            Err(FutexError::Interrupted)
                        } else {
                            Ok(0)
                        }
                    }
                    None => {
                        unsafe { futex_cancel(&mut waiter) };
                        Err(FutexError::Interrupted)
                    }
                },
            }
        }

        FUTEX_WAKE | FUTEX_WAKE_BITSET => {
            if cmd == FUTEX_WAKE_BITSET && val3 == 0 {
                return Err(FutexError::InvalidOp);
            }
            let bitset = if cmd == FUTEX_WAKE_BITSET {
                val3
            } else {
                FUTEX_BITSET_MATCH_ANY
            };
            let (space_id, key_addr) = resolve_user_futex_key(uaddr, is_private)?;
            let woken = unsafe { futex_wake_in_space_bitset(space_id, key_addr, bitset, val, 0) };
            Ok(woken as i64)
        }

        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => {
            let (src_space, src_key) = resolve_user_futex_key(uaddr, is_private)?;
            let (dst_space, dst_key) = resolve_user_futex_key(uaddr2, is_private)?;
            let cmp_expected = if cmd == FUTEX_CMP_REQUEUE {
                Some((uaddr, val3, true))
            } else {
                None
            };
            let (woken, requeued) = unsafe {
                futex_requeue_in_space(
                    src_space,
                    src_key,
                    dst_space,
                    dst_key,
                    cmp_expected,
                    val,
                    val2_requeue,
                    0,
                )
            }?;
            let total = if cmd == FUTEX_CMP_REQUEUE {
                woken.saturating_add(requeued)
            } else {
                woken
            };
            Ok(total as i64)
        }

        _ => Err(FutexError::InvalidOp),
    }
}

/// Entry point of syscall `futex(uaddr, op, val, timeout, uaddr2, val3)`.
pub fn sys_futex(
    uaddr: u64,
    op: u32,
    val: u32,
    timeout: u64,
    uaddr2: u64,
    val3: u32,
) -> Result<i64, FutexError> {
    let timeout_ns = if timeout == 0 { None } else { Some(timeout) };
    sys_futex_with_timeout(uaddr, op, val, timeout_ns, timeout as u32, uaddr2, val3)
}
