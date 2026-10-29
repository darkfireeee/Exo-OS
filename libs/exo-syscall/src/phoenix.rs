//! Primitives ExoPhoenix SSR (`SYS_EXO_RESURRECT = 523`, `SYS_EXOPHOENIX_TEST = 524`).

use crate::{abi, decode_ret, SysResult};

pub const SYS_EXO_RESURRECT: u64 = 523;
pub const SYS_EXOPHOENIX_TEST: u64 = 524;

#[inline]
pub fn trigger_resurrection(mode: u64) -> SysResult<usize> {
    let ret = unsafe { abi::syscall1(SYS_EXO_RESURRECT, mode) };
    decode_ret(ret)
}

#[inline]
pub fn exophoenix_test(subcmd: u64, arg: u64) -> SysResult<usize> {
    let ret = unsafe { abi::syscall2(SYS_EXOPHOENIX_TEST, subcmd, arg) };
    decode_ret(ret)
}
