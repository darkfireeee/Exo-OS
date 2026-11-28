//! # syscall/table/ipc/remote_memory.rs — Inter-process memory operations for privileged servers

use crate::syscall::table::common::*;
use crate::memory::core::{phys_to_virt, PhysAddr, VirtAddr};
use crate::process::core::pid::Pid;
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::syscall::numbers::*;
use crate::syscall::validation::{copy_from_user, copy_to_user};

#[allow(dead_code)]
pub(super) fn process_name_eq(name: &[u8; 16], expected: &[u8]) -> bool {
    let mut len = 0usize;
    while len < name.len() && name[len] != 0 {
        len += 1;
    }
    if len != expected.len() {
        return false;
    }
    let mut i = 0usize;
    while i < len {
        if name[i] != expected[i] {
            return false;
        }
        i += 1;
    }
    true
}

fn caller_can_manage_target_memory(caller_pid: u32) -> bool {
    if caller_pid == 1 {
        return true;
    }
    match PROCESS_REGISTRY.find_by_pid(Pid(caller_pid)) {
        Some(pcb) => {
            // T9-003 FIX: Strict authentication — must be a direct child of init (PID 1),
            // root (euid 0) AND have the reserved service class verified.
            if pcb.ppid().0 != 1 || pcb.creds.lock().euid != 0 {
                return false;
            }
            let class = crate::security::service_class_of(Pid(caller_pid));
            class == crate::security::ServiceClass::MemoryServer
                || class == crate::security::ServiceClass::FbServer
        }
        None => false,
    }
}

fn caller_can_copy_target_memory(caller_pid: u32) -> bool {
    if caller_pid == 1 {
        return true;
    }
    match PROCESS_REGISTRY.find_by_pid(Pid(caller_pid)) {
        Some(pcb) => {
            // T9-003 FIX: Must be a direct child of init (PID 1), root (euid 0), and authorized service.
            if pcb.ppid().0 != 1 || pcb.creds.lock().euid != 0 {
                return false;
            }
            let class = crate::security::service_class_of(Pid(caller_pid));
            class == crate::security::ServiceClass::MemoryServer
                || class == crate::security::ServiceClass::VfsServer
                || class == crate::security::ServiceClass::FbServer
        }
        None => false,
    }
}

struct PinnedUserAddressSpace {
    ptr: *const crate::memory::virt::UserAddressSpace,
}

impl core::ops::Deref for PinnedUserAddressSpace {
    type Target = crate::memory::virt::UserAddressSpace;

    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: `pin_remote()` keeps `UserAddressSpace` alive until `Drop`.
        unsafe { &*self.ptr }
    }
}

impl Drop for PinnedUserAddressSpace {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: paired with `pin_remote()` in `user_as_for_pid`.
        unsafe { &*self.ptr }.unpin_remote();
    }
}

fn user_as_for_pid(pid: u32) -> Result<PinnedUserAddressSpace, i64> {
    if pid == 0 {
        return Err(EINVAL);
    }
    PROCESS_REGISTRY
        .with_pinned_pid(Pid(pid), |pcb| {
            let _as_guard = pcb.as_lock.lock();
            let as_ptr = pcb.address_space_ptr();
            if as_ptr.is_null() {
                return Err(ENOMEM);
            }
            let ptr = as_ptr as *const crate::memory::virt::UserAddressSpace;
            // SAFETY: `pcb` is pinned in `PROCESS_REGISTRY` and `as_lock`
            // prevents `exec()` from swapping `pcb.address_space` until we pin it.
            unsafe { &*ptr }.pin_remote();
            Ok(PinnedUserAddressSpace { ptr })
        })
        .ok_or(ENOENT)?
}

fn validate_remote_user_range(addr: u64, len: usize) -> Result<(), i64> {
    if len == 0 {
        return Ok(());
    }
    if addr == 0 {
        return Err(EFAULT);
    }
    let end = addr.checked_add(len as u64).ok_or(EFAULT)?;
    if end > crate::syscall::validation::USER_ADDR_MAX {
        return Err(EFAULT);
    }
    Ok(())
}

fn remote_user_phys(
    user_as: &crate::memory::virt::UserAddressSpace,
    addr: u64,
    write: bool,
) -> Result<(u64, usize), i64> {
    use crate::memory::virt::page_table::{PageTableWalker, WalkResult};

    let walker = PageTableWalker::new(user_as.pml4_phys());
    let virt = VirtAddr::new(addr);
    let (entry, level) = match walker.walk_read(virt) {
        WalkResult::Leaf { entry, level } | WalkResult::HugePage { entry, level } => (entry, level),
        _ => return Err(EFAULT),
    };
    if !entry.is_user() || (write && (!entry.is_writable() || entry.is_cow())) {
        return Err(EFAULT);
    }
    let page_size = level.page_size();
    let offset = (addr as usize) & (page_size - 1);
    Ok((
        entry.phys_addr().as_u64() + offset as u64,
        page_size - offset,
    ))
}

/// `exo_mem_copy_from_pid(target_pid, remote_src, local_dst, len, flags)`.
///
/// Reserved for root servers that must serve IPC requests with
/// pointers belonging to the client. The local buffer is always copied via
/// `copy_to_user`; the remote buffer is translated via the PML4 of the target PID.
pub fn sys_exo_mem_copy_from_pid(
    target_pid: u64,
    remote_src: u64,
    local_dst: u64,
    len: u64,
    flags: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_MEM_COPY_FROM_PID);
    if flags != 0 {
        return EINVAL;
    }
    let caller_pid = current_pid_u32();
    if !caller_can_copy_target_memory(caller_pid) {
        return EPERM;
    }
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = validate_remote_user_range(remote_src, len) {
        return e;
    }
    if let Err(e) = validate_remote_user_range(local_dst, len) {
        return e;
    }
    let user_as = match user_as_for_pid(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let mut copied = 0usize;
    let mut scratch = [0u8; 2048];
    while copied < len {
        let src = remote_src + copied as u64;
        let n = {
            let _pt_guard = user_as.lock_pt();
            let (phys, page_avail) = match remote_user_phys(&user_as, src, false) {
                Ok(v) => v,
                Err(e) => return if copied == 0 { e } else { copied as i64 },
            };
            let n = (len - copied).min(page_avail).min(scratch.len());
            let src_ptr = phys_to_virt(PhysAddr::new(phys)).as_u64() as *const u8;
            // SAFETY: `remote_user_phys` validated the target PID translation under
            // `user_as.lock_pt()`, preventing concurrent unmap/free during copy.
            unsafe {
                core::ptr::copy_nonoverlapping(src_ptr, scratch.as_mut_ptr(), n);
            }
            n
        };
        if copy_to_user((local_dst + copied as u64) as *mut u8, scratch.as_ptr(), n).is_err() {
            return if copied == 0 { EFAULT } else { copied as i64 };
        }
        copied += n;
    }
    copied as i64
}

/// `exo_mem_copy_to_pid(target_pid, remote_dst, local_src, len, flags)`.
pub fn sys_exo_mem_copy_to_pid(
    target_pid: u64,
    remote_dst: u64,
    local_src: u64,
    len: u64,
    flags: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_MEM_COPY_TO_PID);
    if flags != 0 {
        return EINVAL;
    }
    let caller_pid = current_pid_u32();
    if !caller_can_copy_target_memory(caller_pid) {
        return EPERM;
    }
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if let Err(e) = validate_remote_user_range(remote_dst, len) {
        return e;
    }
    if let Err(e) = validate_remote_user_range(local_src, len) {
        return e;
    }
    let user_as = match user_as_for_pid(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let mut copied = 0usize;
    let mut scratch = [0u8; 2048];
    while copied < len {
        let dst = remote_dst + copied as u64;
        let max_chunk = (len - copied).min(scratch.len()).min(
            crate::memory::core::PAGE_SIZE
                - ((dst as usize) & (crate::memory::core::PAGE_SIZE - 1)),
        );
        if copy_from_user(
            scratch.as_mut_ptr(),
            (local_src + copied as u64) as *const u8,
            max_chunk,
        )
        .is_err()
        {
            return if copied == 0 { EFAULT } else { copied as i64 };
        }
        let n = {
            let _pt_guard = user_as.lock_pt();
            let (phys, page_avail) = match remote_user_phys(&user_as, dst, true) {
                Ok(v) => v,
                Err(e) => return if copied == 0 { e } else { copied as i64 },
            };
            let n = max_chunk.min(page_avail);
            let dst_ptr = phys_to_virt(PhysAddr::new(phys)).as_u64() as *mut u8;
            // SAFETY: `remote_user_phys(..., write=true)` under `user_as.lock_pt()`
            // guarantees a present writable user page that cannot be unmapped/protected
            // while copying from `scratch`.
            unsafe {
                core::ptr::copy_nonoverlapping(scratch.as_ptr(), dst_ptr, n);
            }
            n
        };
        copied += n;
    }
    copied as i64
}

/// `exo_mem_map_pid(target_pid, hint, len, prot, flags)` reserved for memory_server.
pub fn sys_exo_mem_map_pid(
    target_pid: u64,
    hint: u64,
    len: u64,
    prot: u64,
    flags: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_MEM_MAP_PID);
    let caller_pid = current_pid_u32();
    if !caller_can_manage_target_memory(caller_pid) {
        return EPERM;
    }
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let prot = match checked_u32_sysarg(prot) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let flags = match checked_u32_sysarg(flags) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let user_as = match user_as_for_pid(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::memory::virt::mmap::do_mmap_in_as(&user_as, hint, len, prot, flags) {
        Ok(addr) => addr as i64,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_mem_munmap_pid(target_pid, addr, len)` reserved for memory_server.
pub fn sys_exo_mem_munmap_pid(
    target_pid: u64,
    addr: u64,
    len: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_MEM_MUNMAP_PID);
    let caller_pid = current_pid_u32();
    if !caller_can_manage_target_memory(caller_pid) {
        return EPERM;
    }
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if len == 0 {
        return EINVAL;
    }
    if let Err(e) = validate_remote_user_range(addr, len) {
        return e;
    }
    let user_as = match user_as_for_pid(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::memory::virt::mmap::do_munmap_range_in_as(&user_as, addr, len) {
        Ok(()) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_mem_mprotect_pid(target_pid, addr, len, prot)` reserved for memory_server.
pub fn sys_exo_mem_mprotect_pid(
    target_pid: u64,
    addr: u64,
    len: u64,
    prot: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_MEM_MPROTECT_PID);
    let caller_pid = current_pid_u32();
    if !caller_can_manage_target_memory(caller_pid) {
        return EPERM;
    }
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let prot = match checked_u32_sysarg(prot) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let user_as = match user_as_for_pid(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::memory::virt::mmap::do_mprotect_in_as(&user_as, addr, len, prot) {
        Ok(()) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_mem_share(src_pid, src_addr, dst_pid, len, prot, dst_hint)` reserved for privileged servers.
pub fn sys_exo_mem_share(
    src_pid: u64,
    src_addr: u64,
    dst_pid: u64,
    len: u64,
    prot: u64,
    dst_hint: u64,
) -> i64 {
    stat_inc(SYS_EXO_MEM_SHARE);
    let caller_pid = current_pid_u32();
    if !caller_can_manage_target_memory(caller_pid) {
        return EPERM;
    }
    let src_pid = match checked_u32_sysarg(src_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let dst_pid = match checked_u32_sysarg(dst_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let prot = match checked_u32_sysarg(prot) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let src_as = match user_as_for_pid(src_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let dst_as = match user_as_for_pid(dst_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::memory::virt::mmap::do_share_range_between_as(
        &src_as, src_addr, &dst_as, dst_hint, len, prot,
    ) {
        Ok(addr) => addr as i64,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_mem_revoke(target_pid, addr, len)` reserved for privileged servers.
pub fn sys_exo_mem_revoke(
    target_pid: u64,
    addr: u64,
    len: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_MEM_REVOKE);
    let caller_pid = current_pid_u32();
    if !caller_can_manage_target_memory(caller_pid) {
        return EPERM;
    }
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if len == 0 {
        return EINVAL;
    }
    let user_as = match user_as_for_pid(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::memory::virt::mmap::do_munmap_range_in_as(&user_as, addr, len) {
        Ok(()) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}
