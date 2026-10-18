//! # servers/exosh
#![allow(dead_code, unused_imports)]
use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use exo_syscall_abi as syscall;

use super::*;
use crate::types::*;
use crate::util::*;
use crate::terminal::*;
use crate::exec::*;

pub fn cmd_help() {
    write_all(b"Commands:\n");
    write_all(b"  help cd pwd history time shutdown reboot ping tcping bench exit\n");
    write_all(
        b"  /bin: basename cat clear cp dd dirname echo false grep ipc-stat kill ls meminfo mkdir mv ps pwd rm rmdir sleep stat sync syscall-stat top touch tree true uname uptime wc whoami\n",
    );
    write_all(b"Features:\n");
    write_all(b"  Pipelines:     cmd1 | cmd2\n");
    write_all(b"  Redirections:  cmd > out  ;  cmd >> out  ;  cmd < in\n");
    write_all(b"  Variables:     $? (exit code)  ;  $PWD  ;  $HOME\n");
    write_all(b"Examples:\n");
    write_all(b"  cd - ; ls -lah /tmp ; history\n");
    write_all(b"  cat /etc/hostname | wc\n");
    write_all(b"  ps | grep server\n");
    write_all(b"  echo 'test message' > /tmp/out.txt ; cat /tmp/out.txt\n");
}

pub fn cmd_top() {
    write_all(b"PID  NAME              STATE\n");
    let mut entries = [syscall::ExoProcessInfo::zeroed(); 64];
    let count = process_list_snapshot(&mut entries);
    if count > 0 {
        let mut idx = 0usize;
        while idx < count as usize && idx < entries.len() {
            let entry = entries[idx];
            write_u32(entry.pid);
            if entry.pid < 10 {
                write_all(b"    ");
            } else {
                write_all(b"   ");
            }
            write_padded(process_entry_name(&entry), 17);
            write_process_state(entry.state);
            write_all(b"\n");
            idx += 1;
        }
        return;
    }

    write_all(b"process list unavailable\n");
}

pub fn process_list_snapshot(entries: &mut [syscall::ExoProcessInfo]) -> i64 {
    unsafe {
        syscall::syscall3(
            syscall::SYS_EXO_PROCESS_LIST,
            entries.as_mut_ptr() as u64,
            entries.len() as u64,
            core::mem::size_of::<syscall::ExoProcessInfo>() as u64,
        )
    }
}

pub fn process_entry_name(entry: &syscall::ExoProcessInfo) -> &[u8] {
    let mut len = 0usize;
    while len < entry.name.len() && entry.name[len] != 0 {
        len += 1;
    }
    if len == 0 {
        b"user_process"
    } else {
        &entry.name[..len]
    }
}

pub fn write_process_state(state: u32) {
    match state {
        0 => write_all(b"creating"),
        1 => write_all(b"running"),
        2 => write_all(b"sleeping"),
        3 => write_all(b"stopped"),
        4 => write_all(b"zombie"),
        5 => write_all(b"dead"),
        _ => write_all(b"unknown"),
    }
}

pub fn cmd_time(rest: &[u8], state: &mut ShellState) {
    let command = trim(rest);
    if command.is_empty() {
        write_all(b"time: usage: time <command>\n");
        return;
    }

    let start = monotonic_ns();
    run_command(command, state);
    let end = monotonic_ns();
    match (start, end) {
        (Some(a), Some(b)) if b >= a => {
            write_all(b"real ");
            write_duration_ms(b - a);
            write_all(b"\n");
        }
        _ => write_all(b"real unavailable\n"),
    }
}

pub fn cmd_sleep(rest: &[u8]) {
    let (arg, _) = next_arg(rest);
    if arg.is_empty() {
        write_all(b"sleep: usage: sleep <ms>\n");
        return;
    }
    let Some(ms) = parse_u64(arg) else {
        write_all(b"sleep: invalid milliseconds\n");
        return;
    };
    sleep_ms(ms);
}

pub fn cmd_kill(rest: &[u8]) {
    let (first, tail) = next_arg(rest);
    if first.is_empty() {
        write_all(b"kill: usage: kill [-9] <pid|service>\n");
        return;
    }

    let mut signal = SIGTERM;
    let mut target = first;
    if first.len() > 1 && first[0] == b'-' {
        signal = if bytes_eq(&first[1..], b"9") {
            SIGKILL
        } else {
            match parse_u32(&first[1..]) {
                Some(sig) => sig as u64,
                None => {
                    write_all(b"kill: invalid signal\n");
                    return;
                }
            }
        };
        let (next, _) = next_arg(tail);
        target = next;
    }

    if target.is_empty() {
        write_all(b"kill: missing target\n");
        return;
    }

    let Some(pid) = parse_u32(target).or_else(|| service_pid(target)) else {
        write_all(b"kill: unknown target\n");
        return;
    };
    if pid == 1 {
        write_all(b"kill: refusing to kill init_server (pid 1)\n");
        return;
    }

    let rc = unsafe { syscall::syscall2(syscall::SYS_KILL, pid as u64, signal) };
    if rc < 0 {
        print_errno(b"kill", rc);
    } else {
        write_all(b"kill: signal sent to pid ");
        write_u32(pid);
        write_all(b"\n");
    }
}

pub fn cmd_history(state: &ShellState) {
    let count = state.history_count.min(HISTORY_MAX);
    let mut i = count;
    while i > 0 {
        let offset = i - 1;
        let number = count - offset;
        let mut out = [0u8; LINE_MAX + 16];
        let mut len = 0usize;
        push_u32(&mut out, &mut len, number as u32);
        push_bytes(&mut out, &mut len, b"  ");
        if let Some(line) = state.history_at_offset(offset) {
            push_bytes(&mut out, &mut len, line);
        }
        push_bytes(&mut out, &mut len, b"\n");
        write_bytes(&out[..len]);
        i -= 1;
    }
}

pub fn cmd_meminfo() {
    let mut info = LinuxSysInfo::default();
    let rc =
        unsafe { syscall::syscall1(syscall::SYS_SYSINFO, &mut info as *mut LinuxSysInfo as u64) };
    if rc < 0 {
        print_errno(b"meminfo", rc);
        return;
    }

    let unit = if info.mem_unit == 0 {
        1
    } else {
        info.mem_unit as u64
    };
    let total = info.totalram.saturating_mul(unit);
    let free = info.freeram.saturating_mul(unit);
    let used = total.saturating_sub(free);

    write_all(b"MemTotal: ");
    write_human_u64(total);
    write_all(b"\nMemFree:  ");
    write_human_u64(free);
    write_all(b"\nMemUsed:  ");
    write_human_u64(used);
    write_all(b"\nUptime:   ");
    write_i64(info.uptime);
    write_all(b"s\nProcs:    ");
    write_u64(info.procs as u64);
    write_all(b"\n");
}

pub fn perf_read(metric: u64, index: u64) -> Option<u64> {
    let rc = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, metric, index) };
    if rc < 0 {
        None
    } else {
        Some(rc as u64)
    }
}

pub fn cmd_syscall_stat() {
    let rows = [
        (b"read".as_slice(), syscall::SYS_READ),
        (b"write".as_slice(), syscall::SYS_WRITE),
        (b"open".as_slice(), syscall::SYS_OPEN),
        (b"close".as_slice(), syscall::SYS_CLOSE),
        (b"ipc_send".as_slice(), syscall::SYS_IPC_SEND),
        (b"ipc_recv".as_slice(), syscall::SYS_IPC_RECV),
        (b"sync".as_slice(), syscall::SYS_SYNC),
        (b"reboot".as_slice(), syscall::SYS_REBOOT),
        (b"ioport_read".as_slice(), syscall::SYS_IOPORT_READ),
        (b"ioport_write".as_slice(), syscall::SYS_IOPORT_WRITE),
    ];

    write_all(b"SYSCALL          COUNT\n");
    let mut i = 0usize;
    while i < rows.len() {
        let (name, nr) = rows[i];
        write_padded(name, 16);
        match perf_read(syscall::EXO_PERF_SYSCALL_COUNT, nr) {
            Some(count) => write_u64(count),
            None => write_all(b"unavailable"),
        }
        write_all(b"\n");
        i += 1;
    }
}

pub fn cmd_ipc_stat() {
    let rows = [
        (
            b"messages_sent".as_slice(),
            syscall::EXO_PERF_IPC_MESSAGES_SENT,
        ),
        (
            b"messages_recv".as_slice(),
            syscall::EXO_PERF_IPC_MESSAGES_RECEIVED,
        ),
        (
            b"messages_drop".as_slice(),
            syscall::EXO_PERF_IPC_MESSAGES_DROPPED,
        ),
        (b"rpc_calls".as_slice(), syscall::EXO_PERF_IPC_RPC_CALLS),
        (
            b"rpc_timeouts".as_slice(),
            syscall::EXO_PERF_IPC_RPC_TIMEOUTS,
        ),
        (b"futex_waits".as_slice(), syscall::EXO_PERF_IPC_FUTEX_WAITS),
        (b"futex_wakes".as_slice(), syscall::EXO_PERF_IPC_FUTEX_WAKES),
    ];

    write_all(b"IPC_COUNTER      COUNT\n");
    let mut i = 0usize;
    while i < rows.len() {
        let (name, metric) = rows[i];
        write_padded(name, 16);
        match perf_read(metric, 0) {
            Some(count) => write_u64(count),
            None => write_all(b"unavailable"),
        }
        write_all(b"\n");
        i += 1;
    }
}

pub fn reboot_with_cmd(label: &[u8], cmd: u64) {
    let sync_rc = unsafe { syscall::syscall0(syscall::SYS_SYNC) };
    if sync_rc < 0 {
        print_errno(b"sync", sync_rc);
        return;
    }

    let rc = unsafe {
        syscall::syscall4(
            syscall::SYS_REBOOT,
            syscall::LINUX_REBOOT_MAGIC1,
            syscall::LINUX_REBOOT_MAGIC2,
            cmd,
            0,
        )
    };
    if rc < 0 {
        print_errno(label, rc);
    }
}

pub fn cmd_shutdown() {
    reboot_with_cmd(b"shutdown", syscall::RB_POWER_OFF);
}

pub fn cmd_reboot() {
    reboot_with_cmd(b"reboot", syscall::RB_AUTOBOOT);
}

