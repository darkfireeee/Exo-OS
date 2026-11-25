//! Copy and move builtins (`cp`, `mv`, `copy_file_path`).
#![allow(dead_code, unused_imports)]
use exo_syscall_abi as syscall;
use crate::builtins::*;
use crate::types::*;
use crate::util::*;
use crate::terminal::*;
use crate::exec::*;

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

