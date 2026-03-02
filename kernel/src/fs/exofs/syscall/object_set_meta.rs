//! SYS_EXOFS_OBJECT_SET_META (507) — mise à jour des métadonnées d'un objet ExoFS.
//! RÈGLE 9 : copy_from_user() pour buffer userspace.

use alloc::vec::Vec;
use super::validation::{read_user_buf, fserr_to_errno, EINVAL, EFAULT};
use super::object_fd::OBJECT_TABLE;
use crate::fs::exofs::cache::metadata_cache::METADATA_CACHE;

/// Structure meta reçue depuis userspace.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ExofsSetMeta {
    pub flags:   u32,
    pub n_blobs: u32,
    pub size:    u64,
}

const _: () = assert!(core::mem::size_of::<ExofsSetMeta>() == 16);

/// `exofs_object_set_meta(fd, meta_ptr, meta_size) -> 0 ou errno`
pub fn sys_exofs_object_set_meta(
    fd:        u64,
    meta_ptr:  u64,
    meta_size: u64,
    _a4: u64, _a5: u64, _a6: u64,
) -> i64 {
    if meta_ptr == 0 { return EFAULT; }
    if meta_size < core::mem::size_of::<ExofsSetMeta>() as u64 { return EINVAL; }

    let blob_id = match OBJECT_TABLE.get_blob_id(fd as u32) {
        Some(b) => b,
        None    => return super::validation::ENOENT,
    };

    let mut buf: Vec<u8> = Vec::new();
    if let Err(e) = read_user_buf(meta_ptr, core::mem::size_of::<ExofsSetMeta>() as u64, &mut buf) {
        return e;
    }

    // SAFETY: ExofsSetMeta est repr(C) 16B, buffer validé.
    let meta: ExofsSetMeta = unsafe { core::mem::transmute_copy::<[u8; 16], ExofsSetMeta>(&buf[..16].try_into().unwrap()) };

    let raw = blob_id.as_bytes();
    let inode_id = u64::from_le_bytes(raw[0..8].try_into().unwrap_or([0; 8]));
    let im = crate::fs::exofs::cache::metadata_cache::InodeMeta {
        inode_id,
        size:        meta.size,
        flags:       meta.flags,
        n_blobs:     meta.n_blobs as u32,
        cached_tick: 0,
    };
    match METADATA_CACHE.insert(im) {
        Ok(_)  => 0,
        Err(e) => super::validation::fserr_to_errno(e),
    }
}
