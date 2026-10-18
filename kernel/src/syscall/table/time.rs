//! # syscall/table/time.rs — Handlers temps, horloges, nanosleep et futex

use super::common::*;
use crate::syscall::fast_path::Timespec;
use crate::syscall::numbers::*;
use crate::syscall::validation::{copy_to_user, read_user_typed, UserBuf};

/// `gettimeofday(tv, tz)` slow-path wrapper.
pub fn sys_gettimeofday(tv_ptr: u64, tz_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_GETTIMEOFDAY);
    crate::syscall::fast_path::sys_gettimeofday(tv_ptr, tz_ptr)
}


// ─────────────────────────────────────────────────────────────────────────────
// Handlers Scheduler (delay, nanosleep, futex)
// ─────────────────────────────────────────────────────────────────────────────

/// `nanosleep(req_ptr, rem_ptr)` — suspend le thread pendant une durée.
pub fn sys_nanosleep(req_ptr: u64, rem_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_NANOSLEEP);
    if req_ptr == 0 {
        return EFAULT;
    }
    let ts = match read_user_typed::<Timespec>(req_ptr) {
        Ok(t) => t,
        Err(e) => return e.to_errno(),
    };
    if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
        return EINVAL;
    }
    let ns = (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64);
    if !crate::scheduler::timer::sleep_ns(ns) {
        return EINTR;
    }
    let _ = rem_ptr;
    0
}

/// `clock_nanosleep(clockid, flags, req, rem)`.
pub fn sys_clock_nanosleep(
    clk_id: u64,
    flags: u64,
    req_ptr: u64,
    rem_ptr: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_CLOCK_NANOSLEEP);
    const TIMER_ABSTIME: u64 = 1;
    const CLOCK_REALTIME: u64 = 0;
    const CLOCK_MONOTONIC: u64 = 1;
    const CLOCK_BOOTTIME: u64 = 7;
    if flags & !TIMER_ABSTIME != 0 {
        return EINVAL;
    }
    match clk_id {
        CLOCK_REALTIME | CLOCK_MONOTONIC | CLOCK_BOOTTIME => {}
        _ => return EINVAL,
    }
    if flags & TIMER_ABSTIME == 0 {
        return sys_nanosleep(req_ptr, rem_ptr, 0, 0, 0, 0);
    }
    if req_ptr == 0 {
        return EFAULT;
    }
    let ts = match read_user_typed::<Timespec>(req_ptr) {
        Ok(t) => t,
        Err(e) => return e.to_errno(),
    };
    if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
        return EINVAL;
    }
    let target_ns = (ts.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(ts.tv_nsec as u64);
    if !crate::scheduler::timer::sleep_until_ns(target_ns) {
        return EINTR;
    }
    let _ = rem_ptr;
    0
}

/// `futex(uaddr, op, val, timeout, uaddr2, val3)`.
pub fn sys_futex(uaddr: u64, op: u64, val: u64, timeout: u64, uaddr2: u64, val3: u64) -> i64 {
    stat_inc(SYS_FUTEX);
    // futex est dans memory/utils/futex_table.rs (RÈGLE SCHED-03 DOC3).
    match crate::memory::utils::futex_table::sys_futex(
        uaddr,
        op as u32,
        val as u32,
        timeout,
        uaddr2,
        val3 as u32,
    ) {
        Ok(v) => v,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `getrandom(buf, buflen, flags)` — Linux-compatible syscall 318.
pub fn sys_getrandom(buf_ptr: u64, len: u64, flags: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_GETRANDOM);
    const GETRANDOM_MAX: usize = 256 * 1024;
    const GRND_NONBLOCK: u64 = 0x0001;
    const GRND_RANDOM: u64 = 0x0002;
    const GRND_INSECURE: u64 = 0x0004;
    let allowed_flags = GRND_NONBLOCK | GRND_RANDOM | GRND_INSECURE;
    if flags & !allowed_flags != 0 {
        return EINVAL;
    }

    let len = len as usize;
    let _validated = match UserBuf::validate(buf_ptr, len, GETRANDOM_MAX) {
        Ok(buf) => buf,
        Err(e) => return e.to_errno(),
    };
    if len == 0 {
        return 0;
    }

    if !crate::security::crypto::rng_is_ready() {
        crate::security::crypto::rng_init();
    }

    let mut written = 0usize;
    while written < len {
        let mut chunk = [0u8; 64];
        let n = core::cmp::min(chunk.len(), len.saturating_sub(written));
        if crate::security::crypto::rng_fill(&mut chunk[..n]).is_err() {
            return EAGAIN;
        }
        let dst = match buf_ptr.checked_add(written as u64) {
            Some(addr) => addr,
            None => return EFAULT,
        };
        if copy_to_user(dst as *mut u8, chunk.as_ptr(), n).is_err() {
            return EFAULT;
        }
        written = written.saturating_add(n);
    }
    written as i64
}

