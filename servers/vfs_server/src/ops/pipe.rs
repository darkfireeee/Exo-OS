//! pipe/pipe2 compatibility policy.

use exo_syscall_abi as abi;

pub const PIPE_BUF: usize = 4096;
pub const SUPPORTED_PIPE_FLAGS: u64 = abi::O_CLOEXEC | abi::O_NONBLOCK;

pub fn supported_pipe_flags(flags: u64) -> bool {
    flags & !SUPPORTED_PIPE_FLAGS == 0
}
