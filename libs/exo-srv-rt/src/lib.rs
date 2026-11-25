#![no_std]
//! `exo-srv-rt` — Shared `#![no_std]` micro-runtime for Exo-OS Ring 1 servers and drivers.
//!
//! Distinct from `libs/exo-rt` (which provides Ring 3 TLS `GenericTcb`, 0-syscall `SignalTcb`,
//! and `std`-substrate hooks). `exo-srv-rt` deduplicates CRT `_exo_ring1_entry`, `boot_log`,
//! `register_endpoint`, `recv_ipc_timeout`, `send_reply`, and `#[panic_handler]` across all
//! Ring 1 binaries.

pub mod entry;

use core::mem::size_of;
pub use entry::sigreturn_trampoline;
pub use exo_abi::*;
pub use exo_wire::*;

pub mod cap_abi {
    pub use exo_abi::cap::*;
}

pub mod display_abi {
    pub use exo_wire::display::*;
}

pub mod netgrant_abi {
    pub use exo_wire::netgrant::*;
}

/// Emits a kernel boot/service diagnostic message via `SYS_EXO_LOG` (`350`).
#[inline]
pub fn boot_log(msg: &[u8]) {
    unsafe {
        let _ = syscall3(SYS_EXO_LOG, msg.as_ptr() as u64, msg.len() as u64, 1);
    }
}

/// Registers a named Ring 1 IPC endpoint with the kernel/router.
#[inline]
pub fn register_endpoint(name: &[u8], endpoint_id: u64) -> bool {
    let rc = unsafe {
        syscall3(
            SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            endpoint_id,
        )
    };
    rc >= 0
}

/// Resolves a named Ring 1 IPC endpoint.
#[inline]
pub fn lookup_endpoint(name: &[u8]) -> Option<u64> {
    let rc = unsafe { syscall3(SYS_IPC_LOOKUP, name.as_ptr() as u64, name.len() as u64, 0) };
    if rc > 0 {
        Some(rc as u64)
    } else {
        None
    }
}

/// Receives a fixed-size `#[repr(C)]` request on `endpoint_id` with an even millisecond timeout.
/// Returns `Ok(false)` on `ETIMEDOUT`, `Ok(true)` on message receipt, or `Err(errno)`.
#[inline]
pub fn recv_ipc_timeout<T>(endpoint_id: u64, out: &mut T, timeout_ms_even: u64) -> Result<bool, i64> {
    let flags = if timeout_ms_even > 0 {
        IPC_FLAG_TIMEOUT | (timeout_ms_even & !1)
    } else {
        0
    };
    let rc = unsafe {
        syscall4(
            SYS_IPC_RECV,
            endpoint_id,
            out as *mut T as u64,
            size_of::<T>() as u64,
            flags,
        )
    };
    if rc == ETIMEDOUT {
        Ok(false)
    } else if rc < 0 {
        Err(rc)
    } else {
        Ok(true)
    }
}

/// Sends a fixed-size `#[repr(C)]` reply to `destination` (PID or reply endpoint).
#[inline]
pub fn send_reply<T>(destination: u64, reply: &T) -> i64 {
    if destination == 0 {
        return 0;
    }
    unsafe {
        syscall6(
            SYS_IPC_SEND,
            destination,
            reply as *const T as u64,
            size_of::<T>() as u64,
            0,
            0,
            0,
        )
    }
}

/// Declares a canonical Ring 1 `#[panic_handler]` for dual host/`no_std` crates
/// (`#![cfg_attr(target_os = "none", no_std)]`), logging `banner` and spinning.
#[macro_export]
macro_rules! ring1_panic_handler {
    ($banner:expr) => {
        #[cfg(all(target_os = "none", not(test)))]
        #[panic_handler]
        fn __exo_ring1_panic(_info: &core::panic::PanicInfo) -> ! {
            let banner: &[u8] = $banner;
            if !banner.is_empty() {
                $crate::boot_log(banner);
            }
            loop {
                core::hint::spin_loop();
            }
        }
    };
}

/// Declares a canonical `#![no_std]` Ring 1 `#[panic_handler]` for unconditional
/// `#![no_std]` binaries, logging `banner` (if non-empty) and spinning.
#[macro_export]
macro_rules! ring1_nostd_panic_handler {
    ($banner:expr) => {
        #[cfg(not(test))]
        #[panic_handler]
        fn __exo_ring1_panic(_info: &core::panic::PanicInfo) -> ! {
            let banner: &[u8] = $banner;
            if !banner.is_empty() {
                $crate::boot_log(banner);
            }
            loop {
                core::hint::spin_loop();
            }
        }
    };
}

/// Declares a canonical `#![no_std]` Ring 1 `#[panic_handler]` that logs `banner`
/// (if non-empty), calls `SYS_EXIT(1)` so `init_server` can respawn the service, and spins.
#[macro_export]
macro_rules! ring1_panic_exit_handler {
    ($banner:expr) => {
        #[cfg(not(test))]
        #[panic_handler]
        fn __exo_ring1_panic(_info: &core::panic::PanicInfo) -> ! {
            let banner: &[u8] = $banner;
            if !banner.is_empty() {
                $crate::boot_log(banner);
            }
            unsafe {
                let _ = $crate::syscall1($crate::SYS_EXIT, 1);
            }
            loop {
                core::hint::spin_loop();
            }
        }
    };
}

/// Dual host/`no_std` variant of [`ring1_panic_exit_handler!`].
#[macro_export]
macro_rules! ring1_cfg_panic_exit_handler {
    ($banner:expr) => {
        #[cfg(all(target_os = "none", not(test)))]
        #[panic_handler]
        fn __exo_ring1_panic(_info: &core::panic::PanicInfo) -> ! {
            let banner: &[u8] = $banner;
            if !banner.is_empty() {
                $crate::boot_log(banner);
            }
            unsafe {
                let _ = $crate::syscall1($crate::SYS_EXIT, 1);
            }
            loop {
                core::hint::spin_loop();
            }
        }
    };
}
