//! System bindings for Exo-OS (`target_os = "exo"`).
//!
//! Pure-Rust Platform Abstraction Layer over Exo-OS Ring 0/Ring 1 syscalls (`0..=549`),
//! `SignalTcb` 0-syscall (`AT_SIGNAL_TCB = 51`), `CapTokenUser` (`AT_CAP_TOKEN = 52`),
//! and `dlmalloc` over `SYS_MMAP`/`SYS_MREMAP`/`SYS_MUNMAP`.

#![deny(unsafe_op_in_unsafe_fn)]
#![allow(missing_docs, nonstandard_style)]

use crate::io;
use crate::vec::Vec;

pub mod alloc_dlmalloc;
pub mod futex;
pub mod os;
pub mod runtime;
pub mod syscall;
pub mod time;

pub mod libc {
    pub const STDIN_FILENO: i32 = 0;
    pub const STDOUT_FILENO: i32 = 1;
    pub const STDERR_FILENO: i32 = 2;

    #[inline]
    pub unsafe fn close(fd: i32) -> i32 {
        match super::syscall::close(fd) {
            Ok(()) => 0,
            Err(_) => -1,
        }
    }
}

#[inline]
pub fn map_exo_err(err: syscall::SysError) -> io::Error {
    io::Error::from_raw_os_error(err.errno())
}

#[inline]
pub fn cstr_bytes(bytes: &[u8]) -> io::Result<Vec<u8>> {
    if bytes.contains(&0) {
        return Err(io::const_error!(
            io::ErrorKind::InvalidInput,
            "file name contained an unexpected NUL byte"
        ));
    }
    let mut v = Vec::with_capacity(bytes.len() + 1);
    v.extend_from_slice(bytes);
    v.push(0);
    Ok(v)
}

pub fn unsupported<T>() -> io::Result<T> {
    Err(unsupported_err())
}

pub fn unsupported_err() -> io::Error {
    io::const_error!(io::ErrorKind::Unsupported, "operation not supported on Exo-OS yet")
}

pub fn abort_internal() -> ! {
    syscall::exit_group(134)
}

pub fn exit_group(code: i32) -> ! {
    syscall::exit_group(code)
}

// SAFETY: must be called only once during runtime initialization.
pub unsafe fn init(argc: isize, argv: *const *const u8, _sigpipe: u8) {
    unsafe {
        crate::sys::args::init(argc, argv);
    }
}

// SAFETY: must be called only once during runtime cleanup.
pub unsafe fn cleanup() {}

#[cfg(all(not(test), target_arch = "x86_64"))]
core::arch::global_asm!(
    ".section .ltext._start,\"ax\",@progbits",
    ".global _start",
    ".type _start, @function",
    "_start:",
    "xor rbp, rbp",
    "mov rdi, rsp",
    "and rsp, -16",
    "call {crt_start}",
    "ud2",
    crt_start = sym __exo_crt_start,
);

#[cfg(not(test))]
unsafe extern "C" fn __exo_crt_start(initial_sp: *const usize) -> ! {
    unsafe extern "C" {
        fn main(argc: isize, argv: *const *const u8) -> i32;
    }

    let (argc, argv, envp) = unsafe { runtime::exo_rt_init_from_stack(initial_sp) };
    unsafe {
        crate::sys::env::init_from_envp(envp);
    }

    let status = unsafe { main(argc as isize, argv) };
    syscall::exit_group(status)
}
