//! IPC operation contract for `vfs_server`.

use exo_syscall_abi as syscall;

pub mod copy_file_range;
pub mod dup;
pub mod fadvise;
pub mod pipe;
pub mod poll;
pub mod renameat2;
pub mod statx;
pub mod sync_file_range;

pub const VFS_MOUNT: u32 = 0;
pub const VFS_UMOUNT: u32 = 1;
pub const VFS_RESOLVE: u32 = 2;
pub const VFS_OPEN: u32 = 3;

pub const PATH_PAYLOAD_MAX: usize = 120;

pub const EXOFS_READ_RIGHTS: u64 =
    (syscall::EXOFS_RIGHT_READ | syscall::EXOFS_RIGHT_STAT | syscall::EXOFS_RIGHT_LIST) as u64;
pub const EXOFS_WRITE_RIGHTS: u64 = syscall::EXOFS_RIGHT_READ_WRITE as u64;

pub fn path_payload_to_cstr(
    payload: &[u8],
    out: &mut [u8; PATH_PAYLOAD_MAX + 1],
) -> Result<usize, i64> {
    let path_len = payload
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(payload.len());
    if path_len == 0 || path_len > PATH_PAYLOAD_MAX {
        return Err(syscall::EINVAL);
    }
    out[..path_len].copy_from_slice(&payload[..path_len]);
    out[path_len] = 0;
    Ok(path_len)
}

pub fn open_payload_parts(payload: &[u8]) -> Result<(u64, &[u8]), i64> {
    if payload.is_empty() {
        return Err(syscall::EINVAL);
    }

    let path_like_first = payload[0] == b'/' || payload[0] == b'.';
    if path_like_first {
        return Ok((syscall::O_RDONLY, payload));
    }

    if payload.len() < 5 {
        return Err(syscall::EINVAL);
    }

    Ok((
        u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as u64,
        &payload[4..],
    ))
}

pub fn open_needs_write(flags: u64) -> bool {
    flags
        & (syscall::O_WRONLY
            | syscall::O_RDWR
            | syscall::O_CREAT
            | syscall::O_TRUNC
            | syscall::O_APPEND)
        != 0
}

pub fn exofs_rights_for_open(flags: u64) -> u64 {
    crate::translation_layer::exofs_rights_for_open_flags(flags)
}
