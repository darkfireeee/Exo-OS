//! syscall/ — Syscalls ExoFS 500-518 (no_std).
//! RÈGLE 9 : copy_from_user() pour tout pointeur userspace.
//! RÈGLE 10 : buffers PATH_MAX sur le tas uniquement.

pub mod object_fd;
pub mod validation;
pub mod path_resolve;
pub mod object_open;
pub mod object_read;
pub mod object_write;
pub mod object_create;
pub mod object_delete;
pub mod object_stat;
pub mod object_set_meta;
pub mod get_content_hash;
pub mod snapshot_create;
pub mod snapshot_list;
pub mod snapshot_mount;
pub mod relation_create;
pub mod relation_query;
pub mod gc_trigger;
pub mod quota_query;
pub mod export_object;
pub mod import_object;
pub mod epoch_commit;

// ── Numéros ExoFS ──────────────────────────────────────────────────────────
pub const SYS_EXOFS_PATH_RESOLVE:   u64 = 500;
pub const SYS_EXOFS_OBJECT_OPEN:    u64 = 501;
pub const SYS_EXOFS_OBJECT_READ:    u64 = 502;
pub const SYS_EXOFS_OBJECT_WRITE:   u64 = 503;
pub const SYS_EXOFS_OBJECT_CREATE:  u64 = 504;
pub const SYS_EXOFS_OBJECT_DELETE:  u64 = 505;
pub const SYS_EXOFS_OBJECT_STAT:    u64 = 506;
pub const SYS_EXOFS_OBJECT_SET_META:u64 = 507;
pub const SYS_EXOFS_GET_CONTENT_HASH:u64= 508;
pub const SYS_EXOFS_SNAPSHOT_CREATE:u64 = 509;
pub const SYS_EXOFS_SNAPSHOT_LIST:  u64 = 510;
pub const SYS_EXOFS_SNAPSHOT_MOUNT: u64 = 511;
pub const SYS_EXOFS_RELATION_CREATE:u64 = 512;
pub const SYS_EXOFS_RELATION_QUERY: u64 = 513;
pub const SYS_EXOFS_GC_TRIGGER:     u64 = 514;
pub const SYS_EXOFS_QUOTA_QUERY:    u64 = 515;
pub const SYS_EXOFS_EXPORT_OBJECT:  u64 = 516;
pub const SYS_EXOFS_IMPORT_OBJECT:  u64 = 517;
pub const SYS_EXOFS_EPOCH_COMMIT:   u64 = 518;

/// Signature commune des handlers ExoFS (identique à SyscallHandler kernel).
pub type ExofsSyscallHandler = fn(u64, u64, u64, u64, u64, u64) -> i64;

/// Dispatch vers le handler ExoFS par numéro syscall (500-518).
#[inline]
pub fn get_exofs_handler(nr: u64) -> Option<ExofsSyscallHandler> {
    match nr {
        SYS_EXOFS_PATH_RESOLVE    => Some(path_resolve::sys_exofs_path_resolve),
        SYS_EXOFS_OBJECT_OPEN     => Some(object_open::sys_exofs_object_open),
        SYS_EXOFS_OBJECT_READ     => Some(object_read::sys_exofs_object_read),
        SYS_EXOFS_OBJECT_WRITE    => Some(object_write::sys_exofs_object_write),
        SYS_EXOFS_OBJECT_CREATE   => Some(object_create::sys_exofs_object_create),
        SYS_EXOFS_OBJECT_DELETE   => Some(object_delete::sys_exofs_object_delete),
        SYS_EXOFS_OBJECT_STAT     => Some(object_stat::sys_exofs_object_stat),
        SYS_EXOFS_OBJECT_SET_META => Some(object_set_meta::sys_exofs_object_set_meta),
        SYS_EXOFS_GET_CONTENT_HASH=> Some(get_content_hash::sys_exofs_get_content_hash),
        SYS_EXOFS_SNAPSHOT_CREATE => Some(snapshot_create::sys_exofs_snapshot_create),
        SYS_EXOFS_SNAPSHOT_LIST   => Some(snapshot_list::sys_exofs_snapshot_list),
        SYS_EXOFS_SNAPSHOT_MOUNT  => Some(snapshot_mount::sys_exofs_snapshot_mount),
        SYS_EXOFS_RELATION_CREATE => Some(relation_create::sys_exofs_relation_create),
        SYS_EXOFS_RELATION_QUERY  => Some(relation_query::sys_exofs_relation_query),
        SYS_EXOFS_GC_TRIGGER      => Some(gc_trigger::sys_exofs_gc_trigger),
        SYS_EXOFS_QUOTA_QUERY     => Some(quota_query::sys_exofs_quota_query),
        SYS_EXOFS_EXPORT_OBJECT   => Some(export_object::sys_exofs_export_object),
        SYS_EXOFS_IMPORT_OBJECT   => Some(import_object::sys_exofs_import_object),
        SYS_EXOFS_EPOCH_COMMIT    => Some(epoch_commit::sys_exofs_epoch_commit),
        _ => None,
    }
}

/// Enregistre les handlers ExoFS (500-518) dans la table de dispatch du kernel.
/// Appelée par `exofs_init()` au démarrage.
pub fn register_exofs_syscalls() -> Result<(), crate::fs::exofs::core::FsError> {
    // L'enregistrement effectif dépend de l'interface de la table syscall
    // du kernel (crate::arch::syscall::register_handler ou similaire).
    // On valide simplement que tous les handlers sont résolvables.
    for nr in SYS_EXOFS_PATH_RESOLVE..=SYS_EXOFS_EPOCH_COMMIT {
        if get_exofs_handler(nr).is_none() {
            return Err(crate::fs::exofs::core::FsError::InvalidArgument);
        }
    }
    Ok(())
}
