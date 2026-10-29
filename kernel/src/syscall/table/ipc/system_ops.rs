//! # syscall/table/ipc/system_ops.rs — Handlers pour journaux kernel, liste des processus, perf et Phoenix

use core::sync::atomic::Ordering;

use crate::syscall::table::common::*;
use super::remote_memory::process_name_eq;
use crate::process::core::pid::Pid;
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::syscall::numbers::*;
use crate::syscall::validation::{copy_to_user, UserBuf};

/// `exo_log(buf_ptr, len, level)` — log direct vers le ring buffer kernel par tranches de 256 octets.
pub fn sys_exo_log(buf_ptr: u64, len: u64, level: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_LOG);
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid > 1 {
        let is_service = crate::security::service_class_of(Pid(caller_pid))
            != crate::security::ServiceClass::Unknown;
        let is_root = PROCESS_REGISTRY
            .find_by_pid(Pid(caller_pid))
            .map(|pcb| {
                let creds = pcb.get_creds();
                creds.is_root() || creds.euid == 0
            })
            .unwrap_or(false);
        if !is_service && !is_root {
            return EPERM;
        }
    }
    let log_len = (len as usize).min(4096);
    if log_len == 0 {
        return 0;
    }
    if let Err(e) = UserBuf::validate(buf_ptr, log_len, 4096) {
        return e.to_errno();
    }
    let mut kbuf = [0u8; 256];
    let mut offset = 0usize;
    while offset < log_len {
        let chunk = (log_len - offset).min(kbuf.len());
        if crate::syscall::validation::copy_from_user(
            kbuf.as_mut_ptr(),
            (buf_ptr + offset as u64) as *const u8,
            chunk,
        )
        .is_err()
        {
            return EFAULT;
        }
        crate::arch::x86_64::terminal::debug_write(&kbuf[..chunk]);
        offset += chunk;
    }
    let _ = level;
    0
}

const EXO_PROCESS_NAME_LEN: usize = 16;
const EXO_PROCESS_LIST_MAX: usize = 4096;

#[repr(C)]
#[derive(Clone, Copy)]
struct ExoProcessInfo {
    pid: u32,
    ppid: u32,
    state: u32,
    threads: u32,
    name: [u8; EXO_PROCESS_NAME_LEN],
    utime_ns: u64,
    stime_ns: u64,
}

/// `exo_process_list(buf, capacity, entry_size)` — snapshot de la registry PCB.
pub fn sys_exo_process_list(
    buf_ptr: u64,
    capacity: u64,
    entry_size: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_PROCESS_LIST);
    let expected = core::mem::size_of::<ExoProcessInfo>();
    if entry_size != expected as u64 {
        return EINVAL;
    }
    if capacity == 0 {
        return PROCESS_REGISTRY.count() as i64;
    }
    if capacity > EXO_PROCESS_LIST_MAX as u64 {
        return E2BIG;
    }
    let capacity = capacity as usize;
    let total_len = match capacity.checked_mul(expected) {
        Some(len) => len,
        None => return E2BIG,
    };
    if UserBuf::validate(buf_ptr, total_len, EXO_PROCESS_LIST_MAX * expected).is_err() {
        return EFAULT;
    }

    let mut written = 0usize;
    let mut fault = false;
    PROCESS_REGISTRY.for_each(|pcb| {
        if written >= capacity || fault {
            return;
        }
        let entry = ExoProcessInfo {
            pid: pcb.pid.0,
            ppid: pcb.ppid.load(Ordering::Acquire),
            state: pcb.state.load(Ordering::Acquire),
            threads: pcb.thread_count.load(Ordering::Acquire),
            name: pcb.name_snapshot(),
            utime_ns: pcb.utime_ns.load(Ordering::Acquire),
            stime_ns: pcb.stime_ns.load(Ordering::Acquire),
        };
        let dst = (buf_ptr as *mut u8).wrapping_add(written * expected);
        let src = &entry as *const ExoProcessInfo as *const u8;
        if copy_to_user(dst, src, expected).is_err() {
            fault = true;
        } else {
            written += 1;
        }
    });

    if fault {
        EFAULT
    } else {
        written as i64
    }
}

#[inline]
fn perf_value(value: u64) -> i64 {
    core::cmp::min(value, i64::MAX as u64) as i64
}

/// `exo_perf_read(metric, index)` — lecture de compteurs kernel bornés.
pub fn sys_exo_perf_read(metric: u64, index: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_PERF_READ);
    match metric {
        EXO_PERF_SYSCALL_COUNT => {
            if index >= SYSCALL_TABLE_SIZE as u64 {
                EINVAL
            } else {
                perf_value(syscall_table_stat(index as usize))
            }
        }
        EXO_PERF_DISPATCH_TOTAL => perf_value(crate::syscall::dispatch_stats().total),
        EXO_PERF_DISPATCH_FAST_PATH => perf_value(crate::syscall::dispatch_stats().fast_path),
        EXO_PERF_DISPATCH_SLOW_PATH => perf_value(crate::syscall::dispatch_stats().slow_path),
        EXO_PERF_DISPATCH_ENOSYS => perf_value(crate::syscall::dispatch_stats().enosys),
        EXO_PERF_IPC_MESSAGES_SENT => perf_value(crate::ipc::ipc_stats_snapshot().messages_sent()),
        EXO_PERF_IPC_MESSAGES_RECEIVED => {
            perf_value(crate::ipc::ipc_stats_snapshot().messages_received())
        }
        EXO_PERF_IPC_MESSAGES_DROPPED => {
            perf_value(crate::ipc::ipc_stats_snapshot().messages_dropped())
        }
        EXO_PERF_IPC_RPC_CALLS => perf_value(crate::ipc::ipc_stats_snapshot().rpc_calls()),
        EXO_PERF_IPC_RPC_TIMEOUTS => perf_value(crate::ipc::ipc_stats_snapshot().rpc_timeouts()),
        EXO_PERF_IPC_FUTEX_WAITS => perf_value(crate::ipc::ipc_stats_snapshot().futex_waits()),
        EXO_PERF_IPC_FUTEX_WAKES => perf_value(crate::ipc::ipc_stats_snapshot().futex_wakes()),
        _ => EINVAL,
    }
}

pub fn sys_exo_perf_enable(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_PERF_ENABLE);
    0
}

pub fn sys_exo_perf_disable(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_PERF_DISABLE);
    0
}

/// `exo_phoenix_state_set(state)` — synchronise une transition Phoenix root.
pub fn sys_exo_phoenix_state_set(
    state: u64,
    _a2: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_PHOENIX_STATE_SET);
    if state > u8::MAX as u64 {
        return EINVAL;
    }
    if !matches!(state as u8, 1 | 9 | 10) {
        return EINVAL;
    }
    let caller = current_pid_u32();
    let Some(pcb) = PROCESS_REGISTRY.find_by_pid(Pid(caller)) else {
        return EPERM;
    };
    if !pcb.is_root() {
        return EPERM;
    }
    let name = pcb.name_snapshot();
    if !process_name_eq(&name, b"network_server") && !process_name_eq(&name, b"init_server") {
        return EPERM;
    }
    if crate::exophoenix::try_set_state_raw(state as u8) {
        0
    } else {
        EINVAL
    }
}

/// `exo_phoenix_state_get()` — retourne l'état Phoenix courant.
pub fn sys_exo_phoenix_state_get(
    _a1: u64,
    _a2: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_PHOENIX_STATE_GET);
    crate::exophoenix::state() as u8 as i64
}
