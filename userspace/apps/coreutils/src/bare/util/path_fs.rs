#![allow(unused_imports, dead_code)]
use super::io_fmt::{eq, print_errno, write_all};
use crate::bare::types::*;
use exo_syscall_abi as syscall;

    pub fn copy_component(out: &mut [u8; PATH_MAX], len: &mut usize, comp: &[u8]) -> bool {
        if *len > 1 {
            if *len + 1 >= PATH_MAX {
                return false;
            }
            out[*len] = b'/';
            *len += 1;
        }
        if *len + comp.len() >= PATH_MAX {
            return false;
        }
        out[*len..*len + comp.len()].copy_from_slice(comp);
        *len += comp.len();
        true
    }

    pub fn pop_component(out: &mut [u8; PATH_MAX], len: &mut usize) {
        if *len <= 1 {
            *len = 1;
            return;
        }
        while *len > 1 && out[*len - 1] != b'/' {
            *len -= 1;
        }
        if *len > 1 {
            *len -= 1;
        }
    }

    pub fn normalize_into(src: &[u8], out: &mut [u8; PATH_MAX]) -> Option<usize> {
        let mut len = 1usize;
        out[0] = b'/';
        let mut i = 0usize;
        while i <= src.len() {
            while i < src.len() && src[i] == b'/' {
                i += 1;
            }
            let start = i;
            while i < src.len() && src[i] != b'/' {
                i += 1;
            }
            if start == i {
                break;
            }
            let comp = &src[start..i];
            if eq(comp, b".") {
                continue;
            }
            if eq(comp, b"..") {
                pop_component(out, &mut len);
                continue;
            }
            if !copy_component(out, &mut len, comp) {
                return None;
            }
        }
        if len >= PATH_MAX {
            return None;
        }
        out[len] = 0;
        Some(len)
    }

    pub fn path_arg(args: &Args, input: &[u8], out: &mut [u8; PATH_MAX]) -> Option<usize> {
        let input = if input.is_empty() { b"." } else { input };
        if input.starts_with(b"/") {
            return normalize_into(input, out);
        }
        let cwd = args.pwd();
        let mut joined = [0u8; PATH_MAX * 2];
        let mut len = 0usize;
        let cwd_len = cwd.len().min(joined.len());
        joined[..cwd_len].copy_from_slice(&cwd[..cwd_len]);
        len += cwd_len;
        if len == 0 || joined[len - 1] != b'/' {
            if len >= joined.len() {
                return None;
            }
            joined[len] = b'/';
            len += 1;
        }
        if len + input.len() > joined.len() {
            return None;
        }
        joined[len..len + input.len()].copy_from_slice(input);
        len += input.len();
        normalize_into(&joined[..len], out)
    }

    pub fn basename(path: &[u8]) -> &[u8] {
        let mut end = path.len();
        while end > 1 && path[end - 1] == b'/' {
            end -= 1;
        }
        let mut start = end;
        while start > 0 && path[start - 1] != b'/' {
            start -= 1;
        }
        &path[start..end]
    }

    pub fn dirname(path: &[u8], out: &mut [u8; PATH_MAX]) -> usize {
        let mut end = path.len();
        while end > 1 && path[end - 1] == b'/' {
            end -= 1;
        }
        let mut slash = end;
        while slash > 0 && path[slash - 1] != b'/' {
            slash -= 1;
        }
        let len = if slash == 0 {
            1
        } else {
            slash.saturating_sub(1).max(1)
        };
        if slash == 0 {
            out[0] = b'.';
            out[1] = 0;
            1
        } else {
            out[..len].copy_from_slice(&path[..len]);
            out[len] = 0;
            len
        }
    }

    pub fn open_path(path: &[u8; PATH_MAX], flags: u64, mode: u64) -> i64 {
        unsafe {
            syscall::syscall4(
                syscall::SYS_OPENAT,
                AT_FDCWD,
                path.as_ptr() as u64,
                flags,
                mode,
            )
        }
    }

    pub fn close(fd: i64) {
        if fd >= 0 {
            let _ = unsafe { syscall::syscall1(syscall::SYS_CLOSE, fd as u64) };
        }
    }

    pub fn write_fd_all(fd: u64, bytes: &[u8]) -> i64 {
        let mut done = 0usize;
        while done < bytes.len() {
            let n = unsafe {
                syscall::syscall3(
                    syscall::SYS_WRITE,
                    fd,
                    bytes[done..].as_ptr() as u64,
                    (bytes.len() - done) as u64,
                )
            };
            if n <= 0 {
                return if n == 0 { -5 } else { n };
            }
            done += n as usize;
        }
        bytes.len() as i64
    }

    pub fn ftruncate_fd(fd: i64, len: u64) -> i64 {
        unsafe { syscall::syscall2(syscall::SYS_FTRUNCATE, fd as u64, len) }
    }

    pub fn monotonic_ns() -> Option<u64> {
        let mut ts = LinuxTimespec::default();
        let rc = unsafe {
            syscall::syscall2(
                syscall::SYS_CLOCK_GETTIME,
                CLOCK_MONOTONIC,
                &mut ts as *mut LinuxTimespec as u64,
            )
        };
        if rc < 0 || ts.tv_sec < 0 || ts.tv_nsec < 0 {
            return None;
        }
        Some((ts.tv_sec as u64).saturating_mul(1_000_000_000) + ts.tv_nsec as u64)
    }

    pub fn stat_path(path: &[u8; PATH_MAX]) -> Option<LinuxStat> {
        let mut st = LinuxStat::default();
        let rc = unsafe {
            syscall::syscall2(
                syscall::SYS_STAT,
                path.as_ptr() as u64,
                &mut st as *mut LinuxStat as u64,
            )
        };
        if rc == 0 {
            Some(st)
        } else {
            None
        }
    }

    pub fn is_dir_mode(mode: u32) -> bool {
        mode & S_IFMT == S_IFDIR
    }

    pub fn is_regular_mode(mode: u32) -> bool {
        mode & S_IFMT == S_IFREG
    }

    pub fn path_len(path: &[u8; PATH_MAX]) -> usize {
        let mut len = 0usize;
        while len < PATH_MAX && path[len] != 0 {
            len += 1;
        }
        len
    }

    pub fn append_path_component(
        parent: &[u8; PATH_MAX],
        name: &[u8],
        out: &mut [u8; PATH_MAX],
    ) -> Option<usize> {
        if name.is_empty() || name.contains(&b'/') {
            return None;
        }
        let mut len = path_len(parent);
        out[..len].copy_from_slice(&parent[..len]);
        if len == 0 {
            out[0] = b'/';
            len = 1;
        }
        if len > 1 && out[len - 1] != b'/' {
            if len + 1 >= PATH_MAX {
                return None;
            }
            out[len] = b'/';
            len += 1;
        }
        if len + name.len() >= PATH_MAX {
            return None;
        }
        out[len..len + name.len()].copy_from_slice(name);
        len += name.len();
        out[len] = 0;
        Some(len)
    }


    pub fn is_dev_zero(path: &[u8]) -> bool {
        eq(path, b"/dev/zero")
    }

    pub fn is_dev_null(path: &[u8]) -> bool {
        eq(path, b"/dev/null")
    }

    pub fn is_dev_urandom(path: &[u8]) -> bool {
        eq(path, b"/dev/urandom") || eq(path, b"/dev/random")
    }

    pub fn read_file_to_fd(path: &[u8; PATH_MAX], out_fd: u64) -> i32 {
        let fd = open_path(path, syscall::O_RDONLY, 0);
        if fd < 0 {
            return print_errno(b"cat", fd);
        }
        let mut buf = [0u8; IO_BUF];
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
                close(fd);
                return print_errno(b"read", n);
            }
            write_all(out_fd, &buf[..n as usize]);
        }
        close(fd);
        0
    }
