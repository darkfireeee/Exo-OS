//! renameat2 flags and epoch policy.

use exo_syscall_abi as abi;

pub const RENAME_NOREPLACE: u32 = abi::RENAME_NOREPLACE;
pub const RENAME_EXCHANGE: u32 = abi::RENAME_EXCHANGE;
pub const RENAME_WHITEOUT: u32 = abi::RENAME_WHITEOUT;

pub fn supported_flags(flags: u32) -> bool {
    flags & !(RENAME_NOREPLACE | RENAME_EXCHANGE) == 0
}

pub fn requires_single_epoch_exchange(flags: u32) -> bool {
    flags & RENAME_EXCHANGE != 0
}
