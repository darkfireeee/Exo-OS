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

pub fn cmd_ls(rest: &[u8], state: &ShellState) {
    let (opts, target_arg) = parse_ls_args(rest);
    let target = if target_arg.is_empty() {
        state.cwd()
    } else {
        target_arg
    };
    let mut path = [0u8; PATH_MAX];
    let Some(path_len) = absolute_path(state.cwd(), target, &mut path) else {
        write_all(b"ls: path too long\n");
        return;
    };

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
        if opts.long {
            print_long_entry(&path, path_len, basename(&path[..path_len]), opts);
        } else if let Some(stat) = stat_path(&path) {
            let mut line = [0u8; PATH_MAX + 32];
            let mut len = 0usize;
            push_colored_name(
                &mut line,
                &mut len,
                basename(&path[..path_len]),
                stat.st_mode,
                is_dir_mode(stat.st_mode) && path_len > 1,
            );
            push_bytes(&mut line, &mut len, b"\n");
            write_bytes(&line[..len]);
        } else {
            print_errno(b"ls", fd);
        }
        return;
    }

    let mut buf = [0u8; IO_BUF];
    let mut out_buf = [0u8; IO_BUF];
    let mut out_len = 0usize;
    let mut printed = false;
    loop {
        let n = unsafe {
            syscall::syscall3(
                syscall::SYS_GETDENTS64,
                fd as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        if n == 0 {
            break;
        }
        if n < 0 {
            if out_len > 0 {
                write_bytes(&out_buf[..out_len]);
                out_len = 0;
            }
            print_errno(b"ls", n);
            break;
        }
        parse_ls_dirents(
            &buf[..n as usize],
            &path,
            path_len,
            opts,
            &mut printed,
            &mut out_buf,
            &mut out_len,
        );
    }
    if out_len > 0 {
        write_bytes(&out_buf[..out_len]);
    }
    if printed && !opts.long {
        write_all(b"\n");
    }
    let _ = close_fd(fd);
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
        let text = trim_end(&body[..redir]);
        let path_arg = trim(&body[redir + 1..]);
        if path_arg.is_empty() {
            write_all(b"echo: missing redirection path\n");
            return;
        }
        let mut path = [0u8; PATH_MAX];
        if absolute_path(state.cwd(), path_arg, &mut path).is_none() {
            write_all(b"echo: path too long\n");
            return;
        }
        let fd = unsafe {
            syscall::syscall4(
                syscall::SYS_OPENAT,
                AT_FDCWD,
                path.as_ptr() as u64,
                syscall::O_CREAT | syscall::O_WRONLY | syscall::O_TRUNC,
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

pub fn cmd_rm(rest: &[u8], state: &ShellState) {
    let (opts, mut args) = parse_rm_options(rest);
    if trim(args).is_empty() {
        write_all(b"rm: missing path\n");
        return;
    }

    loop {
        let (arg, tail) = next_arg(args);
        if arg.is_empty() {
            break;
        }
        rm_one_arg(arg, state, opts);
        args = tail;
    }
}

pub fn cmd_cp(rest: &[u8], state: &ShellState) {
    let (src_arg, tail) = next_arg(rest);
    let (dst_arg, _) = next_arg(tail);
    if src_arg.is_empty() || dst_arg.is_empty() {
        write_all(b"cp: usage: cp <src> <dst>\n");
        return;
    }

    let mut src = [0u8; PATH_MAX];
    if absolute_path(state.cwd(), src_arg, &mut src).is_none() {
        write_all(b"cp: source path too long\n");
        return;
    }
    let mut dst = [0u8; PATH_MAX];
    let Some(mut dst_len) = absolute_path(state.cwd(), dst_arg, &mut dst) else {
        write_all(b"cp: destination path too long\n");
        return;
    };
    if is_dir_path(&dst) {
        let mut full_dst = [0u8; PATH_MAX];
        let Some(len) = append_path_component(&dst, dst_len, basename(&src_arg), &mut full_dst)
        else {
            write_all(b"cp: destination path too long\n");
            return;
        };
        dst = full_dst;
        dst_len = len;
    }
    let _ = dst_len;
    if is_dir_path(&src) {
        write_all(b"cp: omitting directory\n");
        return;
    }

    let in_fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            src.as_ptr() as u64,
            syscall::O_RDONLY,
            0,
        )
    };
    if in_fd < 0 {
        print_errno(b"cp", in_fd);
        return;
    }

    let out_fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            dst.as_ptr() as u64,
            syscall::O_CREAT | syscall::O_WRONLY | syscall::O_TRUNC,
            0o644,
        )
    };
    if out_fd < 0 {
        print_errno(b"cp", out_fd);
        let _ = close_fd(in_fd);
        return;
    }

    let mut buf = [0u8; 512];
    loop {
        let n = unsafe {
            syscall::syscall3(
                syscall::SYS_READ,
                in_fd as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        if n == 0 {
            break;
        }
        if n < 0 {
            print_errno(b"cp", n);
            break;
        }
        let rc = write_fd_all(out_fd, &buf[..n as usize]);
        if rc < 0 {
            print_errno(b"cp", rc);
            break;
        }
    }

    let _ = close_fd(out_fd);
    let _ = close_fd(in_fd);
}

pub fn cmd_mv(rest: &[u8], state: &ShellState) {
    let (src_arg, tail) = next_arg(rest);
    let (dst_arg, _) = next_arg(tail);
    if src_arg.is_empty() || dst_arg.is_empty() {
        write_all(b"mv: usage: mv <src> <dst>\n");
        return;
    }

    let mut src = [0u8; PATH_MAX];
    let Some(src_len) = absolute_path(state.cwd(), src_arg, &mut src) else {
        write_all(b"mv: source path too long\n");
        return;
    };
    let mut dst = [0u8; PATH_MAX];
    let Some(mut dst_len) = absolute_path(state.cwd(), dst_arg, &mut dst) else {
        write_all(b"mv: destination path too long\n");
        return;
    };
    if is_dir_path(&dst) {
        let mut full_dst = [0u8; PATH_MAX];
        let Some(len) =
            append_path_component(&dst, dst_len, basename(&src[..src_len]), &mut full_dst)
        else {
            write_all(b"mv: destination path too long\n");
            return;
        };
        dst = full_dst;
        dst_len = len;
    }
    let _ = dst_len;

    let rc = unsafe {
        syscall::syscall2(
            syscall::SYS_RENAME,
            src.as_ptr() as u64,
            dst.as_ptr() as u64,
        )
    };
    if (rc == EXDEV || rc == EISDIR)
        && stat_path(&src)
            .map(|stat| is_regular_mode(stat.st_mode))
            .unwrap_or(false)
    {
        let copy_rc = copy_file_path(&src, &dst);
        if copy_rc < 0 {
            print_errno(b"mv", copy_rc);
            return;
        }
        let unlink_rc = unsafe { syscall::syscall1(syscall::SYS_UNLINK, src.as_ptr() as u64) };
        if unlink_rc < 0 {
            print_errno(b"mv", unlink_rc);
            return;
        }
        return;
    }
    if rc < 0 {
        print_errno(b"mv", rc);
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

pub fn cmd_tree(rest: &[u8], state: &ShellState) {
    let (arg, _) = next_arg(rest);
    let target = if arg.is_empty() { state.cwd() } else { arg };
    let mut path = [0u8; PATH_MAX];
    let Some(len) = absolute_path(state.cwd(), target, &mut path) else {
        write_all(b"tree: path too long\n");
        return;
    };

    write_bytes(&path[..len]);
    write_all(b"\n");
    tree_walk(&path, len, 0);
}

pub fn tree_walk(path: &[u8; PATH_MAX], path_len: usize, depth: usize) {
    if depth >= TREE_MAX_DEPTH {
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
        if depth == 0 {
            print_errno(b"tree", fd);
        }
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
        if n == 0 {
            break;
        }
        if n < 0 {
            if depth == 0 {
                print_errno(b"tree", n);
            }
            break;
        }
        parse_tree_dirents(&buf[..n as usize], path, path_len, depth);
    }

    let _ = close_fd(fd);
}

pub fn cmd_stat(rest: &[u8], state: &ShellState) {
    let (arg, _) = next_arg(rest);
    if arg.is_empty() {
        write_all(b"stat: usage: stat <path>\n");
        return;
    }
    let mut path = [0u8; PATH_MAX];
    let Some(path_len) = absolute_path(state.cwd(), arg, &mut path) else {
        write_all(b"stat: path too long\n");
        return;
    };
    let Some(stat) = stat_path(&path) else {
        print_errno(b"stat", syscall::ENOENT);
        return;
    };

    write_all(b"  File: ");
    write_bytes(&path[..path_len]);
    write_all(b"\n  Size: ");
    write_i64(stat.st_size);
    write_all(b"\tBlocks: ");
    write_i64(stat.st_blocks);
    write_all(b"\tIO Block: ");
    write_i64(stat.st_blksize);
    write_all(b"\n  Type: ");
    if is_dir_mode(stat.st_mode) {
        write_all(b"directory");
    } else if is_regular_mode(stat.st_mode) {
        write_all(b"regular file");
    } else {
        write_all(b"special");
    }
    write_all(b"\n  Inode: ");
    write_u64(stat.st_ino);
    write_all(b"\tLinks: ");
    write_u64(stat.st_nlink);
    write_all(b"\n  Mode: ");
    write_mode(stat.st_mode);
    write_all(b" (");
    write_octal_mode(stat.st_mode & 0o7777);
    write_all(b")\n");
}

pub fn cmd_dd(rest: &[u8], state: &ShellState) {
    let mut input_arg: &[u8] = b"";
    let mut output_arg: &[u8] = b"";
    let mut block_size = DD_DEFAULT_BS;
    let mut count: Option<u64> = None;

    let mut args = rest;
    loop {
        let (arg, tail) = next_arg(args);
        if arg.is_empty() {
            break;
        }
        if let Some(value) = strip_prefix(arg, b"if=") {
            input_arg = value;
        } else if let Some(value) = strip_prefix(arg, b"of=") {
            output_arg = value;
        } else if let Some(value) = strip_prefix(arg, b"bs=") {
            let Some(parsed) = parse_size(value) else {
                write_all(b"dd: invalid bs\n");
                return;
            };
            block_size = parsed.max(1);
        } else if let Some(value) = strip_prefix(arg, b"count=") {
            let Some(parsed) = parse_u64(value) else {
                write_all(b"dd: invalid count\n");
                return;
            };
            count = Some(parsed);
        } else {
            write_all(b"dd: unknown operand: ");
            write_bytes(arg);
            write_all(b"\n");
            return;
        }
        args = tail;
    }

    if input_arg.is_empty() || output_arg.is_empty() {
        write_all(
            b"dd: usage: dd if=<path|/dev/zero|/dev/urandom> of=<path|/dev/null> [bs=1M] [count=N]\n",
        );
        return;
    }
    if (is_dev_zero(input_arg) || is_dev_urandom(input_arg)) && count.is_none() {
        write_all(b"dd: count required with generated input\n");
        return;
    }

    let input_zero = is_dev_zero(input_arg);
    let input_random = is_dev_urandom(input_arg);
    let output_null = is_dev_null(output_arg);
    let mut input_fd = -1i64;
    let mut output_fd = -1i64;

    if !input_zero && !input_random {
        let mut path = [0u8; PATH_MAX];
        if absolute_path(state.cwd(), input_arg, &mut path).is_none() {
            write_all(b"dd: input path too long\n");
            return;
        }
        input_fd = unsafe {
            syscall::syscall4(
                syscall::SYS_OPENAT,
                AT_FDCWD,
                path.as_ptr() as u64,
                syscall::O_RDONLY,
                0,
            )
        };
        if input_fd < 0 {
            print_errno(b"dd", input_fd);
            return;
        }
    }

    if !output_null {
        let mut path = [0u8; PATH_MAX];
        if absolute_path(state.cwd(), output_arg, &mut path).is_none() {
            write_all(b"dd: output path too long\n");
            if input_fd >= 0 {
                let _ = close_fd(input_fd);
            }
            return;
        }
        output_fd = unsafe {
            syscall::syscall4(
                syscall::SYS_OPENAT,
                AT_FDCWD,
                path.as_ptr() as u64,
                syscall::O_CREAT | syscall::O_WRONLY | syscall::O_TRUNC,
                0o644,
            )
        };
        if output_fd < 0 {
            print_errno(b"dd", output_fd);
            if input_fd >= 0 {
                let _ = close_fd(input_fd);
            }
            return;
        }
    }

    let start = monotonic_ns();
    if input_zero {
        let Some(max_blocks) = count else {
            write_all(b"dd: count required with /dev/zero\n");
            return;
        };
        let Some(total_bytes) = block_size.checked_mul(max_blocks) else {
            write_all(b"dd: size overflow\n");
            if output_fd >= 0 {
                let _ = close_fd(output_fd);
            }
            return;
        };

        if !output_null {
            let rc = ftruncate_fd(output_fd, total_bytes);
            if rc < 0 {
                print_errno(b"dd", rc);
                let _ = close_fd(output_fd);
                return;
            }
            let _ = close_fd(output_fd);
        }

        let elapsed = match (start, monotonic_ns()) {
            (Some(a), Some(b)) if b >= a => b - a,
            _ => 0,
        };
        print_dd_summary(total_bytes, elapsed);
        return;
    }

    let buf = unsafe { &mut *DD_IO_BUFFER.0.get() };
    let mut total = 0u64;
    let mut blocks = 0u64;
    let mut eof = false;

    loop {
        if let Some(max_blocks) = count {
            if blocks >= max_blocks {
                break;
            }
        }

        let mut remaining = block_size;
        let mut copied_any = false;
        while remaining > 0 {
            let chunk = remaining.min(buf.len() as u64) as usize;
            let n = if input_random {
                unsafe {
                    syscall::syscall3(
                        syscall::SYS_GETRANDOM,
                        buf.as_mut_ptr() as u64,
                        chunk as u64,
                        0,
                    )
                }
            } else {
                unsafe {
                    syscall::syscall3(
                        syscall::SYS_READ,
                        input_fd as u64,
                        buf.as_mut_ptr() as u64,
                        chunk as u64,
                    )
                }
            };

            if n < 0 {
                print_errno(b"dd", n);
                eof = true;
                break;
            }
            if n == 0 {
                eof = true;
                break;
            }

            let n_usize = n as usize;
            if !output_null {
                let rc = write_fd_all(output_fd, &buf[..n_usize]);
                if rc < 0 {
                    print_errno(b"dd", rc);
                    eof = true;
                    break;
                }
            }
            total = total.saturating_add(n as u64);
            remaining = remaining.saturating_sub(n as u64);
            copied_any = true;

            if !input_random && n_usize < chunk {
                eof = true;
                break;
            }
        }

        if copied_any {
            blocks = blocks.saturating_add(1);
        }
        if eof || (!copied_any && count.is_none()) {
            break;
        }
    }

    if input_fd >= 0 {
        let _ = close_fd(input_fd);
    }
    if output_fd >= 0 {
        let _ = close_fd(output_fd);
    }

    let elapsed = match (start, monotonic_ns()) {
        (Some(a), Some(b)) if b >= a => b - a,
        _ => 0,
    };
    print_dd_summary(total, elapsed);
}

pub fn print_dd_summary(total: u64, elapsed: u64) {
    write_u64(total);
    write_all(b" bytes copied in ");
    write_duration_ms(elapsed);
    write_all(b" -> ");
    write_mib_per_sec(total, elapsed);
    write_all(b" MiB/s\n");
}

pub fn parse_ls_args(mut rest: &[u8]) -> (LsOptions, &[u8]) {
    let mut opts = LsOptions::default();
    loop {
        let (arg, tail) = next_arg(rest);
        if arg.len() < 2 || arg[0] != b'-' {
            return (opts, arg);
        }
        let mut i = 1usize;
        while i < arg.len() {
            match arg[i] {
                b'l' => opts.long = true,
                b'a' => opts.all = true,
                b'h' => opts.human = true,
                _ => {}
            }
            i += 1;
        }
        rest = tail;
    }
}

pub fn parse_ls_dirents(
    buf: &[u8],
    parent_path: &[u8; PATH_MAX],
    parent_len: usize,
    opts: LsOptions,
    printed: &mut bool,
    out_buf: &mut [u8; IO_BUF],
    out_len: &mut usize,
) {
    let mut off = 0usize;
    while off + DIRENT64_HEADER_SIZE <= buf.len() {
        let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
        if reclen == 0 || off + reclen > buf.len() {
            break;
        }
        let dtype = buf[off + 18];
        let name_start = off + DIRENT64_HEADER_SIZE;
        let mut name_end = name_start;
        while name_end < off + reclen && buf[name_end] != 0 {
            name_end += 1;
        }
        let name = &buf[name_start..name_end];
        if !name.is_empty() && (opts.all || name[0] != b'.') {
            if opts.long {
                if *out_len > 0 {
                    write_bytes(&out_buf[..*out_len]);
                    *out_len = 0;
                }
                let mut child = [0u8; PATH_MAX];
                if let Some(child_len) =
                    append_path_component(parent_path, parent_len, name, &mut child)
                {
                    print_long_entry(&child, child_len, name, opts);
                }
            } else {
                if *printed {
                    if *out_len + 2 >= out_buf.len() {
                        write_bytes(&out_buf[..*out_len]);
                        *out_len = 0;
                    }
                    push_bytes(out_buf, out_len, b"  ");
                }
                if *out_len + name.len() + 32 >= out_buf.len() {
                    write_bytes(&out_buf[..*out_len]);
                    *out_len = 0;
                }
                if dtype == DT_DIR {
                    push_colored_name(out_buf, out_len, name, S_IFDIR | 0o755, true);
                } else {
                    push_colored_name(out_buf, out_len, name, S_IFREG | 0o644, false);
                }
            }
            *printed = true;
        }
        off += reclen;
    }
}

fn push_hex16(buf: &mut [u8], len: &mut usize, val: u16) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let b0 = HEX[((val >> 12) & 0xF) as usize];
    let b1 = HEX[((val >> 8) & 0xF) as usize];
    let b2 = HEX[((val >> 4) & 0xF) as usize];
    let b3 = HEX[(val & 0xF) as usize];
    push_bytes(buf, len, &[b0, b1, b2, b3]);
}

pub fn print_long_entry(path: &[u8; PATH_MAX], path_len: usize, name: &[u8], opts: LsOptions) {
    let mut line = [0u8; PATH_MAX + 128];
    let mut len = 0usize;
    let Some(stat) = stat_path(path) else {
        push_bytes(&mut line, &mut len, b"? ??????? .          @0000 ep:0000 ? ");
        push_bytes(&mut line, &mut len, name);
        push_bytes(&mut line, &mut len, b"\n");
        write_bytes(&line[..len]);
        return;
    };

    let is_dir = is_dir_mode(stat.st_mode);
    let is_exec = (stat.st_mode & 0o111) != 0;

    // 1. Type & Rights & Flags (SPEC-EXO-DISPLAY-PROTOCOL-STRATA)
    if is_dir {
        push_bytes(&mut line, &mut len, b"d rw-l--- .          ");
    } else if is_exec {
        push_bytes(&mut line, &mut len, b"x r-x---- [sig]      ");
    } else {
        push_bytes(&mut line, &mut len, b"r rw----- .          ");
    }

    // 2. Capability Token & Epoch
    let token_hint = ((stat.st_ino & 0xFFFF) as u16).max(1);
    push_bytes(&mut line, &mut len, b"@");
    push_hex16(&mut line, &mut len, token_hint);
    push_bytes(&mut line, &mut len, b" ep:0001 ");

    // 3. Size
    if opts.human {
        push_human_size(&mut line, &mut len, stat.st_size);
    } else {
        push_i64(&mut line, &mut len, stat.st_size);
    }
    push_bytes(&mut line, &mut len, b" ");

    // 4. Name
    push_colored_name(
        &mut line,
        &mut len,
        name,
        stat.st_mode,
        is_dir && path_len > 1,
    );
    push_bytes(&mut line, &mut len, b"\n");
    write_bytes(&line[..len]);
}

pub fn write_colored_name(name: &[u8], mode: u32, append_slash: bool) {
    let mut buf = [0u8; PATH_MAX + 32];
    let mut len = 0usize;
    push_colored_name(&mut buf, &mut len, name, mode, append_slash);
    write_bytes(&buf[..len]);
}

pub fn parse_tree_dirents(buf: &[u8], parent_path: &[u8; PATH_MAX], parent_len: usize, depth: usize) {
    let mut off = 0usize;
    while off + DIRENT64_HEADER_SIZE <= buf.len() {
        let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
        if reclen == 0 || off + reclen > buf.len() {
            break;
        }
        let dtype = buf[off + 18];
        let name_start = off + DIRENT64_HEADER_SIZE;
        let mut name_end = name_start;
        while name_end < off + reclen && buf[name_end] != 0 {
            name_end += 1;
        }
        let name = &buf[name_start..name_end];
        if !name.is_empty() && !bytes_eq(name, b".") && !bytes_eq(name, b"..") {
            write_tree_indent(depth);
            write_bytes(name);
            if dtype == DT_DIR {
                write_all(b"/");
            }
            write_all(b"\n");

            if dtype == DT_DIR && depth + 1 < TREE_MAX_DEPTH {
                let mut child = [0u8; PATH_MAX];
                if let Some(child_len) =
                    append_path_component(parent_path, parent_len, name, &mut child)
                {
                    tree_walk(&child, child_len, depth + 1);
                }
            }
        }
        off += reclen;
    }
}

pub fn write_tree_indent(depth: usize) {
    let mut i = 0usize;
    while i <= depth {
        write_all(b"  ");
        i += 1;
    }
}

pub fn copy_file_path(src: &[u8; PATH_MAX], dst: &[u8; PATH_MAX]) -> i64 {
    let in_fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            src.as_ptr() as u64,
            syscall::O_RDONLY,
            0,
        )
    };
    if in_fd < 0 {
        return in_fd;
    }

    let out_fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            dst.as_ptr() as u64,
            syscall::O_CREAT | syscall::O_WRONLY | syscall::O_TRUNC,
            0o644,
        )
    };
    if out_fd < 0 {
        let _ = close_fd(in_fd);
        return out_fd;
    }

    let mut buf = [0u8; 512];
    let mut status = 0i64;
    loop {
        let n = unsafe {
            syscall::syscall3(
                syscall::SYS_READ,
                in_fd as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        if n == 0 {
            break;
        }
        if n < 0 {
            status = n;
            break;
        }
        let rc = write_fd_all(out_fd, &buf[..n as usize]);
        if rc < 0 {
            status = rc;
            break;
        }
    }

    let _ = close_fd(out_fd);
    let _ = close_fd(in_fd);
    if status < 0 {
        let _ = unsafe { syscall::syscall1(syscall::SYS_UNLINK, dst.as_ptr() as u64) };
    }
    status
}

pub fn parse_rm_options(mut rest: &[u8]) -> (RmOptions, &[u8]) {
    let mut opts = RmOptions::default();
    loop {
        let (arg, tail) = next_arg(rest);
        if arg.len() < 2 || arg[0] != b'-' {
            return (opts, rest);
        }
        let mut i = 1usize;
        while i < arg.len() {
            match arg[i] {
                b'f' => opts.force = true,
                b'r' | b'R' => opts.recursive = true,
                _ => {}
            }
            i += 1;
        }
        rest = tail;
    }
}

pub fn rm_one_arg(arg: &[u8], state: &ShellState, opts: RmOptions) {
    if is_star_pattern(arg) {
        rm_star(arg, state, opts);
        return;
    }

    let mut path = [0u8; PATH_MAX];
    let Some(path_len) = absolute_path(state.cwd(), arg, &mut path) else {
        write_all(b"rm: path too long\n");
        return;
    };
    rm_path(&path, path_len, opts, 0, b"rm");
}

pub fn is_star_pattern(arg: &[u8]) -> bool {
    basename(arg) == b"*"
}

pub fn rm_star(arg: &[u8], state: &ShellState, opts: RmOptions) {
    let mut dir_arg = [0u8; PATH_MAX];
    let dir_slice = star_parent_arg(arg, &mut dir_arg);
    let mut dir = [0u8; PATH_MAX];
    let Some(dir_len) = absolute_path(state.cwd(), dir_slice, &mut dir) else {
        write_all(b"rm: path too long\n");
        return;
    };
    rm_directory_children(&dir, dir_len, opts, 0, false, true);
}

pub fn star_parent_arg<'a>(arg: &'a [u8], out: &'a mut [u8; PATH_MAX]) -> &'a [u8] {
    let arg = trim_end_slashes(arg);
    let mut i = arg.len();
    while i > 0 {
        if arg[i - 1] == b'/' {
            if i == 1 {
                return b"/";
            }
            let len = i - 1;
            out[..len].copy_from_slice(&arg[..len]);
            return &out[..len];
        }
        i -= 1;
    }
    b"."
}

pub fn rm_path(path: &[u8; PATH_MAX], path_len: usize, opts: RmOptions, depth: usize, prefix: &[u8]) {
    let Some(stat) = stat_path(path) else {
        if !opts.force {
            print_errno(prefix, -2);
        }
        return;
    };

    if is_dir_mode(stat.st_mode) {
        if !opts.recursive {
            print_errno(prefix, -21);
            return;
        }
        if depth >= RM_MAX_DEPTH {
            write_all(b"rm: recursion limit\n");
            return;
        }
        rm_directory_children(path, path_len, opts, depth + 1, true, false);
        return;
    }

    let rc = unsafe { syscall::syscall1(syscall::SYS_UNLINK, path.as_ptr() as u64) };
    if rc < 0 && !opts.force {
        print_errno(prefix, rc);
    }
}

pub fn rm_directory_children(
    path: &[u8; PATH_MAX],
    path_len: usize,
    opts: RmOptions,
    depth: usize,
    remove_self: bool,
    skip_hidden: bool,
) {
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
        if !opts.force {
            print_errno(b"rm", fd);
        }
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
        if n == 0 {
            break;
        }
        if n < 0 {
            if !opts.force {
                print_errno(b"rm", n);
            }
            break;
        }
        rm_dirent_batch(&buf[..n as usize], path, path_len, opts, depth, skip_hidden);
    }
    let _ = close_fd(fd);

    if remove_self {
        let rc = unsafe { syscall::syscall1(syscall::SYS_RMDIR, path.as_ptr() as u64) };
        if rc < 0 && !opts.force {
            print_errno(b"rm", rc);
        }
    }
}

pub fn rm_dirent_batch(
    buf: &[u8],
    parent_path: &[u8; PATH_MAX],
    parent_len: usize,
    opts: RmOptions,
    depth: usize,
    skip_hidden: bool,
) {
    let mut off = 0usize;
    while off + DIRENT64_HEADER_SIZE <= buf.len() {
        let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
        if reclen == 0 || off + reclen > buf.len() {
            break;
        }
        let name_start = off + DIRENT64_HEADER_SIZE;
        let mut name_end = name_start;
        while name_end < off + reclen && buf[name_end] != 0 {
            name_end += 1;
        }
        let name = &buf[name_start..name_end];
        if !name.is_empty()
            && !bytes_eq(name, b".")
            && !bytes_eq(name, b"..")
            && (!skip_hidden || name[0] != b'.')
        {
            let mut child = [0u8; PATH_MAX];
            if let Some(child_len) =
                append_path_component(parent_path, parent_len, name, &mut child)
            {
                rm_path(&child, child_len, opts, depth, b"rm");
            }
        }
        off += reclen;
    }
}

pub fn cmd_sync() {
    let rc = unsafe { syscall::syscall0(syscall::SYS_SYNC) };
    if rc < 0 {
        print_errno(b"sync", rc);
    }
}

