//! sync_file_range ABI helpers.

use exo_syscall_abi as abi;

pub const WRITE: u32 = abi::SYNC_FILE_RANGE_WRITE;
pub const WAIT_BEFORE: u32 = abi::SYNC_FILE_RANGE_WAIT_BEFORE;
pub const WAIT_AFTER: u32 = abi::SYNC_FILE_RANGE_WAIT_AFTER;

pub fn validate_flags(flags: u32) -> bool {
    crate::translation_layer::validate_sync_file_range_flags(flags)
}

pub fn waits_for_completion(flags: u32) -> bool {
    crate::translation_layer::sync_file_range_waits_for_completion(flags)
}
