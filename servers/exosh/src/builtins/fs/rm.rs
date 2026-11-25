//! Recursive removal and glob-star handling (`rm`).
#![allow(dead_code, unused_imports)]
use exo_syscall_abi as syscall;
use crate::builtins::*;
use crate::types::*;
use crate::util::*;
use crate::terminal::*;
use crate::exec::*;

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

