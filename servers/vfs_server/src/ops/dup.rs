//! dup/dup2/dup3 compatibility policy.

use exo_syscall_abi as abi;

pub const SUPPORTED_DUP3_FLAGS: u64 = abi::O_CLOEXEC;

pub fn supported_dup3_flags(flags: u64) -> bool {
    flags & !SUPPORTED_DUP3_FLAGS == 0
}

pub fn dup3_reuses_dup2_path(flags: u64) -> bool {
    supported_dup3_flags(flags)
}
