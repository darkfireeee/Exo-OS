//! # syscall/table/fs/exofs_wrappers.rs — Wrappers pour les appels natifs ExoFS

pub use crate::fs::exofs::syscall::{
    sys_exofs_epoch_commit, sys_exofs_export_object, sys_exofs_gc_trigger,
    sys_exofs_get_content_hash, sys_exofs_import_object, sys_exofs_object_create,
    sys_exofs_object_delete, sys_exofs_object_open, sys_exofs_object_read,
    sys_exofs_object_set_meta, sys_exofs_object_stat, sys_exofs_object_write,
    sys_exofs_open_by_path, sys_exofs_path_resolve, sys_exofs_quota_query, sys_exofs_readdir,
    sys_exofs_relation_create, sys_exofs_relation_query, sys_exofs_snapshot_create,
    sys_exofs_snapshot_list, sys_exofs_snapshot_mount,
};

/// Wrapper ABI pour SYS_EXOFS_OBJECT_SET_META.
///
/// Nouveau contrat: a1=args_ptr(SetMetaArgs), a2=cap_token, a3..a6 ignorés.
pub fn sys_exofs_object_set_meta_abi(
    args_ptr: u64,
    cap_token: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    sys_exofs_object_set_meta(args_ptr, cap_token)
}
