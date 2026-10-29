//! # syscall/table/ipc/remote_memory.rs — Opérations de mémoire inter-processus pour serveurs privilégiés

use crate::syscall::table::common::*;
use crate::memory::core::{phys_to_virt, PhysAddr, VirtAddr};
use crate::process::core::pid::Pid;
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::syscall::numbers::*;
use crate::syscall::validation::{copy_from_user, copy_to_user};

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
            // T9-003 FIX : Authentification stricte — doit être un enfant direct d'init (PID 1),
            // root (euid 0) ET avoir le nom de service réservé vérifié.
            if pcb.ppid().0 != 1 || pcb.creds.lock().euid != 0 {
                return false;
            }
            let name = pcb.name_snapshot();
            process_name_eq(&name, b"memory_server") || process_name_eq(&name, b"fb_server")
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
            // T9-003 FIX : Doit être un enfant direct d'init (PID 1), root (euid 0) et service autorisé.
            if pcb.ppid().0 != 1 || pcb.creds.lock().euid != 0 {
                return false;
            }
            let name = pcb.name_snapshot();
            process_name_eq(&name, b"memory_server")
                || process_name_eq(&name, b"vfs_server")
                || process_name_eq(&name, b"fb_server")
        }
        None => false,
    }
}

fn user_as_for_pid(pid: u32) -> Result<&'static crate::memory::virt::UserAddressSpace, i64> {
    if pid == 0 {
        return Err(EINVAL);
    }
    let pcb = PROCESS_REGISTRY.find_by_pid(Pid(pid)).ok_or(ENOENT)?;
    let as_ptr = pcb.address_space_ptr();
    if as_ptr.is_null() {
        return Err(ENOMEM);
    }
    // SAFETY: le PCB reste dans PROCESS_REGISTRY pendant le syscall et
    // address_space pointe vers un UserAddressSpace créé par lifecycle/create.
    Ok(unsafe { &*(as_ptr as *const crate::memory::virt::UserAddressSpace) })
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
/// Réservé aux serveurs racine qui doivent servir des requêtes IPC avec des
/// pointeurs appartenant au client. Le buffer local est toujours copié via
/// `copy_to_user`; le buffer distant est traduit via la PML4 du PID cible.
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
    let mut scratch = [0u8; 256];
    while copied < len {
        let src = remote_src + copied as u64;
        let (phys, page_avail) = match remote_user_phys(user_as, src, false) {
            Ok(v) => v,
            Err(e) => return if copied == 0 { e } else { copied as i64 },
        };
        let n = (len - copied).min(page_avail).min(scratch.len());
        let src_ptr = phys_to_virt(PhysAddr::new(phys)).as_u64() as *const u8;
        // SAFETY: `remote_user_phys` a validé la traduction du PID cible et
        // `scratch` est un buffer kernel local de taille `n`.
        unsafe {
            core::ptr::copy_nonoverlapping(src_ptr, scratch.as_mut_ptr(), n);
        }
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
    let mut scratch = [0u8; 256];
    while copied < len {
        let dst = remote_dst + copied as u64;
        let (phys, page_avail) = match remote_user_phys(user_as, dst, true) {
            Ok(v) => v,
            Err(e) => return if copied == 0 { e } else { copied as i64 },
        };
        let n = (len - copied).min(page_avail).min(scratch.len());
        if copy_from_user(
            scratch.as_mut_ptr(),
            (local_src + copied as u64) as *const u8,
            n,
        )
        .is_err()
        {
            return if copied == 0 { EFAULT } else { copied as i64 };
        }
        let dst_ptr = phys_to_virt(PhysAddr::new(phys)).as_u64() as *mut u8;
        // SAFETY: `remote_user_phys(..., write=true)` garantit une page user
        // présente et writable; `scratch` contient `n` octets copiés du caller.
        unsafe {
            core::ptr::copy_nonoverlapping(scratch.as_ptr(), dst_ptr, n);
        }
        copied += n;
    }
    copied as i64
}

/// `exo_mem_map_pid(target_pid, hint, len, prot, flags)` réservé à memory_server.
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
    match crate::memory::virt::mmap::do_mmap_in_as(user_as, hint, len, prot, flags) {
        Ok(addr) => addr as i64,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_mem_munmap_pid(target_pid, addr)` réservé à memory_server.
pub fn sys_exo_mem_munmap_pid(
    target_pid: u64,
    addr: u64,
    _len: u64,
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
    let user_as = match user_as_for_pid(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::memory::virt::mmap::do_munmap_in_as(user_as, addr) {
        Ok(()) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_mem_mprotect_pid(target_pid, addr, len, prot)` réservé à memory_server.
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
    match crate::memory::virt::mmap::do_mprotect_in_as(user_as, addr, len, prot) {
        Ok(()) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}
