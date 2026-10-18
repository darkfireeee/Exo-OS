//! # syscall/table/common.rs — Types communs, statistiques et validateurs d'arguments

use crate::syscall::numbers::{EINVAL, ENOSYS, SYSCALL_TABLE_SIZE};
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

#[inline(always)]
pub fn current_pid_u32() -> u32 {
    let tcb_ptr: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nostack, nomem));
    }
    if tcb_ptr == 0 {
        return 0;
    }
    unsafe {
        (*(tcb_ptr as *const crate::scheduler::core::task::ThreadControlBlock))
            .pid
            .0
    }
}

