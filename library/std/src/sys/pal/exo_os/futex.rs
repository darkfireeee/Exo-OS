//! Futex primitives (`SYS_FUTEX = 202`) for `std::sys::sync` on Exo-OS.

use crate::sync::atomic::Atomic;
use crate::time::Duration;
use super::syscall::{RawTimespec, SysError, futex_wait_raw, futex_wake_raw};

pub type Futex = Atomic<Primitive>;
pub type Primitive = u32;

pub type SmallFutex = Atomic<SmallPrimitive>;
pub type SmallPrimitive = u32;

pub fn futex_wait(futex: &Atomic<u32>, expected: u32, timeout: Option<Duration>) -> bool {
    let ts = timeout.and_then(|dur| {
        Some(RawTimespec {
            tv_sec: dur.as_secs().try_into().ok()?,
            tv_nsec: dur.subsec_nanos() as i64,
        })
    });
    match futex_wait_raw(futex.as_ptr(), expected, ts.as_ref()) {
        Ok(()) => true,
        Err(e) => e != SysError::ETIMEDOUT,
    }
}

#[inline]
pub fn futex_wake(futex: &Atomic<u32>) -> bool {
    futex_wake_raw(futex.as_ptr(), 1).unwrap_or(0) > 0
}

#[inline]
pub fn futex_wake_all(futex: &Atomic<u32>) {
    let _ = futex_wake_raw(futex.as_ptr(), i32::MAX as u32);
}
