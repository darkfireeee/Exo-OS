//! statx ABI mapping helpers.

use exo_syscall_abi as abi;

pub const BASIC_STATS: u32 = abi::STATX_BASIC_STATS;
pub const ATTR_IMMUTABLE: u64 = abi::STATX_ATTR_IMMUTABLE;
pub const ATTR_VERITY: u64 = abi::STATX_ATTR_VERITY;

pub fn attrs_for_object(class1: bool, immutable: bool) -> u64 {
    let mut attrs = 0u64;
    if class1 {
        attrs |= ATTR_VERITY;
    }
    if immutable {
        attrs |= ATTR_IMMUTABLE;
    }
    attrs
}
