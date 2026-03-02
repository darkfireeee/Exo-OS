//! SYS_EXOFS_OBJECT_STAT (506) — statistiques d'un objet ExoFS.
//! RÈGLE 9 : copy_to_user() pour écriture vers userspace.

use alloc::vec::Vec;
use super::validation::{write_user_buf, fserr_to_errno, EINVAL, EFAULT};
use super::object_fd::OBJECT_TABLE;
use crate::fs::exofs::cache::metadata_cache::METADATA_CACHE;

/// Statistiques écrites vers userspace (structure fixe 48B).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ExofsObjectStat {
    pub blob_id_lo:  u64,
    pub blob_id_hi:  u64,
    pub size:        u64,
    pub flags:       u32,
    pub n_blobs:     u32,
    pub cached_tick: u64,
    pub _pad:        [u8; 8],
}

const _: () = assert!(core::mem::size_of::<ExofsObjectStat>() == 48);

/// `exofs_object_stat(fd, stat_ptr, stat_size) -> 0 ou errno`
pub fn sys_exofs_object_stat(
    fd:        u64,
    stat_ptr:  u64,
    stat_size: u64,
    _a4: u64, _a5: u64, _a6: u64,
) -> i64 {
    if stat_ptr == 0 { return EFAULT; }
    if stat_size < core::mem::size_of::<ExofsObjectStat>() as u64 { return EINVAL; }

    let blob_id = match OBJECT_TABLE.get_blob_id(fd as u32) {
        Some(b) => b,
        None    => return super::validation::ENOENT,
    };

    let raw = blob_id.as_bytes();
    let inode_id = u64::from_le_bytes(raw[0..8].try_into().unwrap_or([0; 8]));
    let blob_lo  = inode_id;
    let blob_hi  = u64::from_le_bytes(raw[8..16].try_into().unwrap_or([0; 8]));

    let (size, flags, n_blobs, tick) = METADATA_CACHE.get(inode_id)
        .map(|m| (m.size, m.flags, m.n_blobs as u32, m.cached_tick))
        .unwrap_or((0, 0, 0, 0));

    let stat = ExofsObjectStat { blob_id_lo: blob_lo, blob_id_hi: blob_hi, size, flags, n_blobs, cached_tick: tick, _pad: [0; 8] };

    // SAFETY: ExofsObjectStat est repr(C) 48B.
    let buf: [u8; 48] = unsafe { core::mem::transmute_copy(&stat) };
    if let Err(e) = write_user_buf(stat_ptr, &buf) { return e; }
    0
}
