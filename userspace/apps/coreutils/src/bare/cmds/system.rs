#![allow(unused_imports, dead_code)]
use crate::bare::types::*;
use crate::bare::util::*;
use exo_syscall_abi as syscall;

    pub fn cmd_sleep(args: &Args) -> i32 {
        if args.len() < 2 {
            return print_errno(b"sleep", -22);
        }
        let Some(ms) = parse_u64(args.get(1)) else {
            return print_errno(b"sleep", -22);
        };
        let ts = LinuxTimespec {
            tv_sec: (ms / 1000) as i64,
            tv_nsec: ((ms % 1000) * 1_000_000) as i64,
        };
        let _ = unsafe { syscall::syscall2(syscall::SYS_NANOSLEEP, &ts as *const _ as u64, 0) };
        0
    }

    fn sysinfo() -> Option<LinuxSysInfo> {
        let mut info = LinuxSysInfo::default();
        let rc =
            unsafe { syscall::syscall1(syscall::SYS_SYSINFO, &mut info as *mut _ as u64) };
        if rc < 0 {
            None
        } else {
            Some(info)
        }
    }

    fn write_human(value: u64) {
        let mut v = value;
        let units = [b'B', b'K', b'M', b'G', b'T'];
        let mut unit = 0usize;
        while v >= 1024 && unit + 1 < units.len() {
            v = v.saturating_add(512) / 1024;
            unit += 1;
        }
        write_u64(STDOUT, v);
        write_byte(STDOUT, units[unit]);
    }

    pub fn cmd_meminfo(_args: &Args) -> i32 {
        let Some(info) = sysinfo() else {
            return print_errno(b"meminfo", -38);
        };
        let unit = if info.mem_unit == 0 { 1 } else { info.mem_unit as u64 };
        write_all(STDOUT, b"MemTotal: ");
        write_human(info.totalram.saturating_mul(unit));
        write_all(STDOUT, b"\nMemFree:  ");
        write_human(info.freeram.saturating_mul(unit));
        write_all(STDOUT, b"\nProcs:    ");
        write_u64(STDOUT, info.procs as u64);
        write_byte(STDOUT, b'\n');
        0
    }

    fn perf(metric: u64, index: u64) -> Option<u64> {
        let rc = unsafe { syscall::syscall2(syscall::SYS_EXO_PERF_READ, metric, index) };
        if rc < 0 {
            None
        } else {
            Some(rc as u64)
        }
    }

    pub fn cmd_syscall_stat(_args: &Args) -> i32 {
        let rows = [
            (b"read".as_slice(), syscall::SYS_READ),
            (b"write".as_slice(), syscall::SYS_WRITE),
            (b"open".as_slice(), syscall::SYS_OPEN),
            (b"execve".as_slice(), syscall::SYS_EXECVE),
            (b"wait4".as_slice(), syscall::SYS_WAIT4),
            (b"sync".as_slice(), syscall::SYS_SYNC),
        ];
        let mut i = 0usize;
        while i < rows.len() {
            write_all(STDOUT, rows[i].0);
            write_all(STDOUT, b" ");
            match perf(syscall::EXO_PERF_SYSCALL_COUNT, rows[i].1) {
                Some(v) => write_u64(STDOUT, v),
                None => write_all(STDOUT, b"unavailable"),
            }
            write_byte(STDOUT, b'\n');
            i += 1;
        }
        0
    }

    pub fn cmd_ipc_stat(_args: &Args) -> i32 {
        let rows = [
            (b"messages_sent".as_slice(), syscall::EXO_PERF_IPC_MESSAGES_SENT),
            (b"messages_recv".as_slice(), syscall::EXO_PERF_IPC_MESSAGES_RECEIVED),
            (b"messages_drop".as_slice(), syscall::EXO_PERF_IPC_MESSAGES_DROPPED),
            (b"rpc_calls".as_slice(), syscall::EXO_PERF_IPC_RPC_CALLS),
        ];
        let mut i = 0usize;
        while i < rows.len() {
            write_all(STDOUT, rows[i].0);
            write_all(STDOUT, b" ");
            match perf(rows[i].1, 0) {
                Some(v) => write_u64(STDOUT, v),
                None => write_all(STDOUT, b"unavailable"),
            }
            write_byte(STDOUT, b'\n');
            i += 1;
        }
        0
    }

    pub fn cmd_ps(_args: &Args) -> i32 {
        let mut entries = [syscall::ExoProcessInfo::zeroed(); 64];
        let rc = unsafe {
            syscall::syscall2(
                syscall::SYS_EXO_PROCESS_LIST,
                entries.as_mut_ptr() as u64,
                entries.len() as u64,
            )
        };
        if rc < 0 {
            return print_errno(b"ps", rc);
        }
        write_all(STDOUT, b"PID PPID NAME\n");
        let mut i = 0usize;
        while i < rc as usize && i < entries.len() {
            write_u64(STDOUT, entries[i].pid as u64);
            write_byte(STDOUT, b' ');
            write_u64(STDOUT, entries[i].ppid as u64);
            write_byte(STDOUT, b' ');
            let mut end = 0usize;
            while end < entries[i].name.len() && entries[i].name[end] != 0 {
                end += 1;
            }
            write_all(STDOUT, &entries[i].name[..end]);
            write_byte(STDOUT, b'\n');
            i += 1;
        }
        0
    }

    pub fn cmd_top(args: &Args) -> i32 {
        cmd_ps(args)
    }

    pub fn cmd_kill(args: &Args) -> i32 {
        if args.len() < 2 {
            return print_errno(b"kill", -22);
        }
        let mut sig = SIGTERM;
        let mut target = args.get(1);
        if target.starts_with(b"-") && args.len() > 2 {
            sig = parse_u64(&target[1..]).unwrap_or(SIGTERM);
            target = args.get(2);
        }
        let Some(pid) = parse_i32(target) else {
            return print_errno(b"kill", -22);
        };
        let rc = unsafe { syscall::syscall2(syscall::SYS_KILL, pid as u64, sig) };
        if rc < 0 {
            print_errno(b"kill", rc)
        } else {
            0
        }
    }


    pub fn cmd_uptime(_args: &Args) -> i32 {
        let Some(info) = sysinfo() else {
            return print_errno(b"uptime", -38);
        };
        write_all(STDOUT, b"up ");
        write_i64(STDOUT, info.uptime);
        write_all(STDOUT, b" seconds\n");
        0
    }

    pub fn cmd_whoami(_args: &Args) -> i32 {
        let uid = unsafe { syscall::syscall0(syscall::SYS_GETUID) };
        if uid == 0 {
            write_all(STDOUT, b"root\n");
        } else {
            write_all(STDOUT, b"uid");
            write_i64(STDOUT, uid);
            write_byte(STDOUT, b'\n');
        }
        0
    }

    pub fn cmd_uname(_args: &Args) -> i32 {
        let mut uts = LinuxUtsname::default();
        let rc = unsafe { syscall::syscall1(syscall::SYS_UNAME, &mut uts as *mut _ as u64) };
        if rc < 0 {
            return print_errno(b"uname", rc);
        }
        let mut end = 0usize;
        while end < uts.sysname.len() && uts.sysname[end] != 0 {
            end += 1;
        }
        write_all(STDOUT, &uts.sysname[..end]);
        write_all(STDOUT, b" ");
        end = 0;
        while end < uts.machine.len() && uts.machine[end] != 0 {
            end += 1;
        }
        write_all(STDOUT, &uts.machine[..end]);
        write_byte(STDOUT, b'\n');
        0
    }

    pub fn cmd_clear(_args: &Args) -> i32 {
        write_byte(STDOUT, 0x0c);
        0
    }

    pub fn cmd_true(_args: &Args) -> i32 {
        0
    }

    pub fn cmd_false(_args: &Args) -> i32 {
        1
    }

