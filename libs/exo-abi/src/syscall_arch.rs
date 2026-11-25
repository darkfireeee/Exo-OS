//! Raw x86_64 `syscall` invocation primitives (`syscall0`..`syscall6`) and
//! typed ExoFS / Capability raw syscall wrappers.

use crate::cap::ExoCapTokenWire;
use crate::numbers::{
    SYS_EXOFS_OBJECT_OPEN, SYS_EXOFS_OPEN_BY_PATH, SYS_EXOFS_PATH_RESOLVE, SYS_EXOFS_READDIR,
    SYS_EXO_CAP_CHECK, SYS_EXO_CAP_CREATE,
};
use crate::posix_consts::{ExofsOpenArgs, ExofsPathResolveResult};

/// # Safety
/// Caller must ensure `nr` and all 6 register arguments satisfy the target syscall contract.
#[cfg(all(target_arch = "x86_64", not(test)))]
#[inline(always)]
pub unsafe fn syscall6(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> i64 {
    let ret: i64;
    // SAFETY: caller upholds syscall ABI invariants.
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") nr,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            in("r10") a4,
            in("r8") a5,
            in("r9") a6,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

/// Host unit-test / non-x86_64 fallback returning `-ENOSYS`.
#[cfg(not(all(target_arch = "x86_64", not(test))))]
#[inline(always)]
pub unsafe fn syscall6(
    _nr: u64,
    _a1: u64,
    _a2: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    crate::errno::ENOSYS
}

/// # Safety
/// Caller must ensure `nr` is valid.
#[inline(always)]
pub unsafe fn syscall0(nr: u64) -> i64 {
    unsafe { syscall6(nr, 0, 0, 0, 0, 0, 0) }
}

/// # Safety
/// Caller must ensure `nr` and `a1` are valid.
#[inline(always)]
pub unsafe fn syscall1(nr: u64, a1: u64) -> i64 {
    unsafe { syscall6(nr, a1, 0, 0, 0, 0, 0) }
}

/// # Safety
/// Caller must ensure `nr` and arguments are valid.
#[inline(always)]
pub unsafe fn syscall2(nr: u64, a1: u64, a2: u64) -> i64 {
    unsafe { syscall6(nr, a1, a2, 0, 0, 0, 0) }
}

/// # Safety
/// Caller must ensure `nr` and arguments are valid.
#[inline(always)]
pub unsafe fn syscall3(nr: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    unsafe { syscall6(nr, a1, a2, a3, 0, 0, 0) }
}

/// # Safety
/// Caller must ensure `nr` and arguments are valid.
#[inline(always)]
pub unsafe fn syscall4(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    unsafe { syscall6(nr, a1, a2, a3, a4, 0, 0) }
}

/// # Safety
/// Caller must ensure `nr` and arguments are valid.
#[inline(always)]
pub unsafe fn syscall5(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> i64 {
    unsafe { syscall6(nr, a1, a2, a3, a4, a5, 0) }
}

/// # Safety
/// `token_out` must point to a valid writable `ExoCapTokenWire`.
#[inline(always)]
pub unsafe fn exo_cap_create(
    cap_type: u32,
    rights: u32,
    target_pid: u32,
    token_out: &mut ExoCapTokenWire,
) -> i64 {
    unsafe {
        syscall4(
            SYS_EXO_CAP_CREATE,
            cap_type as u64,
            rights as u64,
            target_pid as u64,
            token_out as *mut ExoCapTokenWire as u64,
        )
    }
}

/// # Safety
/// `token` must point to a valid readable `ExoCapTokenWire`.
#[inline(always)]
pub unsafe fn exo_cap_check(
    token: &ExoCapTokenWire,
    required_rights: u32,
    target_pid: u32,
    expected_type: u32,
) -> i64 {
    unsafe {
        syscall4(
            SYS_EXO_CAP_CHECK,
            token as *const ExoCapTokenWire as u64,
            required_rights as u64,
            target_pid as u64,
            expected_type as u64,
        )
    }
}

/// # Safety
/// `path_ptr` and `out` must designate valid userspace memory.
#[inline(always)]
pub unsafe fn exofs_path_resolve_raw(
    path_ptr: u64,
    path_len: u64,
    flags: u64,
    out: *mut ExofsPathResolveResult,
    cap_rights: u64,
) -> i64 {
    unsafe {
        syscall6(
            SYS_EXOFS_PATH_RESOLVE,
            path_ptr,
            path_len,
            flags,
            out as u64,
            0,
            cap_rights,
        )
    }
}

/// # Safety
/// `path_ptr`, `out_fd`, and `args` must designate valid userspace memory.
#[inline(always)]
pub unsafe fn exofs_object_open_raw(
    path_ptr: u64,
    path_len: u64,
    flags: u64,
    out_fd: *mut u32,
    args: *const ExofsOpenArgs,
    cap_rights: u64,
) -> i64 {
    unsafe {
        syscall6(
            SYS_EXOFS_OBJECT_OPEN,
            path_ptr,
            path_len,
            flags,
            out_fd as u64,
            args as u64,
            cap_rights,
        )
    }
}

/// # Safety
/// `path_ptr` must designate a valid NUL-terminated or bounded path in userspace memory.
#[inline(always)]
pub unsafe fn exofs_open_by_path_raw(path_ptr: u64, flags: u64, mode: u64, cap_rights: u64) -> i64 {
    unsafe {
        syscall6(
            SYS_EXOFS_OPEN_BY_PATH,
            path_ptr,
            flags,
            mode,
            0,
            0,
            cap_rights,
        )
    }
}

/// # Safety
/// `buf_ptr` must designate a valid writable buffer of at least `buf_len` bytes.
#[inline(always)]
pub unsafe fn exofs_readdir_raw(fd: u64, buf_ptr: u64, buf_len: u64, cap_rights: u64) -> i64 {
    unsafe { syscall6(SYS_EXOFS_READDIR, fd, buf_ptr, buf_len, 0, 0, cap_rights) }
}
