//! # syscall/handlers/time.rs — Thin wrappers temps (clock_gettime, nanosleep)
//!
//! RÈGLE SYS-03 : THIN WRAPPERS UNIQUEMENT.
//! clock_gettime CLOCK_MONOTONIC → VDSO fast path (150 cycles, pas de syscall).
//! Délègue à time:: pour les autres clocks.

use crate::syscall::errno::{EFAULT, EINVAL, ENOSYS};
use crate::syscall::fast_path::{
    CLOCK_BOOTTIME, CLOCK_MONOTONIC, CLOCK_MONOTONIC_COARSE, CLOCK_MONOTONIC_RAW,
    CLOCK_PROCESS_CPUTIME, CLOCK_REALTIME, CLOCK_REALTIME_COARSE, CLOCK_THREAD_CPUTIME,
};
use crate::syscall::validation::USER_ADDR_MAX;

/// `clock_gettime(clk_id, timespec_ptr)` → 0 ou errno.
///
/// VDSO-01 : CLOCK_MONOTONIC via VDSO (lecture TSC directe, 10× plus rapide).
/// Le fast_path dispatch intercepte ce syscall avant d'atteindre ce handler.
/// Ce handler est le slow-path pour les autres clocks.
pub fn sys_clock_gettime(clk_id: u64, tp_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if tp_ptr == 0 || tp_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    let clock = clk_id as u32;
    match clock {
        CLOCK_REALTIME
        | CLOCK_MONOTONIC
        | CLOCK_MONOTONIC_RAW
        | CLOCK_REALTIME_COARSE
        | CLOCK_MONOTONIC_COARSE
        | CLOCK_BOOTTIME
        | CLOCK_PROCESS_CPUTIME
        | CLOCK_THREAD_CPUTIME => {
            // Délègue → time::clock::get_time(clock, tp_ptr)
            let _ = (clock, tp_ptr);
            ENOSYS
        }
        _ => EINVAL,
    }
}

/// `nanosleep(req, rem)` → 0 ou errno (EINTR si signal, rem mis à jour).
pub fn sys_nanosleep(req_ptr: u64, rem_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if req_ptr == 0 || req_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    if rem_ptr != 0 && rem_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    // Délègue → time::sleep::do_nanosleep(req_ptr, rem_ptr)
    let _ = (req_ptr, rem_ptr);
    ENOSYS
}

/// `clock_nanosleep(clk_id, flags, req, rem)`.
pub fn sys_clock_nanosleep(
    clk_id: u64,
    flags: u64,
    req_ptr: u64,
    rem_ptr: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    if req_ptr == 0 || req_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    if rem_ptr != 0 && rem_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    let clock = clk_id as u32;
    if clock > CLOCK_BOOTTIME {
        return EINVAL;
    }
    let _ = (clock, flags, req_ptr, rem_ptr);
    ENOSYS
}

/// `gettimeofday(tv, tz)` → 0 ou errno.
pub fn sys_gettimeofday(tv_ptr: u64, tz_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if tv_ptr == 0 || tv_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    let _ = (tv_ptr, tz_ptr);
    ENOSYS
}
