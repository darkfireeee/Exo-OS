//! ExoFS syscall dispatcher, syscall numbers (SYS 500–520), counters, and tests.
//!
//! Extracted from `exofs/syscall/mod.rs` so `mod.rs` remains a pure wiring facade.

use core::sync::atomic::{AtomicU64, Ordering};

use super::{
    sys_exofs_epoch_commit, sys_exofs_export_object, sys_exofs_gc_trigger,
    sys_exofs_get_content_hash, sys_exofs_import_object, sys_exofs_object_create,
    sys_exofs_object_delete, sys_exofs_object_open, sys_exofs_object_read,
    sys_exofs_object_set_meta, sys_exofs_object_stat, sys_exofs_object_write,
    sys_exofs_open_by_path, sys_exofs_path_resolve, sys_exofs_quota_query, sys_exofs_readdir,
    sys_exofs_relation_create, sys_exofs_relation_query, sys_exofs_snapshot_create,
    sys_exofs_snapshot_list, sys_exofs_snapshot_mount,
};

pub use exo_abi::{
    SYS_EXOFS_COUNT, SYS_EXOFS_EPOCH_COMMIT, SYS_EXOFS_EXPORT_OBJECT, SYS_EXOFS_FIRST,
    SYS_EXOFS_GC_TRIGGER, SYS_EXOFS_GET_CONTENT_HASH, SYS_EXOFS_IMPORT_OBJECT, SYS_EXOFS_LAST,
    SYS_EXOFS_OBJECT_CREATE, SYS_EXOFS_OBJECT_DELETE, SYS_EXOFS_OBJECT_OPEN, SYS_EXOFS_OBJECT_READ,
    SYS_EXOFS_OBJECT_SET_META, SYS_EXOFS_OBJECT_STAT, SYS_EXOFS_OBJECT_WRITE,
    SYS_EXOFS_OPEN_BY_PATH, SYS_EXOFS_PATH_RESOLVE, SYS_EXOFS_QUOTA_QUERY, SYS_EXOFS_READDIR,
    SYS_EXOFS_RELATION_CREATE, SYS_EXOFS_RELATION_QUERY, SYS_EXOFS_SNAPSHOT_CREATE,
    SYS_EXOFS_SNAPSHOT_LIST, SYS_EXOFS_SNAPSHOT_MOUNT,
};

const ENOSYS: i64 = -38;

#[derive(Clone, Copy, Debug, Default)]
pub struct ExofsSyscallArgs {
    pub nr: u64,
    pub a1: u64,
    pub a2: u64,
    pub a3: u64,
    pub a4: u64,
    pub a5: u64,
    pub a6: u64,
}

pub fn dispatch_exofs_syscall(args: ExofsSyscallArgs) -> i64 {
    match args.nr {
        SYS_EXOFS_PATH_RESOLVE => {
            sys_exofs_path_resolve(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_OBJECT_OPEN => {
            sys_exofs_object_open(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_OBJECT_READ => {
            sys_exofs_object_read(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_OBJECT_WRITE => {
            sys_exofs_object_write(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_OBJECT_CREATE => {
            sys_exofs_object_create(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_OBJECT_DELETE => {
            sys_exofs_object_delete(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_OBJECT_STAT => {
            sys_exofs_object_stat(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_OBJECT_SET_META => sys_exofs_object_set_meta(args.a1, args.a2),
        SYS_EXOFS_GET_CONTENT_HASH => {
            sys_exofs_get_content_hash(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_SNAPSHOT_CREATE => {
            sys_exofs_snapshot_create(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_SNAPSHOT_LIST => {
            sys_exofs_snapshot_list(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_SNAPSHOT_MOUNT => {
            sys_exofs_snapshot_mount(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_RELATION_CREATE => {
            sys_exofs_relation_create(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_RELATION_QUERY => {
            sys_exofs_relation_query(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_GC_TRIGGER => {
            sys_exofs_gc_trigger(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_QUOTA_QUERY => {
            sys_exofs_quota_query(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_EXPORT_OBJECT => {
            sys_exofs_export_object(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_IMPORT_OBJECT => {
            sys_exofs_import_object(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_EPOCH_COMMIT => {
            sys_exofs_epoch_commit(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_OPEN_BY_PATH => {
            sys_exofs_open_by_path(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        SYS_EXOFS_READDIR => {
            sys_exofs_readdir(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        _ => ENOSYS,
    }
}

#[no_mangle]
pub extern "C" fn exofs_syscall_handler(
    nr: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
    a6: u64,
) -> i64 {
    dispatch_exofs_syscall(ExofsSyscallArgs {
        nr,
        a1,
        a2,
        a3,
        a4,
        a5,
        a6,
    })
}

#[inline]
pub fn is_exofs_syscall(nr: u64) -> bool {
    nr >= SYS_EXOFS_FIRST && nr <= SYS_EXOFS_LAST
}

pub fn syscall_name(nr: u64) -> &'static str {
    match nr {
        SYS_EXOFS_PATH_RESOLVE => "exofs_path_resolve",
        SYS_EXOFS_OBJECT_OPEN => "exofs_object_open",
        SYS_EXOFS_OBJECT_READ => "exofs_object_read",
        SYS_EXOFS_OBJECT_WRITE => "exofs_object_write",
        SYS_EXOFS_OBJECT_CREATE => "exofs_object_create",
        SYS_EXOFS_OBJECT_DELETE => "exofs_object_delete",
        SYS_EXOFS_OBJECT_STAT => "exofs_object_stat",
        SYS_EXOFS_OBJECT_SET_META => "exofs_object_set_meta",
        SYS_EXOFS_GET_CONTENT_HASH => "exofs_get_content_hash",
        SYS_EXOFS_SNAPSHOT_CREATE => "exofs_snapshot_create",
        SYS_EXOFS_SNAPSHOT_LIST => "exofs_snapshot_list",
        SYS_EXOFS_SNAPSHOT_MOUNT => "exofs_snapshot_mount",
        SYS_EXOFS_RELATION_CREATE => "exofs_relation_create",
        SYS_EXOFS_RELATION_QUERY => "exofs_relation_query",
        SYS_EXOFS_GC_TRIGGER => "exofs_gc_trigger",
        SYS_EXOFS_QUOTA_QUERY => "exofs_quota_query",
        SYS_EXOFS_EXPORT_OBJECT => "exofs_export_object",
        SYS_EXOFS_IMPORT_OBJECT => "exofs_import_object",
        SYS_EXOFS_EPOCH_COMMIT => "exofs_epoch_commit",
        SYS_EXOFS_OPEN_BY_PATH => "exofs_open_by_path",
        SYS_EXOFS_READDIR => "exofs_readdir",
        _ => "<unknown_exofs_syscall>",
    }
}

pub fn syscall_number(name: &[u8]) -> Option<u64> {
    const TABLE: &[(&[u8], u64)] = &[
        (b"exofs_path_resolve", SYS_EXOFS_PATH_RESOLVE),
        (b"exofs_object_open", SYS_EXOFS_OBJECT_OPEN),
        (b"exofs_object_read", SYS_EXOFS_OBJECT_READ),
        (b"exofs_object_write", SYS_EXOFS_OBJECT_WRITE),
        (b"exofs_object_create", SYS_EXOFS_OBJECT_CREATE),
        (b"exofs_object_delete", SYS_EXOFS_OBJECT_DELETE),
        (b"exofs_object_stat", SYS_EXOFS_OBJECT_STAT),
        (b"exofs_object_set_meta", SYS_EXOFS_OBJECT_SET_META),
        (b"exofs_get_content_hash", SYS_EXOFS_GET_CONTENT_HASH),
        (b"exofs_snapshot_create", SYS_EXOFS_SNAPSHOT_CREATE),
        (b"exofs_snapshot_list", SYS_EXOFS_SNAPSHOT_LIST),
        (b"exofs_snapshot_mount", SYS_EXOFS_SNAPSHOT_MOUNT),
        (b"exofs_relation_create", SYS_EXOFS_RELATION_CREATE),
        (b"exofs_relation_query", SYS_EXOFS_RELATION_QUERY),
        (b"exofs_gc_trigger", SYS_EXOFS_GC_TRIGGER),
        (b"exofs_quota_query", SYS_EXOFS_QUOTA_QUERY),
        (b"exofs_export_object", SYS_EXOFS_EXPORT_OBJECT),
        (b"exofs_import_object", SYS_EXOFS_IMPORT_OBJECT),
        (b"exofs_epoch_commit", SYS_EXOFS_EPOCH_COMMIT),
        (b"exofs_open_by_path", SYS_EXOFS_OPEN_BY_PATH),
        (b"exofs_readdir", SYS_EXOFS_READDIR),
    ];
    let mut i = 0usize;
    while i < TABLE.len() {
        let (k, v) = TABLE[i];
        if bytes_eq(k, name) {
            return Some(v);
        }
        i = i.wrapping_add(1);
    }
    None
}

fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0usize;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i = i.wrapping_add(1);
    }
    true
}

static SYSCALL_COUNTERS: [AtomicU64; 19] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

fn count_call(nr: u64) {
    let idx = nr.wrapping_sub(SYS_EXOFS_FIRST) as usize;
    if idx < SYSCALL_COUNTERS.len() {
        SYSCALL_COUNTERS[idx].fetch_add(1, Ordering::Relaxed);
    }
}

pub fn dispatch_exofs_syscall_counted(args: ExofsSyscallArgs) -> i64 {
    count_call(args.nr);
    dispatch_exofs_syscall(args)
}

pub fn syscall_call_count(nr: u64) -> u64 {
    let idx = nr.wrapping_sub(SYS_EXOFS_FIRST) as usize;
    if idx < SYSCALL_COUNTERS.len() {
        SYSCALL_COUNTERS[idx].load(Ordering::Relaxed)
    } else {
        0
    }
}

pub fn reset_all_counters() {
    let mut i = 0usize;
    while i < SYSCALL_COUNTERS.len() {
        SYSCALL_COUNTERS[i].store(0, Ordering::Relaxed);
        i = i.wrapping_add(1);
    }
}

pub fn for_each_syscall_nr<F: FnMut(u64)>(mut f: F) {
    let mut nr = SYS_EXOFS_FIRST;
    while nr <= SYS_EXOFS_LAST {
        f(nr);
        nr = nr.wrapping_add(1);
    }
}

pub fn all_syscall_names() -> [&'static str; 19] {
    [
        "exofs_path_resolve",
        "exofs_object_open",
        "exofs_object_read",
        "exofs_object_write",
        "exofs_object_create",
        "exofs_object_delete",
        "exofs_object_stat",
        "exofs_object_set_meta",
        "exofs_get_content_hash",
        "exofs_snapshot_create",
        "exofs_snapshot_list",
        "exofs_snapshot_mount",
        "exofs_relation_create",
        "exofs_relation_query",
        "exofs_gc_trigger",
        "exofs_quota_query",
        "exofs_export_object",
        "exofs_import_object",
        "exofs_epoch_commit",
    ]
}

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_exofs_syscall_valid() {
        assert!(is_exofs_syscall(SYS_EXOFS_PATH_RESOLVE));
        assert!(is_exofs_syscall(SYS_EXOFS_EPOCH_COMMIT));
        assert!(is_exofs_syscall(SYS_EXOFS_READDIR));
    }

    #[test]
    fn test_is_exofs_syscall_invalid() {
        assert!(!is_exofs_syscall(0));
        assert!(!is_exofs_syscall(499));
        assert!(!is_exofs_syscall(521));
    }

    #[test]
    fn test_syscall_count_is_19() {
        assert_eq!(SYS_EXOFS_COUNT, 21);
    }

    #[test]
    fn test_syscall_name_known() {
        assert_eq!(syscall_name(SYS_EXOFS_PATH_RESOLVE), "exofs_path_resolve");
        assert_eq!(syscall_name(SYS_EXOFS_EPOCH_COMMIT), "exofs_epoch_commit");
        assert_eq!(syscall_name(SYS_EXOFS_OPEN_BY_PATH), "exofs_open_by_path");
        assert_eq!(syscall_name(SYS_EXOFS_READDIR), "exofs_readdir");
    }

    #[test]
    fn test_syscall_name_unknown() {
        assert_eq!(syscall_name(0), "<unknown_exofs_syscall>");
    }

    #[test]
    fn test_syscall_number_roundtrip() {
        let name = b"exofs_object_create";
        let nr = syscall_number(name).test_unwrap();
        assert_eq!(nr, SYS_EXOFS_OBJECT_CREATE);
    }

    #[test]
    fn test_syscall_number_unknown() {
        assert!(syscall_number(b"not_a_syscall").is_none());
    }

    #[test]
    fn test_dispatch_unknown_returns_enosys() {
        let args = ExofsSyscallArgs {
            nr: 9999,
            ..ExofsSyscallArgs::default()
        };
        assert_eq!(dispatch_exofs_syscall(args), ENOSYS);
    }

    #[test]
    fn test_dispatch_null_args_returns_efault() {
        let mut nr = SYS_EXOFS_FIRST;
        while nr <= SYS_EXOFS_LAST {
            let args = ExofsSyscallArgs {
                nr,
                ..ExofsSyscallArgs::default()
            };
            let ret = dispatch_exofs_syscall(args);
            assert!(ret < 0, "SYS {} doit retourner négatif sur args nuls", nr);
            nr = nr.wrapping_add(1);
        }
    }

    #[test]
    fn test_counter_increments() {
        reset_all_counters();
        let args = ExofsSyscallArgs {
            nr: SYS_EXOFS_GC_TRIGGER,
            ..ExofsSyscallArgs::default()
        };
        dispatch_exofs_syscall_counted(args);
        assert_eq!(syscall_call_count(SYS_EXOFS_GC_TRIGGER), 1);
        reset_all_counters();
    }

    #[test]
    fn test_counter_out_of_range() {
        assert_eq!(syscall_call_count(0), 0);
        assert_eq!(syscall_call_count(9999), 0);
    }

    #[test]
    fn test_bytes_eq() {
        assert!(bytes_eq(b"hello", b"hello"));
        assert!(!bytes_eq(b"hello", b"world"));
        assert!(!bytes_eq(b"a", b"ab"));
    }
}
