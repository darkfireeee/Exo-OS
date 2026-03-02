//! SYS_EXOFS_OBJECT_CREATE (504) — création d'un objet ExoFS.
//! RÈGLE 10 : buffer PATH_MAX sur le tas.

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;
use super::validation::{read_user_path_heap, write_user_buf, fserr_to_errno, EINVAL};
use super::object_fd::OBJECT_TABLE;

/// `exofs_object_create(path_ptr, path_len, flags) -> object_id ou errno`
pub fn sys_exofs_object_create(
    path_ptr: u64,
    path_len: u64,
    flags:    u64,
    out_ptr:  u64,
    _a5: u64, _a6: u64,
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
    match OBJECT_TABLE.create(path_str, flags as u32) {
        Ok(id) => {
            if out_ptr != 0 {
                let _ = write_user_buf(out_ptr, &id.to_le_bytes());
            }
            id as i64
        }
        Err(e) => fserr_to_errno(e),
    }
}
