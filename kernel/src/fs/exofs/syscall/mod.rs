//! mod.rs — ExoFS syscalls (SYS 500–520) wiring facade.

pub mod captable;
mod dispatcher;
pub mod epoch_commit;
pub mod export_object;
pub mod gc_trigger;
pub mod get_content_hash;
pub mod import_object;
pub mod object_create;
pub mod object_delete;
pub mod object_fd;
pub mod object_open;
pub mod object_read;
pub mod object_set_meta;
pub mod object_stat;
pub mod object_store;
pub mod object_write;
pub mod open_by_path;
pub mod path_resolve;
pub mod quota_query;
pub mod readdir;
pub mod relation_create;
pub mod relation_query;
pub mod snapshot_create;
pub mod snapshot_list;
pub mod snapshot_mount;
pub mod validation;

pub use dispatcher::{
    all_syscall_names, dispatch_exofs_syscall, dispatch_exofs_syscall_counted,
    exofs_syscall_handler, for_each_syscall_nr, is_exofs_syscall, reset_all_counters,
    syscall_call_count, syscall_name, syscall_number, ExofsSyscallArgs, SYS_EXOFS_COUNT,
    SYS_EXOFS_EPOCH_COMMIT, SYS_EXOFS_EXPORT_OBJECT, SYS_EXOFS_FIRST, SYS_EXOFS_GC_TRIGGER,
    SYS_EXOFS_GET_CONTENT_HASH, SYS_EXOFS_IMPORT_OBJECT, SYS_EXOFS_LAST, SYS_EXOFS_OBJECT_CREATE,
    SYS_EXOFS_OBJECT_DELETE, SYS_EXOFS_OBJECT_OPEN, SYS_EXOFS_OBJECT_READ,
    SYS_EXOFS_OBJECT_SET_META, SYS_EXOFS_OBJECT_STAT, SYS_EXOFS_OBJECT_WRITE,
    SYS_EXOFS_OPEN_BY_PATH, SYS_EXOFS_PATH_RESOLVE, SYS_EXOFS_QUOTA_QUERY, SYS_EXOFS_READDIR,
    SYS_EXOFS_RELATION_CREATE, SYS_EXOFS_RELATION_QUERY, SYS_EXOFS_SNAPSHOT_CREATE,
    SYS_EXOFS_SNAPSHOT_LIST, SYS_EXOFS_SNAPSHOT_MOUNT,
};
pub use epoch_commit::sys_exofs_epoch_commit;
pub use export_object::sys_exofs_export_object;
pub use gc_trigger::sys_exofs_gc_trigger;
pub use get_content_hash::sys_exofs_get_content_hash;
pub use import_object::sys_exofs_import_object;
pub use object_create::sys_exofs_object_create;
pub use object_delete::sys_exofs_object_delete;
pub use object_open::sys_exofs_object_open;
pub use object_read::sys_exofs_object_read;
pub use object_set_meta::sys_exofs_object_set_meta;
pub use object_stat::sys_exofs_object_stat;
pub use object_write::sys_exofs_object_write;
pub use open_by_path::sys_exofs_open_by_path;
pub use path_resolve::sys_exofs_path_resolve;
pub use quota_query::sys_exofs_quota_query;
pub use readdir::sys_exofs_readdir;
pub use relation_create::sys_exofs_relation_create;
pub use relation_query::sys_exofs_relation_query;
pub use snapshot_create::sys_exofs_snapshot_create;
pub use snapshot_list::sys_exofs_snapshot_list;
pub use snapshot_mount::sys_exofs_snapshot_mount;
