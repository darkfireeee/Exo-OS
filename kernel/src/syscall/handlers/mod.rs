//! System Call Handlers
//!
//! Organized by category:
//! - io: File I/O operations
//! - ipc: Inter-process communication
//! - memory: Memory management
//! - process: Process/thread management
//! - security: Capability-based security
//! - time: Time and timers

pub mod fs_dir;
pub mod fs_events;
pub mod fs_fcntl;
pub mod fs_fifo;
pub mod fs_futex;
pub mod fs_link;
pub mod fs_ops;
pub mod fs_poll;
pub mod inotify;
pub mod io;
pub mod ipc;
pub mod ipc_sysv;
pub mod memory;
pub mod net_socket;
pub mod process;
pub mod process_limits;
pub mod sched;
pub mod security;
pub mod signals;
pub mod sys_info;
pub mod time;

// Re-export commonly used types
pub use io::{Fd, FileFlags, FileStat};
pub use ipc::IpcHandle;
pub use memory::{MapFlags, ProtFlags};
pub use process::{Pid, ProcessStatus, Signal};
pub use security::{CapId, Capability, CapabilityType};
pub use time::{ClockId, TimeSpec, TimerId};

use crate::syscall::dispatch::{register_syscall, syscall_numbers::*, SyscallError};

/// I/O vector for readv/writev
#[repr(C)]
struct Iovec {
    iov_base: *mut core::ffi::c_void,
    iov_len: usize,
}

/// Initialize all syscall handlers
pub fn init() {
    // =========================================================================
    // CRITICAL MISSING SYSCALLS - These are essential for POSIX compliance
    // Added to complete Phase 27 (100% syscall coverage)
    // =========================================================================

    // Note: Some basic syscalls (read, write, open, close, getpid, exit) are
    // registered in dispatch.rs via init_default_handlers()

    // Process Management - Critical for process lifecycle
    let _ = register_syscall(SYS_FORK, |_args| {
        let res = process::sys_fork();
        Ok(res.unwrap_or(0) as u64)
    });

    let _ = register_syscall(SYS_EXECVE, |args| {
        let pathname = args[0] as *const i8;
        let argv = args[1] as *const *const i8;
        let envp = args[2] as *const *const i8;
        let res = unsafe { process::sys_execve(pathname, argv, envp) };
        Ok(if res.is_ok() { 0 } else { (-1i64) as u64 })
    });

    let _ = register_syscall(SYS_WAIT4, |args| {
        let pid = args[0] as u64;
        let _wstatus = args[1] as *mut i32;
        let options_raw = args[2] as i32;
        // WaitOptions has different structure, use simplified version
        let options = process::WaitOptions {
            nohang: options_raw & 1 != 0,
            untraced: false,
            continued: false,
        };
        let res = process::sys_wait(pid, options);
        Ok(res.map(|(p, _)| p).unwrap_or(0) as u64)
    });

    let _ = register_syscall(SYS_CLONE, |args| {
        let flags = args[0] as u32;
        let stack = if args[1] == 0 {
            None
        } else {
            Some(args[1] as usize)
        };
        let res = process::sys_clone(flags, stack);
        Ok(res.unwrap_or(0) as u64)
    });

    let _ = register_syscall(SYS_VFORK, |_args| {
        // vfork is deprecated, use fork
        let res = process::sys_fork();
        Ok(res.unwrap_or(0) as u64)
    });

    let _ = register_syscall(SYS_EXIT_GROUP, |args| {
        let code = args[0] as i32;
        process::sys_exit_group(code);
    });

    let _ = register_syscall(SYS_PAUSE, |_args| {
        let res = process::sys_pause();
        Ok(if res.is_ok() { 0 } else { (-1i64) as u64 })
    });

    let _ = register_syscall(SYS_GETTID, |_args| Ok(process::sys_gettid() as u64));

    // Memory Management - Critical for heap and mmap
    let _ = register_syscall(SYS_BRK, |args| {
        use crate::memory::address::VirtualAddress;
        let addr = VirtualAddress::new(args[0] as usize);
        let res = memory::sys_brk(addr);
        Ok(res.map(|a| a.value() as u64).unwrap_or(0))
    });

    let _ = register_syscall(SYS_MMAP, |args| {
        use crate::memory::address::VirtualAddress;
        let addr = VirtualAddress::new(args[0] as usize);
        let length = args[1] as usize;
        let prot_raw = args[2] as i32;
        let flags_raw = args[3] as i32;
        let fd = args[4];
        let offset = args[5] as usize;

        let prot = memory::ProtFlags {
            read: prot_raw & 0x1 != 0,
            write: prot_raw & 0x2 != 0,
            exec: prot_raw & 0x4 != 0,
        };

        let flags = memory::MapFlags {
            shared: flags_raw & 0x1 != 0,
            private: flags_raw & 0x2 != 0,
            fixed: flags_raw & 0x10 != 0,
            anonymous: flags_raw & 0x20 != 0,
        };

        let res = memory::sys_mmap(addr, length, prot, flags, fd, offset);
        Ok(res.map(|a| a.value() as u64).unwrap_or(0))
    });

    let _ = register_syscall(SYS_MUNMAP, |args| {
        use crate::memory::address::VirtualAddress;
        let addr = VirtualAddress::new(args[0] as usize);
        let length = args[1] as usize;
        let res = memory::sys_munmap(addr, length);
        Ok(if res.is_ok() { 0 } else { (-1i64) as u64 })
    });

    let _ = register_syscall(SYS_MPROTECT, |args| {
        use crate::memory::address::VirtualAddress;
        let addr = VirtualAddress::new(args[0] as usize);
        let length = args[1] as usize;
        let prot_raw = args[2] as i32;

        let prot = memory::ProtFlags {
            read: prot_raw & 0x1 != 0,
            write: prot_raw & 0x2 != 0,
            exec: prot_raw & 0x4 != 0,
        };

        let res = memory::sys_mprotect(addr, length, prot);
        Ok(if res.is_ok() { 0 } else { (-1i64) as u64 })
    });

    // File I/O operations - Essential for file manipulation
    let _ = register_syscall(SYS_LSEEK, |args| {
        let fd = args[0] as i32;
        let offset = args[1] as i64;
        let whence = match args[2] as i32 {
            0 => io::SeekWhence::Start,
            1 => io::SeekWhence::Current,
            2 => io::SeekWhence::End,
            _ => io::SeekWhence::Start,
        };
        let res = io::sys_seek(fd, offset, whence);
        Ok(res.unwrap_or(0) as u64)
    });

    let _ = register_syscall(SYS_STAT, |args| {
        let path = args[0] as *const i8;
        let statbuf = args[1] as *mut io::FileStat;
        if path.is_null() || statbuf.is_null() {
            return Ok((-14i64) as u64); // EFAULT
        }

        // Convert C string to Rust str
        let path_str = unsafe {
            let mut len = 0;
            while *path.offset(len) != 0 {
                len += 1;
                if len > 4096 {
                    return Ok((-36i64) as u64);
                } // ENAMETOOLONG
            }
            let slice = core::slice::from_raw_parts(path as *const u8, len as usize);
            match core::str::from_utf8(slice) {
                Ok(s) => s,
                Err(_) => return Ok((-22i64) as u64), // EINVAL
            }
        };

        // Get file stat
        let res = io::sys_stat(path_str);
        match res {
            Ok(stat) => {
                unsafe {
                    *statbuf = stat;
                }
                Ok(0)
            }
            Err(_) => Ok((-2i64) as u64), // ENOENT
        }
    });

    let _ = register_syscall(SYS_FSTAT, |args| {
        let fd = args[0] as i32;
        let statbuf = args[1] as *mut io::FileStat;
        if statbuf.is_null() {
            return Ok((-14i64) as u64); // EFAULT
        }
        let res = io::sys_fstat(fd);
        if let Ok(stat) = res {
            unsafe {
                *statbuf = stat;
            }
            Ok(0)
        } else {
            Ok((-1i64) as u64)
        }
    });

    let _ = register_syscall(SYS_LSTAT, |args| {
        let path = args[0] as *const i8;
        let statbuf = args[1] as *mut io::FileStat;
        if path.is_null() || statbuf.is_null() {
            return Ok((-14i64) as u64); // EFAULT
        }

        // Convert C string to Rust str
        let path_str = unsafe {
            let mut len = 0;
            while *path.offset(len) != 0 {
                len += 1;
                if len > 4096 {
                    return Ok((-36i64) as u64);
                } // ENAMETOOLONG
            }
            let slice = core::slice::from_raw_parts(path as *const u8, len as usize);
            match core::str::from_utf8(slice) {
                Ok(s) => s,
                Err(_) => return Ok((-22i64) as u64), // EINVAL
            }
        };

        // lstat is same as stat but doesn't follow symlinks
        // For now, just call stat (TODO: implement symlink awareness)
        let res = io::sys_stat(path_str);
        match res {
            Ok(stat) => {
                unsafe {
                    *statbuf = stat;
                }
                Ok(0)
            }
            Err(_) => Ok((-2i64) as u64), // ENOENT
        }
    });

    let _ = register_syscall(SYS_READV, |args| {
        let fd = args[0] as i32;
        let iov_ptr = args[1] as *const Iovec;
        let iovcnt = args[2] as i32;

        if iov_ptr.is_null() || iovcnt < 0 || iovcnt > 1024 {
            return Ok((-22i64) as u64); // EINVAL
        }

        let mut total_read: usize = 0;

        // Read into each iovec buffer
        for i in 0..iovcnt {
            let iov = unsafe { &*iov_ptr.offset(i as isize) };
            if iov.iov_base.is_null() {
                continue;
            }

            // Create temporary buffer
            let mut buf = alloc::vec![0u8; iov.iov_len];
            let res = io::sys_read(fd, &mut buf);

            match res {
                Ok(n) => {
                    if n == 0 {
                        break;
                    } // EOF
                    unsafe {
                        core::ptr::copy_nonoverlapping(buf.as_ptr(), iov.iov_base as *mut u8, n);
                    }
                    total_read += n;
                    if n < iov.iov_len {
                        break;
                    } // Partial read
                }
                Err(_) => {
                    if total_read > 0 {
                        return Ok(total_read as u64);
                    } else {
                        return Ok((-1i64) as u64);
                    }
                }
            }
        }

        Ok(total_read as u64)
    });

    let _ = register_syscall(SYS_WRITEV, |args| {
        let fd = args[0] as i32;
        let iov_ptr = args[1] as *const Iovec;
        let iovcnt = args[2] as i32;

        if iov_ptr.is_null() || iovcnt < 0 || iovcnt > 1024 {
            return Ok((-22i64) as u64); // EINVAL
        }

        let mut total_written: usize = 0;

        // Write from each iovec buffer
        for i in 0..iovcnt {
            let iov = unsafe { &*iov_ptr.offset(i as isize) };
            if iov.iov_base.is_null() || iov.iov_len == 0 {
                continue;
            }

            // Create slice from iovec
            let buf =
                unsafe { core::slice::from_raw_parts(iov.iov_base as *const u8, iov.iov_len) };

            let res = io::sys_write(fd, buf);

            match res {
                Ok(n) => {
                    total_written += n;
                    if n < iov.iov_len {
                        break;
                    } // Partial write
                }
                Err(_) => {
                    if total_written > 0 {
                        return Ok(total_written as u64);
                    } else {
                        return Ok((-1i64) as u64);
                    }
                }
            }
        }

        Ok(total_written as u64)
    });

    // IPC - pipe is critical for shell redirection
    let _ = register_syscall(SYS_PIPE, |args| {
        let pipefd = args[0] as *mut [i32; 2];
        if pipefd.is_null() {
            return Ok((-14i64) as u64); // EFAULT
        }
        let res = ipc::sys_pipe();
        match res {
            Ok((read_fd, write_fd)) => {
                unsafe {
                    (*pipefd)[0] = read_fd;
                    (*pipefd)[1] = write_fd;
                }
                Ok(0)
            }
            Err(_) => Ok((-1i64) as u64),
        }
    });

    // =========================================================================
    // EXISTING SYSCALL REGISTRATIONS (Phases 13-27)
    // =========================================================================

    // Register directory operations (Phase 13)
    let _ = register_syscall(SYS_MKDIR, |args| {
        let path_ptr = args[0] as *const i8;
        let mode = args[1] as u32;
        let res = unsafe { fs_dir::sys_mkdir(path_ptr, mode) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_RMDIR, |args| {
        let path_ptr = args[0] as *const i8;
        let res = unsafe { fs_dir::sys_rmdir(path_ptr) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_SET_ROBUST_LIST as usize, |args| {
        let list_ptr = args[0] as *mut u8;
        let len = args[1] as usize;
        let res = unsafe { fs_futex::sys_set_robust_list(list_ptr, len) };
        Ok(res as u64)
    });

    // Phase 17: Polling & Events
    let _ = register_syscall(SYS_POLL as usize, |args| {
        let fds = args[0] as *mut fs_poll::PollFd;
        let nfds = args[1] as usize;
        let timeout = args[2] as i32;
        let res = unsafe { fs_poll::sys_poll(fds, nfds, timeout) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_PPOLL as usize, |args| {
        let fds = args[0] as *mut fs_poll::PollFd;
        let nfds = args[1] as usize;
        let tmo_p = args[2] as *const TimeSpec;
        let sigmask = args[3] as *const crate::posix_x::signals::SigSet;
        let sigsetsize = args[4] as usize;
        let res = unsafe { fs_poll::sys_ppoll(fds, nfds, tmo_p, sigmask, sigsetsize) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SELECT as usize, |args| {
        let nfds = args[0] as i32;
        let readfds = args[1] as *mut u64;
        let writefds = args[2] as *mut u64;
        let exceptfds = args[3] as *mut u64;
        let timeout = args[4] as *mut TimeSpec;
        let res = unsafe { fs_poll::sys_select(nfds, readfds, writefds, exceptfds, timeout) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_PSELECT6 as usize, |args| {
        let nfds = args[0] as i32;
        let readfds = args[1] as *mut u64;
        let writefds = args[2] as *mut u64;
        let exceptfds = args[3] as *mut u64;
        let timeout = args[4] as *const TimeSpec;
        let sigmask = args[5] as *const u64; // Assuming sigset_t is u64 for simplicity, adjust if needed
        let res =
            unsafe { fs_poll::sys_pselect6(nfds, readfds, writefds, exceptfds, timeout, sigmask) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_EPOLL_CREATE1 as usize, |args| {
        let flags = args[0] as i32;
        let res = unsafe { fs_poll::sys_epoll_create1(flags) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_EPOLL_CTL as usize, |args| {
        let epfd = args[0] as i32;
        let op = args[1] as i32;
        let fd = args[2] as i32;
        let event = args[3] as *mut fs_poll::EpollEvent;
        let res = unsafe { fs_poll::sys_epoll_ctl(epfd, op, fd, event) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_MKFIFO, |args| {
        let path = crate::syscall::dispatch::check_str(args[0])?;
        Ok(fs_fifo::sys_mkfifo(path, args[1] as u32) as isize as u64)
    });
    let _ = register_syscall(SYS_MKNOD, |args| {
        let path = crate::syscall::dispatch::check_str(args[0])?;
        Ok(fs_fifo::sys_mknod(path, args[1] as u32, args[2] as u64) as isize as u64)
    });

    let _ = register_syscall(SYS_EPOLL_WAIT as usize, |args| {
        let epfd = args[0] as i32;
        let events = args[1] as *mut fs_poll::EpollEvent;
        let maxevents = args[2] as i32;
        let timeout = args[3] as i32;
        let res = unsafe { fs_poll::sys_epoll_wait(epfd, events, maxevents, timeout) };
        Ok(res as u64)
    });

    // Memory (Advanced)
    let _ = register_syscall(SYS_MADVISE as usize, |args| {
        let addr = crate::memory::VirtualAddress::new(args[0] as usize);
        let len = args[1] as usize;
        let advice = args[2] as i32;
        memory::sys_madvise(addr, len, advice)
            .map(|_| 0)
            .map_err(memory_err_to_syscall_err)
    });
    let _ = register_syscall(SYS_MINCORE as usize, |args| {
        let addr = crate::memory::VirtualAddress::new(args[0] as usize);
        let len = args[1] as usize;
        let vec = args[2] as *mut u8;
        memory::sys_mincore(addr, len, vec)
            .map(|_| 0)
            .map_err(memory_err_to_syscall_err)
    });
    let _ = register_syscall(SYS_MLOCK as usize, |args| {
        let addr = crate::memory::VirtualAddress::new(args[0] as usize);
        let len = args[1] as usize;
        memory::sys_mlock(addr, len)
            .map(|_| 0)
            .map_err(memory_err_to_syscall_err)
    });
    let _ = register_syscall(SYS_MUNLOCK as usize, |args| {
        let addr = crate::memory::VirtualAddress::new(args[0] as usize);
        let len = args[1] as usize;
        memory::sys_munlock(addr, len)
            .map(|_| 0)
            .map_err(memory_err_to_syscall_err)
    });

    // Resource Limits
    let _ = register_syscall(SYS_GETRLIMIT as usize, |args| {
        let resource = args[0] as u32;
        let rlim = args[1] as *mut process_limits::Rlimit;
        process_limits::sys_getrlimit(resource, rlim)
            .map(|_| 0)
            .map_err(memory_err_to_syscall_err)
    });
    let _ = register_syscall(SYS_SETRLIMIT as usize, |args| {
        let resource = args[0] as u32;
        let rlim = args[1] as *const process_limits::Rlimit;
        process_limits::sys_setrlimit(resource, rlim)
            .map(|_| 0)
            .map_err(memory_err_to_syscall_err)
    });
    let _ = register_syscall(SYS_GETRUSAGE as usize, |args| {
        let who = args[0] as i32;
        let usage = args[1] as *mut process_limits::Rusage;
        process_limits::sys_getrusage(who, usage)
            .map(|_| 0)
            .map_err(memory_err_to_syscall_err)
    });
    let _ = register_syscall(SYS_PRLIMIT64 as usize, |args| {
        let pid = args[0] as u64;
        let resource = args[1] as u32;
        let new_limit = args[2] as *const process_limits::Rlimit;
        let old_limit = args[3] as *mut process_limits::Rlimit;
        process_limits::sys_prlimit64(pid, resource, new_limit, old_limit)
            .map(|_| 0)
            .map_err(memory_err_to_syscall_err)
    });

    // Sockets (Phase 19): Unix Domain Sockets
    let _ = register_syscall(SYS_SOCKET, |args| {
        Ok(net_socket::sys_socket(args[0] as i32, args[1] as i32, args[2] as i32) as u64)
    });
    let _ = register_syscall(SYS_BIND, |args| {
        Ok(net_socket::sys_bind(args[0] as i32, args[1] as *const u8, args[2] as usize) as u64)
    });
    let _ = register_syscall(SYS_LISTEN, |args| {
        Ok(net_socket::sys_listen(args[0] as i32, args[1] as i32) as u64)
    });
    let _ = register_syscall(SYS_ACCEPT, |args| {
        Ok(
            net_socket::sys_accept(args[0] as i32, args[1] as *mut u8, args[2] as *mut usize)
                as u64,
        )
    });
    let _ = register_syscall(SYS_CONNECT, |args| {
        Ok(net_socket::sys_connect(args[0] as i32, args[1] as *const u8, args[2] as usize) as u64)
    });
    let _ = register_syscall(SYS_SENDTO, |args| {
        Ok(net_socket::sys_sendto(
            args[0] as i32,
            args[1] as *const u8,
            args[2] as usize,
            args[3] as i32,
            args[4] as *const u8,
            args[5] as usize,
        ) as u64)
    });
    let _ = register_syscall(SYS_RECVFROM, |args| {
        Ok(net_socket::sys_recvfrom(
            args[0] as i32,
            args[1] as *mut u8,
            args[2] as usize,
            args[3] as i32,
            args[4] as *mut u8,
            args[5] as *mut usize,
        ) as u64)
    });
    let _ = register_syscall(SYS_SEND, |args| {
        Ok(net_socket::sys_send(
            args[0] as i32,
            args[1] as *const u8,
            args[2] as usize,
            args[3] as i32,
        ) as u64)
    });
    let _ = register_syscall(SYS_RECV, |args| {
        Ok(net_socket::sys_recv(
            args[0] as i32,
            args[1] as *mut u8,
            args[2] as usize,
            args[3] as i32,
        ) as u64)
    });
    let _ = register_syscall(SYS_SOCKETPAIR, |args| {
        Ok(net_socket::sys_socketpair(
            args[0] as i32,
            args[1] as i32,
            args[2] as i32,
            args[3] as *mut i32,
        ) as u64)
    });
    let _ = register_syscall(SYS_SHUTDOWN, |args| {
        Ok(net_socket::sys_shutdown(args[0] as i32, args[1] as i32) as u64)
    });
    let _ = register_syscall(SYS_SENDMSG, |args| {
        Ok(net_socket::sys_sendmsg(
            args[0] as i32,
            args[1] as *const net_socket::Msghdr,
            args[2] as i32,
        ) as u64)
    });
    let _ = register_syscall(SYS_RECVMSG, |args| {
        Ok(net_socket::sys_recvmsg(
            args[0] as i32,
            args[1] as *mut net_socket::Msghdr,
            args[2] as i32,
        ) as u64)
    });
    let _ = register_syscall(SYS_SETSOCKOPT, |args| {
        Ok(net_socket::sys_setsockopt(
            args[0] as i32,
            args[1] as i32,
            args[2] as i32,
            args[3] as *const u8,
            args[4] as usize,
        ) as u64)
    });
    let _ = register_syscall(SYS_GETSOCKOPT, |args| {
        Ok(net_socket::sys_getsockopt(
            args[0] as i32,
            args[1] as i32,
            args[2] as i32,
            args[3] as *mut u8,
            args[4] as *mut usize,
        ) as u64)
    });

    let _ = register_syscall(SYS_GETCWD, |args| {
        let buf = args[0] as *mut u8;
        let size = args[1] as usize;
        let res = unsafe { fs_dir::sys_getcwd(buf, size) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_CHDIR, |args| {
        let path_ptr = args[0] as *const i8;
        let res = unsafe { fs_dir::sys_chdir(path_ptr) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_FCHDIR, |args| {
        let fd = args[0] as i32;
        let res = unsafe { fs_dir::sys_fchdir(fd) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_GETDENTS64, |args| {
        let fd = args[0] as i32;
        let dirp = args[1] as *mut u8;
        let count = args[2] as usize;
        let res = unsafe { fs_dir::sys_getdents64(fd, dirp, count) };
        Ok(res as u64)
    });

    // Register file link and rename operations (Phase 14)
    let _ = register_syscall(SYS_LINK, |args| {
        let oldpath = args[0] as *const i8;
        let newpath = args[1] as *const i8;
        let res = unsafe { fs_link::sys_link(oldpath, newpath) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_SYMLINK, |args| {
        let target = args[0] as *const i8;
        let linkpath = args[1] as *const i8;
        let res = unsafe { fs_link::sys_symlink(target, linkpath) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_READLINK, |args| {
        let path = args[0] as *const i8;
        let buf = args[1] as *mut u8;
        let bufsiz = args[2] as usize;
        let res = unsafe { fs_link::sys_readlink(path, buf, bufsiz) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_UNLINK, |args| {
        let path = args[0] as *const i8;
        let res = unsafe { fs_link::sys_unlink(path) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_UNLINKAT, |args| {
        let dirfd = args[0] as i32;
        let path = args[1] as *const i8;
        let flags = args[2] as i32;
        let res = unsafe { fs_link::sys_unlinkat(dirfd, path, flags) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_RENAME, |args| {
        let oldpath = args[0] as *const i8;
        let newpath = args[1] as *const i8;
        let res = unsafe { fs_link::sys_rename(oldpath, newpath) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_RENAMEAT, |args| {
        let olddirfd = args[0] as i32;
        let oldpath = args[1] as *const i8;
        let newdirfd = args[2] as i32;
        let newpath = args[3] as *const i8;
        let res = unsafe { fs_link::sys_renameat(olddirfd, oldpath, newdirfd, newpath) };
        Ok(res as u64)
    });

    // Register file control operations (Phase 15)
    let _ = register_syscall(SYS_DUP, |args| {
        let oldfd = args[0] as i32;
        let res = unsafe { fs_fcntl::sys_dup(oldfd) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_DUP2, |args| {
        let oldfd = args[0] as i32;
        let newfd = args[1] as i32;
        let res = unsafe { fs_fcntl::sys_dup2(oldfd, newfd) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_DUP3, |args| {
        let oldfd = args[0] as i32;
        let newfd = args[1] as i32;
        let flags = args[2] as i32;
        let res = unsafe { fs_fcntl::sys_dup3(oldfd, newfd, flags) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_FCNTL, |args| {
        let fd = args[0] as i32;
        let cmd = args[1] as i32;
        let arg = args[2];
        let res = unsafe { fs_fcntl::sys_fcntl(fd, cmd, arg) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_IOCTL, |args| {
        let fd = args[0] as i32;
        let request = args[1];
        let arg = args[2];
        let res = unsafe { fs_fcntl::sys_ioctl(fd, request, arg) };
        Ok(res as u64)
    });

    // Register signal operations (Phase 11 - Extended)
    let _ = register_syscall(SYS_TKILL as usize, |args| {
        let tid = args[0] as i32;
        let sig = args[1] as u32;
        let res = unsafe { signals::sys_tkill(tid, sig) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_SIGALTSTACK as usize, |args| {
        let ss = args[0] as *const u8;
        let old_ss = args[1] as *mut u8;
        let res = unsafe { signals::sys_sigaltstack(ss, old_ss) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_RT_SIGPENDING as usize, |args| {
        let set = args[0] as *mut crate::posix_x::signals::SigSet;
        let size = args[1] as usize;
        let res = unsafe { signals::sys_rt_sigpending(set, size) };
        Ok(res as u64)
    });

    let _ = register_syscall(SYS_RT_SIGSUSPEND as usize, |args| {
        let mask = args[0] as *const crate::posix_x::signals::SigSet;
        let size = args[1] as usize;
        let res = unsafe { signals::sys_rt_sigsuspend(mask, size) };
        Ok(res as u64)
    });

    // SysV IPC
    let _ = register_syscall(SYS_SHMGET, |args| {
        Ok(ipc_sysv::sys_shmget(args[0] as i32, args[1] as usize, args[2] as i32) as u64)
    });
    let _ = register_syscall(SYS_SHMAT, |args| {
        Ok(ipc_sysv::sys_shmat(args[0] as i32, args[1] as *const u8, args[2] as i32) as u64)
    });
    let _ = register_syscall(SYS_SHMDT, |args| {
        Ok(ipc_sysv::sys_shmdt(args[0] as *const u8) as u64)
    });
    let _ = register_syscall(SYS_SHMCTL, |args| {
        Ok(ipc_sysv::sys_shmctl(
            args[0] as i32,
            args[1] as i32,
            args[2] as *mut ipc_sysv::ShmidDs,
        ) as u64)
    });
    let _ = register_syscall(SYS_SEMGET, |args| {
        Ok(ipc_sysv::sys_semget(args[0] as i32, args[1] as i32, args[2] as i32) as u64)
    });
    let _ = register_syscall(SYS_SEMOP, |args| {
        Ok(ipc_sysv::sys_semop(
            args[0] as i32,
            args[1] as *mut ipc_sysv::Sembuf,
            args[2] as usize,
        ) as u64)
    });
    let _ = register_syscall(SYS_SEMCTL, |args| {
        Ok(ipc_sysv::sys_semctl(
            args[0] as i32,
            args[1] as i32,
            args[2] as i32,
            args[3] as usize,
        ) as u64)
    });
    let _ = register_syscall(SYS_MSGGET, |args| {
        Ok(ipc_sysv::sys_msgget(args[0] as i32, args[1] as i32) as u64)
    });
    let _ = register_syscall(SYS_MSGSND, |args| {
        Ok(ipc_sysv::sys_msgsnd(
            args[0] as i32,
            args[1] as *const u8,
            args[2] as usize,
            args[3] as i32,
        ) as u64)
    });
    let _ = register_syscall(SYS_MSGRCV, |args| {
        Ok(ipc_sysv::sys_msgrcv(
            args[0] as i32,
            args[1] as *mut u8,
            args[2] as usize,
            args[3] as isize,
            args[4] as i32,
        ) as u64)
    });
    let _ = register_syscall(SYS_MSGCTL, |args| {
        Ok(ipc_sysv::sys_msgctl(
            args[0] as i32,
            args[1] as i32,
            args[2] as *mut ipc_sysv::MsqidDs,
        ) as u64)
    });

    // Event/Signal FDs
    let _ = register_syscall(SYS_EVENTFD2, |args| {
        Ok(fs_events::sys_eventfd2(args[0] as u32, args[1] as i32) as u64)
    });
    let _ = register_syscall(SYS_SIGNALFD4, |args| {
        Ok(fs_events::sys_signalfd4(
            args[0] as i32,
            args[1] as *const u64,
            args[2] as usize,
            args[3] as i32,
        ) as u64)
    });

    // File Operations (Phase 23)
    let _ = register_syscall(SYS_TRUNCATE, |args| {
        let path = args[0] as *const i8;
        let length = args[1] as i64;
        let res = unsafe { fs_ops::sys_truncate(path, length) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_FTRUNCATE, |args| {
        let fd = args[0] as i32;
        let length = args[1] as i64;
        let res = unsafe { fs_ops::sys_ftruncate(fd, length) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SYNC, |_args| {
        let res = unsafe { fs_ops::sys_sync() };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_FSYNC, |args| {
        let fd = args[0] as i32;
        let res = unsafe { fs_ops::sys_fsync(fd) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_FDATASYNC, |args| {
        let fd = args[0] as i32;
        let res = unsafe { fs_ops::sys_fdatasync(fd) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SENDFILE, |args| {
        let out_fd = args[0] as i32;
        let in_fd = args[1] as i32;
        let offset = args[2] as *mut i64;
        let count = args[3] as usize;
        let res = unsafe { fs_ops::sys_sendfile(out_fd, in_fd, offset, count) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SPLICE, |args| {
        let fd_in = args[0] as i32;
        let off_in = args[1] as *mut i64;
        let fd_out = args[2] as i32;
        let off_out = args[3] as *mut i64;
        let len = args[4] as usize;
        let flags = args[5] as u32;
        let res = unsafe { fs_ops::sys_splice(fd_in, off_in, fd_out, off_out, len, flags) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_TEE, |args| {
        let fd_in = args[0] as i32;
        let fd_out = args[1] as i32;
        let len = args[2] as usize;
        let flags = args[3] as u32;
        let res = unsafe { fs_ops::sys_tee(fd_in, fd_out, len, flags) };
        Ok(res as u64)
    });

    // System Info & Utilities (Phase 24)
    let _ = register_syscall(SYS_UNAME, |args| {
        let buf = args[0] as *mut sys_info::UtsName;
        let res = unsafe { sys_info::sys_uname(buf) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SYSINFO, |args| {
        let info = args[0] as *mut sys_info::SysInfo;
        let res = unsafe { sys_info::sys_sysinfo(info) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_UMASK, |args| {
        let mask = args[0] as u32;
        let res = unsafe { sys_info::sys_umask(mask) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_GETRANDOM, |args| {
        let buf = args[0] as *mut u8;
        let buflen = args[1] as usize;
        let flags = args[2] as u32;
        let res = unsafe { sys_info::sys_getrandom(buf, buflen, flags) };
        Ok(res as u64)
    });

    // Process Scheduling (Phase 25)
    let _ = register_syscall(SYS_SCHED_YIELD, |_args| {
        let res = unsafe { sched::sys_sched_yield() };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SETPRIORITY, |args| {
        let which = args[0] as i32;
        let who = args[1] as u32;
        let prio = args[2] as i32;
        let res = unsafe { sched::sys_setpriority(which, who, prio) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_GETPRIORITY, |args| {
        let which = args[0] as i32;
        let who = args[1] as u32;
        let res = unsafe { sched::sys_getpriority(which, who) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SCHED_SETSCHEDULER, |args| {
        let pid = args[0] as i32;
        let policy = args[1] as i32;
        let param = args[2] as *const sched::SchedParam;
        let res = unsafe { sched::sys_sched_setscheduler(pid, policy, param) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SCHED_GETSCHEDULER, |args| {
        let pid = args[0] as i32;
        let res = unsafe { sched::sys_sched_getscheduler(pid) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SCHED_SETPARAM, |args| {
        let pid = args[0] as i32;
        let param = args[1] as *const sched::SchedParam;
        let res = unsafe { sched::sys_sched_setparam(pid, param) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SCHED_GETPARAM, |args| {
        let pid = args[0] as i32;
        let param = args[1] as *mut sched::SchedParam;
        let res = unsafe { sched::sys_sched_getparam(pid, param) };
        Ok(res as u64)
    });

    // File Notifications (Phase 26)
    let _ = register_syscall(SYS_INOTIFY_INIT, |_args| {
        let res = unsafe { inotify::sys_inotify_init() };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_INOTIFY_INIT1, |args| {
        let flags = args[0] as i32;
        let res = unsafe { inotify::sys_inotify_init1(flags) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_INOTIFY_ADD_WATCH, |args| {
        let fd = args[0] as i32;
        let pathname = args[1] as *const i8;
        let mask = args[2] as u32;
        let res = unsafe { inotify::sys_inotify_add_watch(fd, pathname, mask) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_INOTIFY_RM_WATCH, |args| {
        let fd = args[0] as i32;
        let wd = args[1] as i32;
        let res = unsafe { inotify::sys_inotify_rm_watch(fd, wd) };
        Ok(res as u64)
    });

    // Advanced Security (Phase 27)
    let _ = register_syscall(SYS_PRCTL, |args| {
        let option = args[0] as i32;
        let arg2 = args[1] as usize;
        let arg3 = args[2] as usize;
        let arg4 = args[3] as usize;
        let arg5 = args[4] as usize;
        let res = unsafe { security::sys_prctl(option, arg2, arg3, arg4, arg5) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_INOTIFY_INIT, |_args| {
        let res = unsafe { inotify::sys_inotify_init() };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_INOTIFY_INIT1, |args| {
        let flags = args[0] as i32;
        let res = unsafe { inotify::sys_inotify_init1(flags) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_INOTIFY_ADD_WATCH, |args| {
        let fd = args[0] as i32;
        let pathname = args[1] as *const i8;
        let mask = args[2] as u32;
        let res = unsafe { inotify::sys_inotify_add_watch(fd, pathname, mask) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_INOTIFY_RM_WATCH, |args| {
        let fd = args[0] as i32;
        let wd = args[1] as i32;
        let res = unsafe { inotify::sys_inotify_rm_watch(fd, wd) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SCHED_YIELD, |_args| {
        let res = unsafe { sched::sys_sched_yield() };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SETPRIORITY, |args| {
        let which = args[0] as i32;
        let who = args[1] as u32;
        let prio = args[2] as i32;
        let res = unsafe { sched::sys_setpriority(which, who, prio) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_GETPRIORITY, |args| {
        let which = args[0] as i32;
        let who = args[1] as u32;
        let res = unsafe { sched::sys_getpriority(which, who) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SCHED_SETSCHEDULER, |args| {
        let pid = args[0] as i32;
        let policy = args[1] as i32;
        let param = args[2] as *const sched::SchedParam;
        let res = unsafe { sched::sys_sched_setscheduler(pid, policy, param) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SCHED_GETSCHEDULER, |args| {
        let pid = args[0] as i32;
        let res = unsafe { sched::sys_sched_getscheduler(pid) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SCHED_SETPARAM, |args| {
        let pid = args[0] as i32;
        let param = args[1] as *const sched::SchedParam;
        let res = unsafe { sched::sys_sched_setparam(pid, param) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SCHED_GETPARAM, |args| {
        let pid = args[0] as i32;
        let param = args[1] as *mut sched::SchedParam;
        let res = unsafe { sched::sys_sched_getparam(pid, param) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_UNAME, |args| {
        let buf = args[0] as *mut sys_info::UtsName;
        let res = unsafe { sys_info::sys_uname(buf) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SYSINFO, |args| {
        let info = args[0] as *mut sys_info::SysInfo;
        let res = unsafe { sys_info::sys_sysinfo(info) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_UMASK, |args| {
        let mask = args[0] as u32;
        let res = unsafe { sys_info::sys_umask(mask) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_GETRANDOM, |args| {
        let buf = args[0] as *mut u8;
        let buflen = args[1] as usize;
        let flags = args[2] as u32;
        let res = unsafe { sys_info::sys_getrandom(buf, buflen, flags) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_TRUNCATE, |args| {
        let path = args[0] as *const i8;
        let length = args[1] as i64;
        let res = unsafe { fs_ops::sys_truncate(path, length) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_FTRUNCATE, |args| {
        let fd = args[0] as i32;
        let length = args[1] as i64;
        let res = unsafe { fs_ops::sys_ftruncate(fd, length) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SYNC, |_args| {
        let res = unsafe { fs_ops::sys_sync() };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_FSYNC, |args| {
        let fd = args[0] as i32;
        let res = unsafe { fs_ops::sys_fsync(fd) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_FDATASYNC, |args| {
        let fd = args[0] as i32;
        let res = unsafe { fs_ops::sys_fdatasync(fd) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SENDFILE, |args| {
        let out_fd = args[0] as i32;
        let in_fd = args[1] as i32;
        let offset = args[2] as *mut i64;
        let count = args[3] as usize;
        let res = unsafe { fs_ops::sys_sendfile(out_fd, in_fd, offset, count) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_SPLICE, |args| {
        let fd_in = args[0] as i32;
        let off_in = args[1] as *mut i64;
        let fd_out = args[2] as i32;
        let off_out = args[3] as *mut i64;
        let len = args[4] as usize;
        let flags = args[5] as u32;
        let res = unsafe { fs_ops::sys_splice(fd_in, off_in, fd_out, off_out, len, flags) };
        Ok(res as u64)
    });
    let _ = register_syscall(SYS_TEE, |args| {
        let fd_in = args[0] as i32;
        let fd_out = args[1] as i32;
        let len = args[2] as usize;
        let flags = args[3] as u32;
        let res = unsafe { fs_ops::sys_tee(fd_in, fd_out, len, flags) };
        Ok(res as u64)
    });

    log::info!("Syscall handlers initialized");
}

fn memory_err_to_syscall_err(
    e: crate::memory::MemoryError,
) -> crate::syscall::dispatch::SyscallError {
    use crate::memory::MemoryError;
    use crate::syscall::dispatch::SyscallError;
    match e {
        MemoryError::OutOfMemory => SyscallError::OutOfMemory,
        MemoryError::InvalidAddress => SyscallError::InvalidArgument,
        MemoryError::AlreadyMapped => SyscallError::AlreadyExists,
        MemoryError::NotMapped => SyscallError::NotFound,
        MemoryError::PermissionDenied => SyscallError::PermissionDenied,
        MemoryError::AlignmentError => SyscallError::InvalidArgument,
        MemoryError::InvalidSize => SyscallError::InvalidArgument,
        MemoryError::NotFound => SyscallError::NotFound,
        MemoryError::InvalidParameter => SyscallError::InvalidArgument,
        MemoryError::Mfile => SyscallError::IoError,
        MemoryError::InternalError(_) => SyscallError::IoError,
    }
}
