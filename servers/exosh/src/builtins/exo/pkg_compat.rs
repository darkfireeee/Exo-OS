//! `exo compat`, `exo install`, `exo remove`, `exo list`, `exo search`.
#![allow(dead_code, unused_imports)]
use exo_syscall_abi as syscall;
use crate::builtins::*;
use crate::types::*;
use crate::util::*;
use crate::terminal::*;

fn parse_compat_policy_promises() -> u64 {
    // Minimal compat default matching `kernel/src/security/isolation/pledge.rs`:
    // STDIO(0) | RPATH(1) | WPATH(2) | CPATH(3) | TMPPATH(4) | UNIX(6) | EXEC(10) |
    // SHM(13) | SIGNAL(14) | TTY(15) | FUTEX(16) = 0x1e45f.
    // Explicitly omits INET(5), DNS(7), PROC(9), ID(11), ROUTE(12), UNIX_ABSTRACT(17), DLOPEN(18).
    const DEFAULT_COMPAT_PROMISES: u64 = (1 << 0)
        | (1 << 1)
        | (1 << 2)
        | (1 << 3)
        | (1 << 4)
        | (1 << 6)
        | (1 << 10)
        | (1 << 13)
        | (1 << 14)
        | (1 << 15)
        | (1 << 16);

    let fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            b"/etc/exoshield/compat_default.policy\0".as_ptr() as u64,
            syscall::O_RDONLY,
            0,
        )
    };
    if fd < 0 {
        return DEFAULT_COMPAT_PROMISES;
    }
    let mut buf = [0u8; 512];
    let n = unsafe {
        syscall::syscall3(
            syscall::SYS_READ,
            fd as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    let _ = close_fd(fd);
    if n <= 0 {
        return DEFAULT_COMPAT_PROMISES;
    }
    let slice = &buf[..n as usize];
    // Parse optional `promises=0x...` line while never permitting privileged Ring 1 promises.
    let mut i = 0usize;
    while i < slice.len() {
        let start = i;
        while i < slice.len() && slice[i] != b'\n' {
            i += 1;
        }
        let line = trim(&slice[start..i]);
        if let Some(hex_str) = line.strip_prefix(b"promises=0x") {
            let mut val = 0u64;
            let mut valid = !hex_str.is_empty();
            for &b in hex_str {
                let d = match b {
                    b'0'..=b'9' => (b - b'0') as u64,
                    b'a'..=b'f' => (b - b'a' + 10) as u64,
                    b'A'..=b'F' => (b - b'A' + 10) as u64,
                    _ => {
                        valid = false;
                        break;
                    }
                };
                val = (val << 4) | d;
            }
            if valid && val != 0 {
                // Always retain EXEC (1<<10) so execve succeeds and never grant ID(11)/ROUTE(12).
                let forbidden: u64 = (1 << 11) | (1 << 12);
                return (val | (1 << 10)) & !forbidden;
            }
        }
        i = i.saturating_add(1);
    }
    DEFAULT_COMPAT_PROMISES
}

pub(super) fn handle_exo_compat(args: &[u8], state: &mut ShellState) {
    let (subcmd, rest) = first_token(args);
    if bytes_eq(subcmd, b"install") {
        handle_exo_install(rest, true);
    } else if bytes_eq(subcmd, b"run") {
        let cmdline = trim(rest);
        if cmdline.is_empty() {
            write_all(b"Usage: exo compat run <binary> [args...]\n");
            return;
        }
        // S2.3: Confine via SYS_EXO_PLEDGE (324) with minimal compat policy +
        // NO_RING1_CRITICAL_IPC flag (0x2) before execve.
        let promises = parse_compat_policy_promises();
        const PLEDGE_FLAG_NO_RING1_CRITICAL_IPC: u64 = 0x2;
        if let Err(()) =
            crate::exec::run_external_with_pledge(cmdline, state, Some((promises, PLEDGE_FLAG_NO_RING1_CRITICAL_IPC)))
        {
            let (pkg, _) = first_token(cmdline);
            write_all(b"exo compat run: binary not found: ");
            write_bytes(pkg);
            write_all(b"\n");
        }
    } else {
        write_all(b"Usage: exo compat [install|run] <pkg> [args...]\n");
    }
}

pub(super) fn handle_exo_install(args: &[u8], is_compat: bool) {
    let (pkg_name, _) = first_token(args);
    if pkg_name.is_empty() {
        if is_compat {
            write_all(b"Usage: exo compat install <package>\n");
        } else {
            write_all(b"Usage: exo install <package>\n");
        }
        return;
    }
    write_all(b"exo install: package manager backend not implemented for '");
    write_bytes(pkg_name);
    write_all(b"' (ENOSYS)\n");
}

pub(super) fn handle_exo_remove(args: &[u8]) {
    let (pkg_name, _) = first_token(args);
    if pkg_name.is_empty() {
        write_all(b"Usage: exo remove <package>\n");
        return;
    }
    write_all(b"exo remove: package manager backend not implemented for '");
    write_bytes(pkg_name);
    write_all(b"' (ENOSYS)\n");
}

fn list_dir_entries_brief(dir_cstr: &[u8], label: &[u8]) {
    write_bytes(label);
    write_all(b":\n");
    let fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            dir_cstr.as_ptr() as u64,
            syscall::O_RDONLY,
            0,
        )
    };
    if fd < 0 {
        write_all(b"  (unavailable)\n");
        return;
    }
    let mut buf = [0u8; IO_BUF];
    loop {
        let n = unsafe {
            syscall::syscall3(
                syscall::SYS_GETDENTS64,
                fd as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        if n <= 0 {
            break;
        }
        let slice = &buf[..n as usize];
        let mut off = 0usize;
        while off + DIRENT64_HEADER_SIZE <= slice.len() {
            let reclen = u16::from_le_bytes([slice[off + 16], slice[off + 17]]) as usize;
            if reclen == 0 || off + reclen > slice.len() {
                break;
            }
            let start = off + DIRENT64_HEADER_SIZE;
            let mut end = start;
            while end < off + reclen && slice[end] != 0 {
                end += 1;
            }
            let name = &slice[start..end];
            if !name.is_empty() && !bytes_eq(name, b".") && !bytes_eq(name, b"..") {
                write_all(b"  ");
                write_bytes(name);
                write_all(b"\n");
            }
            off += reclen;
        }
    }
    let _ = close_fd(fd);
}

pub(super) fn list_installed_packages() {
    write_all(b"Installed Executables on ExoFS:\n");
    list_dir_entries_brief(b"/bin\0", b"/bin");
    list_dir_entries_brief(b"/sbin\0", b"/sbin");
}

pub(super) fn search_packages(args: &[u8]) {
    let (term, _) = first_token(args);
    write_all(b"exo search: remote package registry not configured for '");
    write_bytes(term);
    write_all(b"' (ENOSYS)\n");
}

