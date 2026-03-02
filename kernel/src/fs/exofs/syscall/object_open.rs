//! SYS_EXOFS_OBJECT_OPEN (501) — ouverture d'un objet ExoFS.
//! RÈGLE 9 : copy_from_user() obligatoire.

use alloc::vec::Vec;
use crate::fs::exofs::core::FsError;
use super::validation::{read_user_path_heap, fserr_to_errno, EINVAL};
use super::object_fd::OBJECT_TABLE;

pub mod open_flags {
    pub const O_RDONLY:  u32 = 0x0000;
    pub const O_WRONLY:  u32 = 0x0001;
    pub const O_RDWR:    u32 = 0x0002;
    pub const O_CREAT:   u32 = 0x0040;
    pub const O_TRUNC:   u32 = 0x0200;
    pub const O_APPEND:  u32 = 0x0400;
    pub const O_EXCL:    u32 = 0x0080;
}

/// `exofs_object_open(path_ptr, path_len, flags) -> fd (>0) ou errno (<0)`
pub fn sys_exofs_object_open(
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

    let open_flags = flags as u32;
    match OBJECT_TABLE.open(path_str, open_flags) {
        Ok(fd)  => fd as i64,
        Err(e)  => fserr_to_errno(e),
    }
}
