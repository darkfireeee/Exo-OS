//! Navigation and basic file builtins (`cd`, `pwd`, `mkdir`, `touch`, `cat`, `echo`, `rmdir`, `sync`).
#![allow(dead_code, unused_imports)]
use exo_syscall_abi as syscall;
use crate::builtins::*;
use crate::types::*;
use crate::util::*;
use crate::terminal::*;
use crate::exec::*;

pub fn cmd_cd(rest: &[u8], state: &mut ShellState) {
    let (arg, _) = next_arg(rest);
    let mut path = [0u8; PATH_MAX];
    let len = if bytes_eq(arg, b"-") {
        if state.old_cwd_len == 0 {
            write_all(b"cd: no previous directory\n");
            return;
        }
        path[..state.old_cwd_len].copy_from_slice(&state.old_cwd[..state.old_cwd_len]);
        write_bytes(&path[..state.old_cwd_len]);
        write_all(b"\n");
        state.old_cwd_len
    } else {
        let target = if arg.is_empty() { b"/".as_slice() } else { arg };
        match absolute_path(state.cwd(), target, &mut path) {
            Some(l) => l,
            None => {
                write_all(b"cd: path too long\n");
                return;
            }
        }
    };

    let rc = unsafe { syscall::syscall1(syscall::SYS_CHDIR, path.as_ptr() as u64) };
    if rc == 0 {
        state.record_old_cwd();
        state.set_cwd(&path[..len]);
    } else {
        print_errno(b"cd", rc);
    }
}

pub fn cmd_pwd(state: &ShellState) {
    write_bytes(state.cwd());
    write_all(b"\n");
}


pub fn cmd_mkdir(rest: &[u8], state: &ShellState) {
    let (arg, _) = next_arg(rest);
    if arg.is_empty() {
        write_all(b"mkdir: missing path\n");
        return;
    }
    let mut path = [0u8; PATH_MAX];
    if absolute_path(state.cwd(), arg, &mut path).is_none() {
        write_all(b"mkdir: path too long\n");
        return;
    }
    let rc = unsafe { syscall::syscall2(syscall::SYS_MKDIR, path.as_ptr() as u64, 0o755) };
    if rc < 0 {
        print_errno(b"mkdir", rc);
    }
}

pub fn cmd_touch(rest: &[u8], state: &ShellState) {
    let mut args = rest;
    if trim(args).is_empty() {
        write_all(b"touch: missing path\n");
        return;
    }

    loop {
        let (arg, tail) = next_arg(args);
        if arg.is_empty() {
            break;
        }
        touch_one_arg(arg, state);
        args = tail;
    }
}

pub fn touch_one_arg(arg: &[u8], state: &ShellState) {
    let mut path = [0u8; PATH_MAX];
    if absolute_path(state.cwd(), arg, &mut path).is_none() {
        write_all(b"touch: path too long\n");
        return;
    }
    let fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            path.as_ptr() as u64,
            syscall::O_CREAT | syscall::O_RDWR,
            0o644,
        )
    };
    if fd < 0 {
        print_errno(b"touch", fd);
    } else {
        let _ = close_fd(fd);
    }
}

pub fn cmd_cat(rest: &[u8], state: &ShellState) {
    let (arg, _) = next_arg(rest);
    if arg.is_empty() {
        write_all(b"cat: missing path\n");
        return;
    }
    let mut path = [0u8; PATH_MAX];
    if absolute_path(state.cwd(), arg, &mut path).is_none() {
        write_all(b"cat: path too long\n");
        return;
    }
    let fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            path.as_ptr() as u64,
            syscall::O_RDONLY,
            0,
        )
    };
    if fd < 0 {
        print_errno(b"cat", fd);
        return;
    }

    let mut buf = [0u8; 512];
    loop {
        let n = unsafe {
            syscall::syscall3(
                syscall::SYS_READ,
                fd as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        if n == 0 {
            break;
        }
        if n < 0 {
            print_errno(b"cat", n);
            break;
        }
        write_bytes(&buf[..n as usize]);
    }
    let _ = close_fd(fd);
}

pub fn cmd_echo(rest: &[u8], state: &ShellState) {
    let body = trim(rest);
    if let Some(redir) = find_byte(body, b'>') {
        let is_append = redir + 1 < body.len() && body[redir + 1] == b'>';
        let text = trim_end(&body[..redir]);
        let path_start = if is_append { redir + 2 } else { redir + 1 };
        let path_arg = trim(&body[path_start..]);
        if path_arg.is_empty() {
            write_all(b"echo: missing redirection path\n");
            return;
        }
        let mut path = [0u8; PATH_MAX];
        if absolute_path(state.cwd(), path_arg, &mut path).is_none() {
            write_all(b"echo: path too long\n");
            return;
        }
        let flags = if is_append {
            syscall::O_CREAT | syscall::O_WRONLY | syscall::O_APPEND
        } else {
            syscall::O_CREAT | syscall::O_WRONLY | syscall::O_TRUNC
        };
        let fd = unsafe {
            syscall::syscall4(
                syscall::SYS_OPENAT,
                AT_FDCWD,
                path.as_ptr() as u64,
                flags,
                0o644,
            )
        };
        if fd < 0 {
            print_errno(b"echo", fd);
            return;
        }
        let rc = write_fd_all(fd, text);
        if rc >= 0 {
            let _ = write_fd_all(fd, b"\n");
        } else {
            print_errno(b"echo", rc);
        }
        let _ = close_fd(fd);
    } else {
        write_bytes(body);
        write_all(b"\n");
    }
}


pub fn cmd_rmdir(rest: &[u8], state: &ShellState) {
    let (arg, _) = next_arg(rest);
    if arg.is_empty() {
        write_all(b"rmdir: missing path\n");
        return;
    }
    let mut path = [0u8; PATH_MAX];
    if absolute_path(state.cwd(), arg, &mut path).is_none() {
        write_all(b"rmdir: path too long\n");
        return;
    }
    let rc = unsafe { syscall::syscall1(syscall::SYS_RMDIR, path.as_ptr() as u64) };
    if rc < 0 {
        print_errno(b"rmdir", rc);
    }
}


pub fn cmd_sync() {
    let rc = unsafe { syscall::syscall0(syscall::SYS_SYNC) };
    if rc < 0 {
        print_errno(b"sync", rc);
    }
}

