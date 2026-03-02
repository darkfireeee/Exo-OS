//! SYS_EXOFS_OBJECT_WRITE (503) — écriture vers un objet ExoFS.
//! RÈGLE 9 : copy_from_user() pour buffer userspace.

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;
use super::validation::{read_user_buf, fserr_to_errno, EINVAL};
use super::object_fd::OBJECT_TABLE;

/// `exofs_object_write(fd, buf_ptr, count, offset) -> bytes_written ou errno`
pub fn sys_exofs_object_write(
    fd:      u64,
    buf_ptr: u64,
    count:   u64,
    offset:  u64,
    _a5: u64, _a6: u64,
) -> i64 {
    if count == 0 { return 0; }
    let mut data: Vec<u8> = Vec::new();
    if let Err(e) = read_user_buf(buf_ptr, count, &mut data) {
        return e;
    }
    match OBJECT_TABLE.write(fd as u32, offset, &data) {
        Ok(n)  => n as i64,
        Err(e) => fserr_to_errno(e),
    }
}
