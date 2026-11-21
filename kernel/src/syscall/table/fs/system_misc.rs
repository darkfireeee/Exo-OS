//! # syscall/table/fs/system_misc.rs — Miscellaneous handlers (limits, reboot, mremap, msync, time)

use crate::syscall::numbers::*;
use crate::syscall::table::common::*;
use crate::syscall::validation::{copy_from_user, copy_to_user, UserBuf};

/// `getrlimit(resource, rlim)`.
pub fn sys_getrlimit(resource: u64, rlim_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_GETRLIMIT);
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_getrlimit(resource as u32, rlim_ptr, pid))
}

/// `setrlimit(resource, rlim)`.
pub fn sys_setrlimit(resource: u64, rlim_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_SETRLIMIT);
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_setrlimit(resource as u32, rlim_ptr, pid))
}

const LINUX_REBOOT_MAGIC1: u64 = 0xfee1_dead;
const LINUX_REBOOT_MAGIC2: u64 = 672_274_793;
const LINUX_REBOOT_CMD_RESTART: u64 = 0x0123_4567;
const LINUX_REBOOT_CMD_POWER_OFF: u64 = 0x4321_fedc;
const EXO_REBOOT_CMD_COMPLIANCE_EXIT: u64 = 0x4558_4f54; // "EXOT"
const QEMU_DEBUG_EXIT_PORT: u16 = 0x00f4;
const QEMU_DEBUG_EXIT_SUCCESS: u32 = 0x10; // QEMU exit code (0x10 << 1) | 1 = 33
const QEMU_DEBUG_EXIT_FAILURE: u32 = 0x11; // QEMU exit code (0x11 << 1) | 1 = 35
const PS2_CONTROLLER_PORT: u16 = 0x0064;
const PS2_RESET_COMMAND: u8 = 0xfe;

#[inline(never)]
fn halt_after_reboot_request() -> ! {
    crate::arch::halt_cpu()
}

#[inline(never)]
#[cfg(target_arch = "x86_64")]
fn platform_debug_exit(code: u32) -> ! {
    // SAFETY: SYS_REBOOT runs in ring 0. Port 0xf4 is the configured
    // QEMU isa-debug-exit device in the project boot commands.
    unsafe {
        crate::arch::x86_64::outl(QEMU_DEBUG_EXIT_PORT, code);
    }
    halt_after_reboot_request()
}

#[inline(never)]
#[cfg(not(target_arch = "x86_64"))]
fn platform_debug_exit(_code: u32) -> ! {
    halt_after_reboot_request()
}

#[inline(never)]
fn platform_poweroff() -> ! {
    platform_debug_exit(QEMU_DEBUG_EXIT_SUCCESS)
}

#[inline(never)]
#[cfg(target_arch = "x86_64")]
fn platform_restart() -> ! {
    // SAFETY: SYS_REBOOT runs in ring 0. 0x64/0xfe is the legacy PS/2
    // controller CPU reset command supported by QEMU and many PCs.
    unsafe {
        crate::arch::x86_64::outb(PS2_CONTROLLER_PORT, PS2_RESET_COMMAND);
    }
    halt_after_reboot_request()
}

#[inline(never)]
#[cfg(not(target_arch = "x86_64"))]
fn platform_restart() -> ! {
    halt_after_reboot_request()
}

/// `reboot(magic1, magic2, cmd, arg)` compatible Linux/musl.
pub fn sys_reboot(magic1: u64, magic2: u64, cmd: u64, arg: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_REBOOT);
    if magic1 != LINUX_REBOOT_MAGIC1 || magic2 != LINUX_REBOOT_MAGIC2 {
        return EINVAL;
    }

    let caller = current_pid_u32();
    if cmd == EXO_REBOOT_CMD_COMPLIANCE_EXIT {
        let is_root = caller <= 1
            || crate::process::core::registry::PROCESS_REGISTRY
                .find_by_pid(crate::process::core::pid::Pid(caller))
                .map(|pcb| pcb.creds.lock().euid == 0)
                .unwrap_or(false);
        if !is_root {
            crate::security::shield_feed::push_event(
                caller,
                crate::security::shield_feed::event_type::SYSCALL,
                crate::security::shield_feed::severity::CRITICAL,
                crate::syscall::numbers::SYS_REBOOT as u32,
                magic1,
                cmd,
            );
            return EPERM;
        }
        let exit_val = match arg as u32 {
            QEMU_DEBUG_EXIT_SUCCESS => QEMU_DEBUG_EXIT_SUCCESS,
            QEMU_DEBUG_EXIT_FAILURE => QEMU_DEBUG_EXIT_FAILURE,
            _ => return EINVAL,
        };
        let _ = crate::syscall::fs_bridge::fs_sync(caller);
        platform_debug_exit(exit_val);
    }

    // Only PID 0 (kernel boot context) or PID 1 (init) may reboot the system.
    // Any other process knowing the magic constants could otherwise DoS the system.
    if caller > 1 {
        crate::security::shield_feed::push_event(
            caller,
            crate::security::shield_feed::event_type::SYSCALL,
            crate::security::shield_feed::severity::CRITICAL,
            crate::syscall::numbers::SYS_REBOOT as u32,
            magic1,
            cmd,
        );
        return EPERM;
    }

    use crate::syscall::fs_bridge;
    if let Err(err) = fs_bridge::fs_sync(current_pid_u32()) {
        return err.to_errno();
    }

    match cmd {
        LINUX_REBOOT_CMD_POWER_OFF => platform_poweroff(),
        LINUX_REBOOT_CMD_RESTART => platform_restart(),
        _ => EINVAL,
    }
}

/// `msync(addr, length, flags)`; accepted for RAM-backed ExoFS mappings.
pub fn sys_msync(addr: u64, len: u64, flags: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MSYNC);
    const MS_ASYNC: u64 = 0x1;
    const MS_INVALIDATE: u64 = 0x2;
    const MS_SYNC: u64 = 0x4;
    if addr == 0 || len == 0 || flags & !(MS_ASYNC | MS_INVALIDATE | MS_SYNC) != 0 {
        return EINVAL;
    }
    0
}

/// `mremap(old_addr, old_size, new_size, flags, new_addr)`.
pub fn sys_mremap(
    old_addr: u64,
    old_size: u64,
    new_size: u64,
    flags: u64,
    new_addr: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_MREMAP);
    const MREMAP_MAYMOVE: u64 = 0x1;
    const MREMAP_FIXED: u64 = 0x2;
    const MREMAP_DONTUNMAP: u64 = 0x4;
    const PROT_READ_WRITE: u32 = 0x1 | 0x2;
    const MAP_PRIVATE_ANON: u32 = 0x02 | 0x20;
    const MAP_FIXED: u32 = 0x10;
    const PAGE: u64 = crate::memory::core::PAGE_SIZE as u64;
    let user_end = crate::memory::core::layout::USER_END.as_u64();

    if old_addr < PAGE || !old_addr.is_multiple_of(PAGE) || old_size == 0 || new_size == 0 {
        return EINVAL;
    }
    if flags & !(MREMAP_MAYMOVE | MREMAP_FIXED | MREMAP_DONTUNMAP) != 0 {
        return EINVAL;
    }
    if flags & MREMAP_DONTUNMAP != 0 && flags & MREMAP_MAYMOVE == 0 {
        return EINVAL;
    }
    if flags & MREMAP_FIXED != 0
        && (flags & MREMAP_MAYMOVE == 0 || new_addr < PAGE || !new_addr.is_multiple_of(PAGE))
    {
        return EINVAL;
    }
    let old_len = match checked_usize_sysarg(old_size) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let new_len = match checked_usize_sysarg(new_size) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let Some(old_len_aligned) = (old_len as u64).checked_add(PAGE - 1).map(|v| v & !(PAGE - 1)) else {
        return EINVAL;
    };
    let Some(new_len_aligned) = (new_len as u64).checked_add(PAGE - 1).map(|v| v & !(PAGE - 1)) else {
        return ENOMEM;
    };
    let Some(old_end) = old_addr.checked_add(old_len_aligned) else {
        return EINVAL;
    };
    if old_end > user_end {
        return EFAULT;
    }
    if flags & MREMAP_FIXED != 0 {
        let Some(new_end) = new_addr.checked_add(new_len_aligned) else {
            return EINVAL;
        };
        if new_end > user_end || (new_addr < old_end && new_end > old_addr) {
            return EINVAL;
        }
    }
    if new_len <= old_len && flags & MREMAP_FIXED == 0 {
        if new_len < old_len {
            let shrink_start = old_addr.saturating_add(new_len as u64);
            let aligned_shrink_start = (shrink_start.saturating_add(PAGE - 1)) & !(PAGE - 1);
            if aligned_shrink_start < old_end {
                let shrink_len = (old_end - aligned_shrink_start) as usize;
                let _ = crate::memory::virt::mmap::do_munmap(aligned_shrink_start, shrink_len);
            }
        }
        return old_addr as i64;
    }
    if flags & MREMAP_MAYMOVE == 0 {
        return ENOMEM;
    }

    match crate::memory::virt::mmap::do_mremap_zero_copy(
        old_addr, old_len, new_len, flags, new_addr,
    ) {
        Ok(crate::memory::virt::mmap::MremapZeroCopy::Moved(addr)) => return addr as i64,
        Ok(crate::memory::virt::mmap::MremapZeroCopy::Unsupported) => {}
        Err(e) => return e.to_kernel_errno() as i64,
    }

    let (hint, mmap_flags) = if flags & MREMAP_FIXED != 0 {
        (new_addr, MAP_PRIVATE_ANON | MAP_FIXED)
    } else {
        (0, MAP_PRIVATE_ANON)
    };
    let mapped = match crate::memory::virt::mmap::do_mmap(
        hint,
        new_len,
        PROT_READ_WRITE,
        mmap_flags,
        -1,
        0,
    ) {
        Ok(addr) => addr as u64,
        Err(e) => return e.to_kernel_errno() as i64,
    };

    let copy_len = old_len.min(new_len);
    if copy_len != 0 {
        if let Err(errno) = copy_user_range_streamed(old_addr, mapped, copy_len) {
            let _ = crate::memory::virt::mmap::do_munmap(mapped, new_len);
            return errno;
        }
    }
    if flags & MREMAP_DONTUNMAP == 0 {
        let _ = crate::memory::virt::mmap::do_munmap(old_addr, old_len);
    }
    mapped as i64
}

fn copy_user_range_streamed(src: u64, dst: u64, len: usize) -> Result<(), i64> {
    const MREMAP_COPY_CHUNK: usize = crate::memory::core::PAGE_SIZE;
    let mut buf = [0u8; MREMAP_COPY_CHUNK];
    let mut copied = 0usize;
    while copied < len {
        let n = core::cmp::min(MREMAP_COPY_CHUNK, len - copied);
        let src_addr = src.checked_add(copied as u64).ok_or(EFAULT)?;
        let dst_addr = dst.checked_add(copied as u64).ok_or(EFAULT)?;
        UserBuf::validate(src_addr, n, MREMAP_COPY_CHUNK).map_err(|e| e.to_errno())?;
        UserBuf::validate(dst_addr, n, MREMAP_COPY_CHUNK).map_err(|e| e.to_errno())?;
        copy_from_user(buf.as_mut_ptr(), src_addr as *const u8, n).map_err(|e| e.to_errno())?;
        copy_to_user(dst_addr as *mut u8, buf.as_ptr(), n).map_err(|e| e.to_errno())?;
        copied += n;
    }
    Ok(())
}

/// `clock_gettime(clockid, tp)` slow-path wrapper.
pub fn sys_clock_gettime(clk_id: u64, tp_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_CLOCK_GETTIME);
    crate::syscall::fast_path::sys_clock_gettime(clk_id, tp_ptr)
}
