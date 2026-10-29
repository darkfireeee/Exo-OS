//! `exo-std` (`std` for `x86_64-unknown-exo`).
//!
//! Provides the Rust standard library surface backed by:
//! - [`exo_alloc`] (`dlmalloc` pure Rust over `SYS_MMAP`/`SYS_MREMAP`/`SYS_MUNMAP`)
//! - [`exo_rt`] (TLS `GenericTcb` `FS_BASE`, 0-syscall `SignalTcb`, `_start` AuxV parser)
//! - [`exo_libc`] (`Pal for ExoOs` over ExoFS `500..=520` and POSIX VFS/socket/process syscalls)
//! - [`os::exo`] (`CapTokenExt`, `ExoFsExt`, `PhoenixSafeExt`, `ExoPledgeExt`)

#![no_std]

pub extern crate alloc as alloc_crate;

pub use core::{
    any, array, ascii, cell, char, clone, cmp, convert, default, fmt, future, hash, hint, iter,
    marker, mem, num, ops, option, pin, ptr, result, slice, str, task,
};

pub use alloc_crate::{borrow, boxed, collections, rc, string, vec};

pub mod os;
pub mod sys;

/// `std::alloc` module.
pub mod alloc {
    pub use alloc_crate::alloc::*;
    pub use crate::sys::exo_os::alloc::System;
}

/// `std::io` module.
pub mod io {
    pub use crate::sys::exo_os::stdio::{
        stderr, stdin, stdout, Error, ErrorKind, Read, Result, Stderr, StderrLock, Stdin,
        StdinLock, Stdout, StdoutLock, Write, _eprint, _print,
    };
}

/// `std::fs` module.
pub mod fs {
    pub use crate::sys::exo_os::fs::{
        create_dir, metadata, read, read_dir, read_to_string, remove_dir, remove_file, rename,
        write, DirEntry, File, Metadata, OpenOptions, ReadDir,
    };
}

/// `std::net` module.
pub mod net {
    pub use crate::sys::exo_os::net::{Epoll, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
}

/// `std::process` module.
pub mod process {
    pub use crate::sys::exo_os::process::{exit, id, Child, Command, ExitStatus, Output, Stdio};
}

/// `std::thread` module.
pub mod thread {
    pub use crate::sys::exo_os::thread::{
        current, sleep, spawn, yield_now, JoinHandle, Thread, ThreadId,
    };
}

/// `std::sync` module.
pub mod sync {
    pub use alloc_crate::sync::{Arc, Weak};
    pub use core::sync::atomic;
    pub use crate::sys::exo_os::locks::{
        Condvar, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
    };
}

/// `std::time` module.
pub mod time {
    pub use crate::sys::exo_os::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
}

/// `std::env` module.
pub mod env {
    pub use crate::sys::exo_os::env::{args, current_dir, set_current_dir, var, vars, Args, Vars};
}

/// `std::path` module.
pub mod path {
    use alloc_crate::string::{String, ToString};

    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct PathBuf {
        inner: String,
    }

    impl PathBuf {
        #[inline]
        pub fn new() -> Self {
            Self {
                inner: String::new(),
            }
        }

        #[inline]
        pub fn from(s: &str) -> Self {
            Self {
                inner: s.to_string(),
            }
        }

        #[inline]
        pub fn as_str(&self) -> &str {
            &self.inner
        }

        pub fn push(&mut self, part: &str) {
            if part.starts_with('/') {
                self.inner = part.to_string();
            } else {
                if !self.inner.is_empty() && !self.inner.ends_with('/') {
                    self.inner.push('/');
                }
                self.inner.push_str(part);
            }
        }
    }

    impl Default for PathBuf {
        fn default() -> Self {
            Self::new()
        }
    }
}

/// `std::prelude` module.
pub mod prelude {
    pub mod rust_2021 {
        pub use core::prelude::rust_2021::*;
        pub use alloc_crate::borrow::ToOwned;
        pub use alloc_crate::boxed::Box;
        pub use alloc_crate::string::{String, ToString};
        pub use alloc_crate::vec::Vec;
        pub use alloc_crate::{format, vec};
        pub use crate::{eprint, eprintln, print, println};
    }
    pub use rust_2021::*;
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::io::_print(core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n")
    };
    ($($arg:tt)*) => {
        $crate::io::_print(core::format_args!("{}\n", core::format_args!($($arg)*)))
    };
}

#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => {
        $crate::io::_eprint(core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! eprintln {
    () => {
        $crate::eprint!("\n")
    };
    ($($arg:tt)*) => {
        $crate::io::_eprint(core::format_args!("{}\n", core::format_args!($($arg)*)))
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os::exo::{CapTokenExt, CapTokenUser};

    #[test]
    fn test_std_mutex_and_rwlock() {
        let m = sync::Mutex::new(41u32);
        {
            let mut g = m.lock().unwrap();
            *g += 1;
        }
        assert_eq!(*m.lock().unwrap(), 42);

        let rw = sync::RwLock::new(10u32);
        assert_eq!(*rw.read().unwrap(), 10);
        *rw.write().unwrap() = 20;
        assert_eq!(*rw.read().unwrap(), 20);
    }

    #[test]
    fn test_std_time_and_os_exo() {
        let t0 = time::Instant::now();
        let elapsed = t0.elapsed();
        assert!(elapsed.as_nanos() < 10_000_000_000);

        let tok = CapTokenUser::current_process_token();
        assert_eq!(tok.generation(), 0);
    }
}
