//! SYS_EXOFS_EXPORT_OBJECT (516) — export d'objet via syscall (no_std).

use crate::fs::exofs::core::{BlobId, FsError};
use super::validation::copy_struct_from_user;

/// Arguments userspace pour SYS_EXOFS_EXPORT_OBJECT.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SysExportObjectArgs {
    pub blob_id:       [u8; 32],
    pub out_buf_ptr:   u64,   // *mut u8 — buffer de destination userspace.
    pub buf_size:      u64,
    pub out_size_ptr:  u64,   // *mut u64 — taille écrite.
    pub format:        u8,    // 0=raw, 1=exoar.
    pub _pad:          [u8; 7],
}

const _: () = assert!(core::mem::size_of::<SysExportObjectArgs>() == 64);

pub fn sys_export_object(args_ptr: u64, _uid: u64) -> i64 {
    let args: SysExportObjectArgs = match copy_struct_from_user(args_ptr) {
        Ok(a)  => a,
        Err(_) => return -14,
    };

    let _blob_id = BlobId::from_raw(args.blob_id);

    // Export raw : lit le blob depuis le cache/storage et écrit vers userspace.
    // L'implémentation complète dépend du storage backend.
    // Pour l'instant, on retourne ENOSYS jusqu'à l'intégration du storage.
    -38 // ENOSYS
}
