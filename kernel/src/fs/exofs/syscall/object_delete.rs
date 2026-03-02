//! SYS_EXOFS_OBJECT_DELETE (505) — suppression d'un objet ExoFS.

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;
use super::validation::{read_user_path_heap, fserr_to_errno, EINVAL};
use super::object_fd::OBJECT_TABLE;

/// `exofs_object_delete(path_ptr, path_len, flags) -> 0 ou errno`
pub fn sys_exofs_object_delete(
    path_ptr: u64,
    path_len: u64,
    flags:    u64,
    _a4: u64, _a5: u64, _a6: u64,
) -> i64 {
    let mut path_buf: Vec<u8> = Vec::new();
    let len = match read_user_path_heap(path_ptr, &mut path_buf) {
        Ok(l)  => l,
        Err(e) => return e,
    };
    let path_str = match core::str::from_utf8(&path_buf[..len]) {
        Ok(s)  => s,
        Err(_) => return EINVAL,
    };
    match OBJECT_TABLE.delete(path_str) {
        Ok(()) => 0,
        Err(e) => fserr_to_errno(e),
    }
}
