//! SYS_EXOFS_IMPORT_OBJECT (517) — import d'objet via syscall (no_std).
//! RÈGLE 9 : copy_from_user() pour tous les pointeurs userspace.
//! RÈGLE 11 : BlobId calculé sur les données brutes avant compression.

use crate::fs::exofs::core::{BlobId, FsError};
use super::validation::copy_struct_from_user;

/// Arguments userspace pour SYS_EXOFS_IMPORT_OBJECT.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SysImportObjectArgs {
    pub src_ptr:       u64,   // *const u8 — données à importer (userspace).
    pub src_size:      u64,
    pub format:        u8,    // 0=raw, 1=exoar.
    pub _pad:          [u8; 7],
    pub out_blob_id:   u64,   // *mut [u8;32] — BlobId résultant.
}

const _: () = assert!(core::mem::size_of::<SysImportObjectArgs>() == 32);

pub fn sys_import_object(args_ptr: u64, _uid: u64) -> i64 {
    let args: SysImportObjectArgs = match copy_struct_from_user(args_ptr) {
        Ok(a)  => a,
        Err(_) => return -14,
    };

    if args.src_size == 0 || args.src_size > 256 * 1024 * 1024 {
        return -22; // EINVAL
    }

    // Allouer un buffer heap pour lire les données userspace (RÈGLE 10).
    use alloc::vec::Vec;
    let size = args.src_size as usize;
    let mut buf: Vec<u8> = Vec::new();
    if buf.try_reserve(size).is_err() { return -12; }
    buf.resize(size, 0);

    // RÈGLE 9 : copy_from_user obligatoire.
    if unsafe { crate::fs::exofs::syscall::validation::copy_from_user(
        args.src_ptr as *const u8, buf.as_mut_ptr(), size
    )}.is_err() {
        return -14;
    }

    // RÈGLE 11 : BlobId = Blake3(données brutes avant toute transformation).
    let blob_id = BlobId::from_bytes_blake3(&buf);
    let raw = blob_id.as_bytes();

    if unsafe { crate::fs::exofs::syscall::validation::copy_to_user(
        args.out_blob_id as *mut u8, raw.as_ptr(), 32
    )}.is_err() {
        return -14;
    }

    0
}
