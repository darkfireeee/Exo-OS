//! SYS_EXOFS_OBJECT_READ (502) — lecture d'un objet ExoFS.
//! RÈGLE 9 : copy_to_user() pour écriture vers userspace.

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;
use super::validation::{write_user_buf, fserr_to_errno, EINVAL, ERANGE};
use super::object_fd::OBJECT_TABLE;

/// `exofs_object_read(fd, buf_ptr, count, offset) -> bytes_read ou errno`
pub fn sys_exofs_object_read(
    fd:      u64,
    buf_ptr: u64,
    count:   u64,
    offset:  u64,
    _a5: u64, _a6: u64,
) -> i64 {
    let count = count as usize;
    if count == 0 { return 0; }
    if count > super::validation::EXOFS_BLOB_MAX { return ERANGE; }
    if buf_ptr == 0 { return super::validation::EFAULT; }

    let mut data: Vec<u8> = Vec::new();
    data.try_reserve(count).map_err(|_| super::validation::ENOMEM as i64)
        .and_then(|_| {
            data.resize(count, 0);
            OBJECT_TABLE.read(fd as u32, offset, &mut data)
                .map_err(|e| fserr_to_errno(e))
                .and_then(|n_read| {
                    write_user_buf(buf_ptr, &data[..n_read])?;
                    Ok(n_read as i64)
                })
        })
        .unwrap_or_else(|e| e)
}
