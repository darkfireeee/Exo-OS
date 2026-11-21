//! # syscall/table/time.rs — Time, clocks, nanosleep, and futex handlers

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
// Scheduler handlers (delay, nanosleep, futex)
// ─────────────────────────────────────────────────────────────────────────────

/// `nanosleep(req_ptr, rem_ptr)` — suspends the thread for a duration.
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
    // FIX-SYSCALL-NANOSLEEP-01: saturating calculation to prevent overflow
    let ns = (ts.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(ts.tv_nsec as u64);
    let deadline_ns = crate::scheduler::timer::monotonic_ns().saturating_add(ns);
    if !crate::scheduler::timer::sleep_until_ns(deadline_ns) {
        if rem_ptr != 0 {
            let now = crate::scheduler::timer::monotonic_ns();
            let rem_ns = deadline_ns.saturating_sub(now);
            let rem_ts = Timespec {
                tv_sec: (rem_ns / 1_000_000_000) as i64,
                tv_nsec: (rem_ns % 1_000_000_000) as i64,
            };
            if copy_to_user(
                rem_ptr as *mut u8,
                &rem_ts as *const Timespec as *const u8,
                core::mem::size_of::<Timespec>(),
            )
            .is_err()
            {
                return EFAULT;
            }
        }
        return EINTR;
    }
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
    let mono_target_ns = if clk_id == CLOCK_REALTIME {
        target_ns.saturating_sub(crate::scheduler::timer::clock::realtime_offset_ns())
    } else {
        target_ns
    };
    if !crate::scheduler::timer::sleep_until_ns(mono_target_ns) {
        return EINTR;
    }
    let _ = rem_ptr;
    0
}

/// `clock_getres(clockid, res_ptr)` — writes clock resolution (1 ns) when `res_ptr != 0`.
pub fn sys_clock_getres(clk_id: u64, res_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_CLOCK_GETRES);
    if clk_id > 7 {
        return EINVAL;
    }
    if res_ptr != 0 {
        let ts = Timespec {
            tv_sec: 0,
            tv_nsec: 1,
        };
        if copy_to_user(
            res_ptr as *mut u8,
            &ts as *const Timespec as *const u8,
            core::mem::size_of::<Timespec>(),
        )
        .is_err()
        {
            return EFAULT;
        }
    }
    0
}

/// `futex(uaddr, op, val, timeout, uaddr2, val3)`.
pub fn sys_futex(uaddr: u64, op: u64, val: u64, timeout: u64, uaddr2: u64, val3: u64) -> i64 {
    stat_inc(SYS_FUTEX);
    if uaddr == 0
        || !uaddr.is_multiple_of(4)
        || uaddr.checked_add(4).map_or(true, |end| end > crate::syscall::USER_ADDR_MAX)
        || !crate::memory::core::address::is_user_canonical(uaddr)
    {
        return -crate::syscall::errno::EFAULT;
    }
    let op32 = op as u32;
    let cmd = op32 & !(128 | 256);
    if cmd == 0 && (op32 & 256) != 0 {
        return EINVAL;
    }
    let timeout_opt: Option<u64> = if (cmd == 0 || cmd == 9) && timeout != 0 {
        let ts = match read_user_typed::<Timespec>(timeout) {
            Ok(t) => t,
            Err(e) => return e.to_errno(),
        };
        if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
            return EINVAL;
        }
        let raw_ns = (ts.tv_sec as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(ts.tv_nsec as u64);
        if cmd == 9 {
            // FUTEX_WAIT_BITSET uses an absolute deadline (CLOCK_REALTIME if bit 256 set, else CLOCK_MONOTONIC).
            let now_ns = if (op32 & 256) != 0 {
                crate::scheduler::timer::realtime_ns()
            } else {
                crate::scheduler::timer::monotonic_ns()
            };
            Some(raw_ns.saturating_sub(now_ns))
        } else {
            Some(raw_ns)
        }
    } else if cmd == 0 || cmd == 9 {
        None
    } else {
        Some(timeout)
    };

    // futex is in memory/utils/futex_table.rs (RULE SCHED-03 DOC3).
    match crate::memory::utils::futex_table::sys_futex_with_timeout(
        uaddr,
        op32,
        val as u32,
        timeout_opt,
        timeout as u32,
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

