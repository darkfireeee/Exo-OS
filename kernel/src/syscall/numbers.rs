//! # syscall/numbers.rs — Ring 0 re-export of canonical `libs/exo-abi` numbers & errno
//!
//! All `SYS_*` (`0..550`), `errno` constants, and range classifiers live in `libs/exo-abi`.

pub use exo_abi::errno::*;
pub use exo_abi::numbers::*;

/// Generates a syscall handler that returns `ENOSYS` with a documented reason.
#[macro_export]
macro_rules! syscall_stub {
    ($name:ident, $reason:expr) => {
        #[allow(non_snake_case)]
        pub fn $name(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
            // STUB: $reason
            $crate::syscall::numbers::ENOSYS
        }
    };
}
