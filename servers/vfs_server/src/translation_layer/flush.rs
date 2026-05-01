//! Flush and durability policy shared by sync-related ops.

use exo_syscall_abi as abi;

pub const SYNC_FILE_RANGE_SUPPORTED_MASK: u32 =
    abi::SYNC_FILE_RANGE_WRITE | abi::SYNC_FILE_RANGE_WAIT_BEFORE | abi::SYNC_FILE_RANGE_WAIT_AFTER;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlushDurability {
    AsyncSubmit,
    WaitBeforeCompletion,
    WaitAfterCompletion,
    FullCommit,
    CacheInvalidate,
}

pub fn validate_sync_file_range_flags(flags: u32) -> bool {
    flags & !SYNC_FILE_RANGE_SUPPORTED_MASK == 0
}

pub fn sync_file_range_waits_for_start(flags: u32) -> bool {
    flags & abi::SYNC_FILE_RANGE_WAIT_BEFORE != 0
}

pub fn sync_file_range_waits_for_completion(flags: u32) -> bool {
    flags & abi::SYNC_FILE_RANGE_WAIT_AFTER != 0
}

pub fn durability_for_sync_file_range(flags: u32) -> FlushDurability {
    if flags & abi::SYNC_FILE_RANGE_WAIT_AFTER != 0 {
        FlushDurability::WaitAfterCompletion
    } else if flags & abi::SYNC_FILE_RANGE_WAIT_BEFORE != 0 {
        FlushDurability::WaitBeforeCompletion
    } else {
        FlushDurability::AsyncSubmit
    }
}
