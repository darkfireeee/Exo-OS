//! poll/epoll ABI guardrails.

use exo_syscall_abi as abi;

pub const EPOLL_CTL_ADD: u32 = abi::EPOLL_CTL_ADD;
pub const EPOLL_CTL_DEL: u32 = abi::EPOLL_CTL_DEL;
pub const EPOLL_CTL_MOD: u32 = abi::EPOLL_CTL_MOD;
pub const EPOLL_CLOEXEC: i32 = abi::EPOLL_CLOEXEC;

pub fn valid_epoll_ctl(op: u32) -> bool {
    matches!(op, EPOLL_CTL_ADD | EPOLL_CTL_DEL | EPOLL_CTL_MOD)
}
