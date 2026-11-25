#![allow(unused_imports, dead_code)]
use super::io_fmt::{eq, print_errno};
use super::path_fs::{append_path_component, close, is_dir_mode, open_path, path_arg, stat_path};
use crate::bare::types::*;
use exo_syscall_abi as syscall;

    pub fn remove_path(path: &[u8; PATH_MAX], recursive: bool, force: bool, depth: usize) -> i32 {
        let Some(st) = stat_path(path) else {
            return if force { 0 } else { print_errno(b"rm", -2) };
        };

        if !is_dir_mode(st.st_mode) {
            let rc = unsafe { syscall::syscall1(syscall::SYS_UNLINK, path.as_ptr() as u64) };
            if rc < 0 && !force {
                return print_errno(b"rm", rc);
            }
            return 0;
        }

        if !recursive {
            return if force { 0 } else { print_errno(b"rm", EISDIR) };
        }
        if depth >= RM_MAX_DEPTH {
            return if force { 0 } else { print_errno(b"rm", -36) };
        }

        let fd = open_path(path, syscall::O_RDONLY, 0);
        if fd < 0 {
            return if force { 0 } else { print_errno(b"rm", fd) };
        }

        let mut rc = 0i32;
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
                close(fd);
                return if force { 0 } else { print_errno(b"rm", n) };
            }

            let mut off = 0usize;
            while off + DIRENT64_HEADER_SIZE <= n as usize {
                let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
                if reclen == 0 || off + reclen > n as usize {
                    break;
                }
                let start = off + DIRENT64_HEADER_SIZE;
                let mut end = start;
                while end < off + reclen && buf[end] != 0 {
                    end += 1;
                }
                let name = &buf[start..end];
                if !name.is_empty() && !eq(name, b".") && !eq(name, b"..") {
                    let mut child = [0u8; PATH_MAX];
                    if append_path_component(path, name, &mut child).is_none() {
                        if !force {
                            rc = print_errno(b"rm", -36);
                        }
                    } else {
                        let child_rc = remove_path(&child, recursive, force, depth + 1);
                        if child_rc != 0 && rc == 0 {
                            rc = child_rc;
                        }
                    }
                }
                off += reclen;
            }
        }
        close(fd);

        let rmdir_rc = unsafe { syscall::syscall1(syscall::SYS_RMDIR, path.as_ptr() as u64) };
        if rmdir_rc < 0 && !force {
            return print_errno(b"rm", rmdir_rc);
        }
        rc
    }

    pub fn remove_star(args: &Args, arg: &[u8], recursive: bool, force: bool) -> i32 {
        let parent_arg = if eq(arg, b"*") {
            b"." as &[u8]
        } else if arg.len() > 2 && arg[arg.len() - 2] == b'/' && arg[arg.len() - 1] == b'*' {
            &arg[..arg.len() - 2]
        } else {
            return print_errno(b"rm", -22);
        };

        let mut parent = [0u8; PATH_MAX];
        if path_arg(args, parent_arg, &mut parent).is_none() {
            return if force { 0 } else { print_errno(b"rm", -36) };
        }
        let fd = open_path(&parent, syscall::O_RDONLY, 0);
        if fd < 0 {
            return if force { 0 } else { print_errno(b"rm", fd) };
        }

        let mut rc = 0i32;
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
                close(fd);
                return if force { 0 } else { print_errno(b"rm", n) };
            }

            let mut off = 0usize;
            while off + DIRENT64_HEADER_SIZE <= n as usize {
                let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
                if reclen == 0 || off + reclen > n as usize {
                    break;
                }
                let start = off + DIRENT64_HEADER_SIZE;
                let mut end = start;
                while end < off + reclen && buf[end] != 0 {
                    end += 1;
                }
                let name = &buf[start..end];
                if !name.is_empty() && name[0] != b'.' && !eq(name, b".") && !eq(name, b"..") {
                    let mut child = [0u8; PATH_MAX];
                    if append_path_component(&parent, name, &mut child).is_none() {
                        if !force {
                            rc = print_errno(b"rm", -36);
                        }
                    } else {
                        let child_rc = remove_path(&child, recursive, force, 1);
                        if child_rc != 0 && rc == 0 {
                            rc = child_rc;
                        }
                    }
                }
                off += reclen;
            }
        }
        close(fd);
        rc
    }

