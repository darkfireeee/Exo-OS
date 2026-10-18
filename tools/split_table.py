#!/usr/bin/env python3
import os

def run():
    root = "/mnt/c/Users/xavie/Desktop/Exo-OS"
    src_file = os.path.join(root, "kernel/src/syscall/table.rs")
    out_dir = os.path.join(root, "kernel/src/syscall/table")
    os.makedirs(out_dir, exist_ok=True)

    with open(src_file, "r", encoding="utf-8") as f:
        lines = f.readlines()

    # Index 0-based
    # common:
    # 0..132 (headers, SyscallHandler, SYSCALL_STATS, syscall_table_stat, syscall_stats_for, stat_inc, sys_enosys)
    # + 3857..3884 (checked_u32_sysarg, checked_i32_sysarg, checked_usize_sysarg)

    common_code = """//! # syscall/table/common.rs — Types communs, statistiques et validateurs d'arguments

use crate::syscall::errno::{EINVAL, ENOSYS};
use crate::syscall::numbers::SYSCALL_TABLE_SIZE;
use core::sync::atomic::{AtomicU64, Ordering};

pub type SyscallHandler = fn(u64, u64, u64, u64, u64, u64) -> i64;

static SYSCALL_STATS: [AtomicU64; SYSCALL_TABLE_SIZE] = {
    unsafe {
        core::mem::transmute::<[u64; SYSCALL_TABLE_SIZE], [AtomicU64; SYSCALL_TABLE_SIZE]>(
            [0u64; SYSCALL_TABLE_SIZE],
        )
    }
};

#[inline]
pub fn syscall_table_stat(nr: usize) -> u64 {
    if nr < SYSCALL_TABLE_SIZE {
        SYSCALL_STATS[nr].load(Ordering::Relaxed)
    } else {
        0
    }
}

#[inline]
pub fn syscall_stats_for(nr: u64) -> u64 {
    syscall_table_stat(nr as usize)
}

#[inline(always)]
pub(crate) fn stat_inc(nr: u64) {
    if (nr as usize) < SYSCALL_TABLE_SIZE {
        SYSCALL_STATS[nr as usize].fetch_add(1, Ordering::Relaxed);
    }
}

pub fn sys_enosys(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    ENOSYS
}

#[inline]
pub fn checked_u32_sysarg(value: u64) -> Result<u32, i64> {
    if value > u32::MAX as u64 {
        Err(EINVAL)
    } else {
        Ok(value as u32)
    }
}

#[inline]
pub fn checked_i32_sysarg(value: u64) -> Result<i32, i64> {
    let signed = value as i64;
    if signed < i32::MIN as i64 || signed > i32::MAX as i64 {
        Err(EINVAL)
    } else {
        Ok(signed as i32)
    }
}

#[inline]
pub fn checked_usize_sysarg(value: u64) -> Result<usize, i64> {
    if value > usize::MAX as u64 {
        Err(EINVAL)
    } else {
        Ok(value as usize)
    }
}
"""

    with open(os.path.join(out_dir, "common.rs"), "w", encoding="utf-8") as f:
        f.write(common_code)

    # FS: lignes 132..2051 (0-indexed: 132..2051)
    fs_header = """//! # syscall/table/fs.rs — Handlers pour I/O, descripteurs de fichiers et ExoFS

extern crate alloc;
use alloc::vec::Vec;
use super::common::{checked_i32_sysarg, checked_u32_sysarg, checked_usize_sysarg, stat_inc};
use crate::syscall::errno::*;
use crate::syscall::numbers::*;
use crate::syscall::validation::{
    copy_from_user, copy_to_user, read_user_path, read_user_typed, validate_fd, validate_flags,
    write_user_typed, UserBuf, IO_BUF_MAX,
};
use crate::fs::exofs::syscall::sys_exofs_object_set_meta;
"""
    fs_body = "".join(lines[132:2051])
    with open(os.path.join(out_dir, "fs.rs"), "w", encoding="utf-8") as f:
        f.write(fs_header + "\n" + fs_body)

    # Memory: lignes 2057..2159
    mem_header = """//! # syscall/table/memory.rs — Handlers de gestion mémoire (mmap, brk, mprotect, madvise)

use super::common::{checked_i32_sysarg, checked_u32_sysarg, checked_usize_sysarg, stat_inc};
use crate::syscall::errno::*;
use crate::syscall::numbers::*;
use core::sync::atomic::Ordering;
"""
    mem_body = "".join(lines[2057:2159])
    with open(os.path.join(out_dir, "memory.rs"), "w", encoding="utf-8") as f:
        f.write(mem_header + "\n" + mem_body)

    # Process: lignes 2159..2490
    proc_header = """//! # syscall/table/process.rs — Handlers cycle de vie des processus (fork, exec, wait, uid/gid)

use super::common::{checked_i32_sysarg, checked_u32_sysarg, checked_usize_sysarg, stat_inc};
use crate::syscall::errno::*;
use crate::syscall::numbers::*;
use crate::syscall::validation::{read_user_typed, write_user_typed};
"""
    proc_body = "".join(lines[2159:2490])
    with open(os.path.join(out_dir, "process.rs"), "w", encoding="utf-8") as f:
        f.write(proc_header + "\n" + proc_body)

    # Signal: lignes 2490..2549
    sig_header = """//! # syscall/table/signal.rs — Handlers des signaux POSIX (kill, sigaction, sigaltstack)

use super::common::stat_inc;
use crate::syscall::errno::*;
use crate::syscall::numbers::*;
"""
    sig_body = "".join(lines[2490:2549])
    with open(os.path.join(out_dir, "signal.rs"), "w", encoding="utf-8") as f:
        f.write(sig_header + "\n" + sig_body)

    # Time: lignes 2051..2057 (sys_gettimeofday) + 2549..2754
    time_header = """//! # syscall/table/time.rs — Handlers temps, horloges, nanosleep et futex

use super::common::{checked_i32_sysarg, checked_u32_sysarg, checked_usize_sysarg, stat_inc};
use crate::syscall::errno::*;
use crate::syscall::fast_path::Timespec;
use crate::syscall::numbers::*;
use crate::syscall::validation::{read_user_typed, write_user_typed};
"""
    time_gettimeofday = "".join(lines[2051:2057])
    time_rest = "".join(lines[2549:2754])
    with open(os.path.join(out_dir, "time.rs"), "w", encoding="utf-8") as f:
        f.write(time_header + "\n" + time_gettimeofday + "\n" + time_rest)

    # IPC & Security: lignes 2754..4306
    # Note: dans ce bloc, supprimer les lignes 3857..3885 (déjà dans common.rs)
    ipc_header = """//! # syscall/table/ipc.rs — Handlers IPC natifs, Capabilities, ExoCordon, ExoShield et ExoFS natif

extern crate alloc;
use alloc::vec::Vec;
use super::common::{checked_i32_sysarg, checked_u32_sysarg, checked_usize_sysarg, stat_inc};
use crate::syscall::errno::*;
use crate::syscall::numbers::*;
use crate::syscall::validation::{
    copy_from_user, copy_to_user, read_user_path, read_user_typed, write_user_typed,
};
use crate::fs::exofs::syscall::{
    sys_exofs_epoch_commit, sys_exofs_export_object, sys_exofs_gc_trigger,
    sys_exofs_get_content_hash, sys_exofs_import_object, sys_exofs_object_create,
    sys_exofs_object_delete, sys_exofs_object_open, sys_exofs_object_read,
    sys_exofs_object_stat, sys_exofs_object_write, sys_exofs_open_by_path,
    sys_exofs_path_resolve, sys_exofs_quota_query, sys_exofs_readdir,
    sys_exofs_relation_create, sys_exofs_relation_query, sys_exofs_snapshot_create,
    sys_exofs_snapshot_list, sys_exofs_snapshot_mount,
};
use crate::ipc::core::types::{EndpointId, IpcError};
use crate::memory::core::{phys_to_virt, PhysAddr, VirtAddr};
use crate::process::core::pid::Pid;
use crate::process::core::registry::PROCESS_REGISTRY;
"""
    # 2754..3857 + 3885..4306
    ipc_part1 = "".join(lines[2754:3857])
    ipc_part2 = "".join(lines[3884:4306])
    with open(os.path.join(out_dir, "ipc.rs"), "w", encoding="utf-8") as f:
        f.write(ipc_header + "\n" + ipc_part1 + "\n" + ipc_part2)

    # Drivers: lignes 4306..5025
    drivers_header = """//! # syscall/table/drivers.rs — Handlers des pilotes matériels GI-03 (DMA, PCI, IRQ, MSI, MMIO)

use super::common::{checked_i32_sysarg, checked_u32_sysarg, checked_usize_sysarg, stat_inc};
use crate::arch::x86_64::irq::{
    irq_error_to_errno, parse_irq_source_kind, IpcEndpoint, IrqAckResult, IrqOwnerPid, IrqVector,
};
use crate::drivers::{ClaimError, MmioError, MsiError, PciCfgError, TopoError};
use crate::memory::core::{phys_to_virt, PhysAddr, VirtAddr};
use crate::memory::dma::core::types::{
    DmaDirection, DmaError, DmaMapFlags, IommuDomainId, IovaAddr,
};
use crate::syscall::errno::*;
use crate::syscall::numbers::*;
use crate::syscall::validation::{copy_from_user, copy_to_user, read_user_typed, write_user_typed};
use pci_types::PciAddress;
"""
    drivers_body = "".join(lines[4306:5025])
    with open(os.path.join(out_dir, "drivers.rs"), "w", encoding="utf-8") as f:
        f.write(drivers_header + "\n" + drivers_body)

    # mod.rs: index + get_handler (lignes 5025..5267)
    mod_header = """//! # syscall/table/mod.rs — Table de dispatch O(1) et orchestration des handlers syscall
//!
//! Ce module indexe et expose l'intégralité des handlers de syscalls d'Exo-OS,
//! répartis de manière propre et ségréguée par domaine d'activité :
//! - `common`  : Types ABI, statistiques d'appels, ENOSYS et validateurs d'arguments.
//! - `fs`      : I/O, système de fichiers, descripteurs et VFS.
//! - `memory`  : Allocations virtuelles, mmap, brk, mprotect.
//! - `process` : Cycle de vie des processus et threads, signaux, identités.
//! - `signal`  : Routage et masquage des signaux POSIX.
//! - `time`    : Horloges, temporisateurs, nanosleep et futex.
//! - `ipc`     : IPC natif Exo-OS, capabilities, ExoCordon et ExoShield.
//! - `drivers` : Pilotes GI-03, DMA, bus PCI, vecteurs IRQ et MSI.

pub mod common;
pub mod drivers;
pub mod fs;
pub mod ipc;
pub mod memory;
pub mod process;
pub mod signal;
pub mod time;

pub use common::*;
pub use drivers::*;
pub use fs::*;
pub use ipc::*;
pub use memory::*;
pub use process::*;
pub use signal::*;
pub use time::*;

use crate::syscall::numbers::*;
"""
    mod_body = "".join(lines[5025:])
    with open(os.path.join(out_dir, "mod.rs"), "w", encoding="utf-8") as f:
        f.write(mod_header + "\n" + mod_body)

    print("Découpage terminé avec succès dans kernel/src/syscall/table/")

if __name__ == "__main__":
    run()
