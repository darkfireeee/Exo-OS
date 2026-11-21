//! Weak `_Unwind_*` ABI symbols for `target_os = "exo"`.
//!
//! Enabling `target-family = ["unix"]` makes upstream `std::backtrace_rs`
//! select its `libunwind` backend (`_Unwind_Backtrace`, `_Unwind_GetIP`, ...).
//! `x86_64-unknown-exo.json` sets `"frame-pointer": "always"`, and both
//! `_start` and `thread_start_trampoline` zero `%rbp` at the base of every
//! thread stack, allowing safe, zero-allocation frame-pointer unwinding for
//! `std::backtrace::Backtrace::capture()` / `force_capture()`.
//! Marked `weak` so a full DWARF `.eh_frame` unwinder in `libunwind`/`musl-exo`
//! overrides these cleanly when linked.

#![allow(non_snake_case)]

use core::ffi::{c_int, c_void};

const _URC_NO_REASON: c_int = 0;
const _URC_END_OF_STACK: c_int = 5;

const USER_ADDR_MIN: usize = 0x1000;
const USER_ADDR_MAX: usize = 0x0000_7FFF_FFFF_F000;
const MAX_FRAME_STEP: usize = 4 * 1024 * 1024;
const MAX_UNWIND_FRAMES: usize = 128;

pub type UnwindTraceFn = extern "C" fn(ctx: *mut c_void, arg: *mut c_void) -> c_int;

#[repr(C)]
struct ExoUnwindContext {
    ip: usize,
    cfa: usize,
}

#[linkage = "weak"]
#[inline(never)]
#[unsafe(no_mangle)]
pub extern "C" fn _Unwind_Backtrace(trace: UnwindTraceFn, trace_argument: *mut c_void) -> c_int {
    #[cfg(target_arch = "x86_64")]
    {
        let mut rbp: usize;
        // SAFETY: reading the current frame pointer register `%rbp` has no side effects.
        unsafe {
            core::arch::asm!(
                "mov {}, rbp",
                out(reg) rbp,
                options(nomem, nostack, preserves_flags),
            );
        }

        let mut depth = 0usize;
        while depth < MAX_UNWIND_FRAMES {
            if rbp == 0
                || (rbp & 7) != 0
                || rbp < USER_ADDR_MIN
                || rbp >= USER_ADDR_MAX - 16
            {
                break;
            }
            // SAFETY: `rbp` is 8-byte aligned within canonical user stack bounds and
            // monotonically ascending along the active thread's frame-pointer chain.
            let prev_rbp = unsafe { *(rbp as *const usize) };
            let ret_rip = unsafe { *((rbp + 8) as *const usize) };
            if ret_rip < USER_ADDR_MIN || ret_rip >= USER_ADDR_MAX {
                break;
            }

            let mut ctx = ExoUnwindContext {
                ip: ret_rip,
                cfa: rbp.saturating_add(16),
            };
            let rc = trace(&raw mut ctx as *mut c_void, trace_argument);
            if rc != _URC_NO_REASON {
                return rc;
            }

            if prev_rbp == 0
                || prev_rbp <= rbp
                || prev_rbp.wrapping_sub(rbp) > MAX_FRAME_STEP
            {
                break;
            }
            rbp = prev_rbp;
            depth += 1;
        }
        _URC_END_OF_STACK
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = (trace, trace_argument);
        _URC_END_OF_STACK
    }
}

#[linkage = "weak"]
#[unsafe(no_mangle)]
pub extern "C" fn _Unwind_GetIP(ctx: *mut c_void) -> usize {
    if ctx.is_null() {
        return 0;
    }
    // SAFETY: `ctx` points to `ExoUnwindContext` created on the stack by `_Unwind_Backtrace`.
    unsafe { (*(ctx as *const ExoUnwindContext)).ip }
}

#[linkage = "weak"]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Unwind_GetIPInfo(
    ctx: *mut c_void,
    ip_before_insn: *mut c_int,
) -> usize {
    if !ip_before_insn.is_null() {
        // SAFETY: caller passes a valid writable `c_int` pointer per IA-64/Itanium C++ ABI.
        // Return addresses from `call` point after the call instruction (`ip_before_insn = 0`).
        unsafe {
            *ip_before_insn = 0;
        }
    }
    _Unwind_GetIP(ctx)
}

#[linkage = "weak"]
#[unsafe(no_mangle)]
pub extern "C" fn _Unwind_GetCFA(ctx: *mut c_void) -> usize {
    if ctx.is_null() {
        return 0;
    }
    // SAFETY: `ctx` points to `ExoUnwindContext` created on the stack by `_Unwind_Backtrace`.
    unsafe { (*(ctx as *const ExoUnwindContext)).cfa }
}

#[linkage = "weak"]
#[unsafe(no_mangle)]
pub extern "C" fn _Unwind_FindEnclosingFunction(pc: *mut c_void) -> *mut c_void {
    pc
}
