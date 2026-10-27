//! # servers/exosh
#![allow(dead_code, unused_imports)]
use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use exo_syscall_abi as syscall;

use crate::types::*;
use crate::util::*;
use crate::terminal::*;
use crate::builtins::*;

pub fn run_pipeline(left: &[u8], right: &[u8], state: &mut ShellState) {
    let mut fds = [0i32; 2];
    let pipe_rc = unsafe { syscall::syscall2(syscall::SYS_PIPE2, fds.as_mut_ptr() as u64, 0) };
    if pipe_rc < 0 {
        print_errno(b"pipe", pipe_rc);
        return;
    }
    let p_read = fds[0] as u64;
    let p_write = fds[1] as u64;

    let child_left = unsafe { syscall::syscall0(syscall::SYS_FORK) };
    if child_left < 0 {
        print_errno(b"fork", child_left);
        unsafe {
            let _ = syscall::syscall1(syscall::SYS_CLOSE, p_read);
            let _ = syscall::syscall1(syscall::SYS_CLOSE, p_write);
        }
        return;
    }

    if child_left == 0 {
        restore_sigint();
        unsafe {
            let _ = syscall::syscall2(syscall::SYS_DUP2, p_write, STDOUT);
            let _ = syscall::syscall1(syscall::SYS_CLOSE, p_read);
            let _ = syscall::syscall1(syscall::SYS_CLOSE, p_write);
        }
        run_command(left, state);
        unsafe {
            let _ = syscall::syscall1(syscall::SYS_EXIT, 0);
        }
        loop {
            core::hint::spin_loop();
        }
    }

    let child_right = unsafe { syscall::syscall0(syscall::SYS_FORK) };
    if child_right < 0 {
        print_errno(b"fork", child_right);
        unsafe {
            let _ = syscall::syscall1(syscall::SYS_CLOSE, p_read);
            let _ = syscall::syscall1(syscall::SYS_CLOSE, p_write);
            // Tue et récolte child_left pour éviter qu'il ne reste bloqué sur le pipe orphelin
            let _ = syscall::syscall2(syscall::SYS_KILL, child_left as u64, 9);
            let mut status: i32 = 0;
            let _ = syscall::syscall4(
                syscall::SYS_WAIT4,
                child_left as u64,
                &mut status as *mut i32 as u64,
                0,
                0,
            );
        }
        return;
    }

    if child_right == 0 {
        restore_sigint();
        unsafe {
            let _ = syscall::syscall2(syscall::SYS_DUP2, p_read, STDIN);
            let _ = syscall::syscall1(syscall::SYS_CLOSE, p_read);
            let _ = syscall::syscall1(syscall::SYS_CLOSE, p_write);
        }
        run_command(right, state);
        unsafe {
            let _ = syscall::syscall1(syscall::SYS_EXIT, 0);
        }
        loop {
            core::hint::spin_loop();
        }
    }

    unsafe {
        let _ = syscall::syscall1(syscall::SYS_CLOSE, p_read);
        let _ = syscall::syscall1(syscall::SYS_CLOSE, p_write);
    }

    set_tty_foreground(child_right as u32);
    let mut st1 = 0i32;
    let mut st2 = 0i32;
    unsafe {
        let _ = syscall::syscall4(
            syscall::SYS_WAIT4,
            child_left as u64,
            &mut st1 as *mut i32 as u64,
            0,
            0,
        );
        let _ = syscall::syscall4(
            syscall::SYS_WAIT4,
            child_right as u64,
            &mut st2 as *mut i32 as u64,
            0,
            0,
        );
    }
    let self_pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) } as u32;
    set_tty_foreground(self_pid);
    state.last_status = if (st2 & 0x7f) == 0 { (st2 >> 8) & 0xff } else { 128 + (st2 & 0x7f) };
}

pub fn run_commands(line: &[u8], state: &mut ShellState) {
    let mut start = 0usize;
    let mut i = 0usize;
    while i <= line.len() {
        if i == line.len() || line[i] == b';' {
            let part = trim(&line[start..i]);
            if !part.is_empty() {
                if let Some(pipe_pos) = find_byte(part, b'|') {
                    let left = trim(&part[..pipe_pos]);
                    let right = trim(&part[pipe_pos + 1..]);
                    if !left.is_empty() && !right.is_empty() {
                        run_pipeline(left, right, state);
                    } else {
                        write_all(b"exosh: syntax error near unexpected token '|'\n");
                    }
                } else {
                    run_command(part, state);
                }
            }
            start = i.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
}

pub fn run_command(line: &[u8], state: &mut ShellState) {
    let (cmd, rest) = first_token(line);
    if bytes_eq(cmd, b"help") {
        cmd_help();
    } else if bytes_eq(cmd, b"exo") {
        cmd_exo(rest, state);
    } else if bytes_eq(cmd, b"pwd") {
        cmd_pwd(state);
    } else if bytes_eq(cmd, b"cd") {
        cmd_cd(rest, state);
    } else if bytes_eq(cmd, b"ls") {
        cmd_ls(rest, state);
    } else if bytes_eq(cmd, b"mkdir") {
        cmd_mkdir(rest, state);
    } else if bytes_eq(cmd, b"touch") {
        cmd_touch(rest, state);
    } else if bytes_eq(cmd, b"cat") {
        cmd_cat(rest, state);
    } else if bytes_eq(cmd, b"echo") {
        cmd_echo(rest, state);
    } else if bytes_eq(cmd, b"rm") {
        cmd_rm(rest, state);
    } else if bytes_eq(cmd, b"cp") {
        cmd_cp(rest, state);
    } else if bytes_eq(cmd, b"mv") {
        cmd_mv(rest, state);
    } else if bytes_eq(cmd, b"rmdir") {
        cmd_rmdir(rest, state);
    } else if bytes_eq(cmd, b"tree") {
        cmd_tree(rest, state);
    } else if bytes_eq(cmd, b"stat") {
        cmd_stat(rest, state);
    } else if bytes_eq(cmd, b"history") {
        cmd_history(state);
    } else if bytes_eq(cmd, b"time") {
        cmd_time(rest, state);
    } else if bytes_eq(cmd, b"sleep") {
        cmd_sleep(rest);
    } else if bytes_eq(cmd, b"dd") {
        cmd_dd(rest, state);
    } else if bytes_eq(cmd, b"sync") {
        cmd_sync();
    } else if bytes_eq(cmd, b"meminfo") {
        cmd_meminfo();
    } else if bytes_eq(cmd, b"syscall-stat") {
        cmd_syscall_stat();
    } else if bytes_eq(cmd, b"ipc-stat") {
        cmd_ipc_stat();
    } else if bytes_eq(cmd, b"grep") {
        run_external_or_report(line, state);
    } else if bytes_eq(cmd, b"shutdown") || bytes_eq(cmd, b"poweroff") {
        cmd_shutdown();
    } else if bytes_eq(cmd, b"reboot") {
        cmd_reboot();
    } else if bytes_eq(cmd, b"ping") {
        cmd_ping(rest);
    } else if bytes_eq(cmd, b"tcping") {
        cmd_tcping(rest);
    } else if bytes_eq(cmd, b"bench") {
        cmd_bench(rest);
    } else if bytes_eq(cmd, b"top") || bytes_eq(cmd, b"ps") {
        cmd_top();
    } else if bytes_eq(cmd, b"kill") {
        cmd_kill(rest);
    } else if bytes_eq(cmd, b"clear") {
        write_all(b"\x0c");
    } else if bytes_eq(cmd, b"beep") {
        cmd_beep(rest);
    } else if bytes_eq(cmd, b"chime") {
        cmd_chime(rest);
    } else if bytes_eq(cmd, b"calendar") || bytes_eq(cmd, b"cal") || bytes_eq(cmd, b"exo-calendar") {
        cmd_calendar(rest);
    } else if bytes_eq(cmd, b"exit") {
        write_all(b"exosh: exit requested; init_server may restart the shell\n");
        unsafe {
            let _ = syscall::syscall1(syscall::SYS_EXIT, 0);
        }
    } else {
        run_external_or_report(line, state);
    }
}

pub fn run_external_or_report(line: &[u8], state: &mut ShellState) {
    if let Err(()) = run_external(line, state) {
        let (cmd, _) = first_token(line);
        write_all(b"exosh: command not found: ");
        write_bytes(cmd);
        write_all(b"\n");
    }
}

pub fn run_external(line: &[u8], state: &mut ShellState) -> Result<(), ()> {
    let mut arg_storage = [[0u8; EXEC_ARG_MAX]; EXEC_MAX_ARGS];
    let mut argv = [0u64; EXEC_MAX_ARGS + 1];
    let mut argc = 0usize;

    let mut stdin_file = [0u8; PATH_MAX];
    let mut stdout_file = [0u8; PATH_MAX];
    let mut stderr_file = [0u8; PATH_MAX];
    let mut has_stdin = false;
    let mut has_stdout = false;
    let mut stdout_append = false;
    let mut has_stderr = false;

    let mut rest = line;
    loop {
        let (arg, tail) = next_arg(rest);
        if arg.is_empty() {
            break;
        }

        if bytes_eq(arg, b">") || bytes_eq(arg, b">>") {
            let (target, after_target) = next_arg(tail);
            if target.is_empty() {
                write_all(b"exosh: syntax error near unexpected token 'newline'\n");
                return Ok(());
            }
            let mut resolved = [0u8; PATH_MAX];
            if let Some(rlen) = absolute_path(state.cwd(), target, &mut resolved) {
                stdout_file[..rlen].copy_from_slice(&resolved[..rlen]);
                stdout_file[rlen] = 0;
                has_stdout = true;
                stdout_append = bytes_eq(arg, b">>");
            }
            rest = after_target;
            continue;
        } else if bytes_eq(arg, b"<") {
            let (target, after_target) = next_arg(tail);
            if target.is_empty() {
                write_all(b"exosh: syntax error near unexpected token 'newline'\n");
                return Ok(());
            }
            let mut resolved = [0u8; PATH_MAX];
            if let Some(rlen) = absolute_path(state.cwd(), target, &mut resolved) {
                stdin_file[..rlen].copy_from_slice(&resolved[..rlen]);
                stdin_file[rlen] = 0;
                has_stdin = true;
            }
            rest = after_target;
            continue;
        } else if bytes_eq(arg, b"2>") {
            let (target, after_target) = next_arg(tail);
            if target.is_empty() {
                write_all(b"exosh: syntax error near unexpected token 'newline'\n");
                return Ok(());
            }
            let mut resolved = [0u8; PATH_MAX];
            if let Some(rlen) = absolute_path(state.cwd(), target, &mut resolved) {
                stderr_file[..rlen].copy_from_slice(&resolved[..rlen]);
                stderr_file[rlen] = 0;
                has_stderr = true;
            }
            rest = after_target;
            continue;
        }

        if argc == EXEC_MAX_ARGS {
            write_all(b"exosh: too many arguments\n");
            return Ok(());
        }

        // Expansion de variables ($?, $PWD, $HOME)
        if bytes_eq(arg, b"$?") {
            let mut num_buf = [0u8; 16];
            let nlen = u64_to_str(state.last_status as u64, &mut num_buf);
            arg_storage[argc][..nlen].copy_from_slice(&num_buf[..nlen]);
            arg_storage[argc][nlen] = 0;
        } else if bytes_eq(arg, b"$PWD") {
            let cwd = state.cwd();
            let nlen = cwd.len().min(EXEC_ARG_MAX - 1);
            arg_storage[argc][..nlen].copy_from_slice(&cwd[..nlen]);
            arg_storage[argc][nlen] = 0;
        } else if bytes_eq(arg, b"$HOME") {
            arg_storage[argc][0] = b'/';
            arg_storage[argc][1] = 0;
        } else {
            let n = arg.len().min(EXEC_ARG_MAX - 1);
            arg_storage[argc][..n].copy_from_slice(&arg[..n]);
            arg_storage[argc][n] = 0;
        }

        argv[argc] = arg_storage[argc].as_ptr() as u64;
        argc += 1;
        rest = tail;
    }
    if argc == 0 {
        return Ok(());
    }

    let mut exec_path = [0u8; PATH_MAX];
    let mut fallback_path = [0u8; PATH_MAX];
    let Some(exec_len) =
        external_command_path(&arg_storage[0], state, &mut exec_path, &mut fallback_path)
    else {
        return Err(());
    };

    let mut pwd_env = [0u8; PATH_MAX + 5];
    pwd_env[..4].copy_from_slice(b"PWD=");
    let cwd = state.cwd();
    let cwd_len = cwd.len().min(PATH_MAX);
    pwd_env[4..4 + cwd_len].copy_from_slice(&cwd[..cwd_len]);
    pwd_env[4 + cwd_len] = 0;
    let envp = [pwd_env.as_ptr() as u64, 0u64];

    let child = unsafe { syscall::syscall0(syscall::SYS_FORK) };
    if child < 0 {
        print_errno(b"fork", child);
        return Ok(());
    }
    if child == 0 {
        restore_sigint();
        // Appliquer redirections
        if has_stdin {
            let fd = unsafe {
                syscall::syscall4(
                    syscall::SYS_OPENAT,
                    AT_FDCWD,
                    stdin_file.as_ptr() as u64,
                    syscall::O_RDONLY,
                    0,
                )
            };
            if fd >= 0 {
                unsafe {
                    let _ = syscall::syscall2(syscall::SYS_DUP2, fd as u64, STDIN);
                    let _ = syscall::syscall1(syscall::SYS_CLOSE, fd as u64);
                }
            }
        }
        if has_stdout {
            let flags = syscall::O_CREAT
                | syscall::O_WRONLY
                | if stdout_append {
                    syscall::O_APPEND
                } else {
                    syscall::O_TRUNC
                };
            let fd = unsafe {
                syscall::syscall4(
                    syscall::SYS_OPENAT,
                    AT_FDCWD,
                    stdout_file.as_ptr() as u64,
                    flags,
                    0o644,
                )
            };
            if fd >= 0 {
                unsafe {
                    let _ = syscall::syscall2(syscall::SYS_DUP2, fd as u64, STDOUT);
                    let _ = syscall::syscall1(syscall::SYS_CLOSE, fd as u64);
                }
            }
        }
        if has_stderr {
            let flags = syscall::O_CREAT | syscall::O_WRONLY | syscall::O_TRUNC;
            let fd = unsafe {
                syscall::syscall4(
                    syscall::SYS_OPENAT,
                    AT_FDCWD,
                    stderr_file.as_ptr() as u64,
                    flags,
                    0o644,
                )
            };
            if fd >= 0 {
                unsafe {
                    let _ = syscall::syscall2(syscall::SYS_DUP2, fd as u64, STDERR);
                    let _ = syscall::syscall1(syscall::SYS_CLOSE, fd as u64);
                }
            }
        }

        let mut rc = unsafe {
            syscall::syscall3(
                syscall::SYS_EXECVE,
                exec_path.as_ptr() as u64,
                argv.as_ptr() as u64,
                envp.as_ptr() as u64,
            )
        };
        if rc < 0 && fallback_path[0] != 0 {
            rc = unsafe {
                syscall::syscall3(
                    syscall::SYS_EXECVE,
                    fallback_path.as_ptr() as u64,
                    argv.as_ptr() as u64,
                    envp.as_ptr() as u64,
                )
            };
        }
        write_all(b"execve: ");
        write_bytes(&exec_path[..exec_len]);
        write_all(b": errno ");
        write_i64(rc);
        write_all(b"\n");
        unsafe {
            let _ = syscall::syscall1(syscall::SYS_EXIT, 127);
            let _ = syscall::syscall1(syscall::SYS_EXIT_GROUP, 127);
        }
        loop {
            core::hint::spin_loop();
        }
    }

    set_tty_foreground(child as u32);
    let mut status = 0i32;
    let wait_rc = unsafe {
        syscall::syscall4(
            syscall::SYS_WAIT4,
            child as u64,
            &mut status as *mut i32 as u64,
            0,
            0,
        )
    };
    let self_pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) } as u32;
    set_tty_foreground(self_pid);
    if wait_rc < 0 {
        print_errno(b"wait4", wait_rc);
    } else {
        state.last_status = if (status & 0x7f) == 0 { (status >> 8) & 0xff } else { 128 + (status & 0x7f) };
    }
    Ok(())
}

pub fn external_command_path(
    raw_cmd: &[u8; EXEC_ARG_MAX],
    state: &ShellState,
    out: &mut [u8; PATH_MAX],
    fallback: &mut [u8; PATH_MAX],
) -> Option<usize> {
    let cmd = cstr_prefix(raw_cmd);
    if cmd_contains_slash(cmd) {
        return absolute_path(state.cwd(), cmd, out);
    }
    let bin_len = build_prefixed_command_path(b"/bin/", cmd, out)?;
    let _ = build_prefixed_command_path(b"/sbin/", cmd, fallback);
    Some(bin_len)
}

pub fn cstr_prefix(buf: &[u8]) -> &[u8] {
    let mut len = 0usize;
    while len < buf.len() && buf[len] != 0 {
        len += 1;
    }
    &buf[..len]
}

pub fn cmd_contains_slash(cmd: &[u8]) -> bool {
    let mut i = 0usize;
    while i < cmd.len() {
        if cmd[i] == b'/' {
            return true;
        }
        i += 1;
    }
    false
}

pub fn build_prefixed_command_path(
    prefix: &[u8],
    cmd: &[u8],
    out: &mut [u8; PATH_MAX],
) -> Option<usize> {
    if cmd.is_empty() || prefix.len() + cmd.len() >= PATH_MAX {
        return None;
    }
    out[..prefix.len()].copy_from_slice(prefix);
    out[prefix.len()..prefix.len() + cmd.len()].copy_from_slice(cmd);
    let len = prefix.len() + cmd.len();
    out[len] = 0;
    Some(len)
}

