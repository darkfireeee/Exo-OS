//! mod.rs — Dispatcher centralisé des syscalls ExoFS (SYS 500–520)
//!
//! Déclare tous les sous-modules, ré-exporte les handlers, fournit
//! `dispatch_exofs_syscall()` qui route par numéro de syscall.
//! RECUR-01 / OOM-02 / ARITH-02.

// ─────────────────────────────────────────────────────────────────────────────
// Déclarations de modules
// ─────────────────────────────────────────────────────────────────────────────

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
pub mod object_write;
pub mod path_resolve;
pub mod quota_query;
pub mod relation_create;
pub mod relation_query;
pub mod snapshot_create;
pub mod snapshot_list;
pub mod snapshot_mount;
pub mod validation;
// FIX BUG-01 + BUG-02 : nouveaux syscalls ExoFS
pub mod open_by_path;
pub mod readdir;

// ─────────────────────────────────────────────────────────────────────────────
// Ré-exports publics des handlers
// ─────────────────────────────────────────────────────────────────────────────

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
pub use path_resolve::sys_exofs_path_resolve;
pub use quota_query::sys_exofs_quota_query;
pub use relation_create::sys_exofs_relation_create;
pub use relation_query::sys_exofs_relation_query;
pub use snapshot_create::sys_exofs_snapshot_create;
pub use snapshot_list::sys_exofs_snapshot_list;
pub use snapshot_mount::sys_exofs_snapshot_mount;
// BUG-01/BUG-02 handlers
pub use open_by_path::sys_exofs_open_by_path;
pub use readdir::sys_exofs_readdir;

// ─────────────────────────────────────────────────────────────────────────────
// Numéros de syscalls ExoFS
// ─────────────────────────────────────────────────────────────────────────────

pub const SYS_EXOFS_PATH_RESOLVE: u64 = 500;
pub const SYS_EXOFS_OBJECT_OPEN: u64 = 501;
pub const SYS_EXOFS_OBJECT_READ: u64 = 502;
pub const SYS_EXOFS_OBJECT_WRITE: u64 = 503;
pub const SYS_EXOFS_OBJECT_CREATE: u64 = 504;
pub const SYS_EXOFS_OBJECT_DELETE: u64 = 505;
pub const SYS_EXOFS_OBJECT_STAT: u64 = 506;
pub const SYS_EXOFS_OBJECT_SET_META: u64 = 507;
pub const SYS_EXOFS_GET_CONTENT_HASH: u64 = 508;
pub const SYS_EXOFS_SNAPSHOT_CREATE: u64 = 509;
pub const SYS_EXOFS_SNAPSHOT_LIST: u64 = 510;
pub const SYS_EXOFS_SNAPSHOT_MOUNT: u64 = 511;
pub const SYS_EXOFS_RELATION_CREATE: u64 = 512;
pub const SYS_EXOFS_RELATION_QUERY: u64 = 513;
pub const SYS_EXOFS_GC_TRIGGER: u64 = 514;
pub const SYS_EXOFS_QUOTA_QUERY: u64 = 515;
pub const SYS_EXOFS_EXPORT_OBJECT: u64 = 516;
pub const SYS_EXOFS_IMPORT_OBJECT: u64 = 517;
pub const SYS_EXOFS_EPOCH_COMMIT: u64 = 518;
/// FIX BUG-01 : open() POSIX atomique Ring0
pub const SYS_EXOFS_OPEN_BY_PATH: u64 = 519;
/// FIX BUG-02 : getdents64 ExoFS
pub const SYS_EXOFS_READDIR: u64 = 520;

pub const SYS_EXOFS_FIRST: u64 = SYS_EXOFS_PATH_RESOLVE;
pub const SYS_EXOFS_LAST: u64 = SYS_EXOFS_READDIR;
pub const SYS_EXOFS_COUNT: u64 = SYS_EXOFS_LAST - SYS_EXOFS_FIRST + 1;

// ─────────────────────────────────────────────────────────────────────────────
// Erreurs génériques
// ─────────────────────────────────────────────────────────────────────────────

/// errno "numéro de syscall inconnu"
const ENOSYS: i64 = -38;

// ─────────────────────────────────────────────────────────────────────────────
// Arguments d'un syscall ExoFS (registres a1..a6)
// ─────────────────────────────────────────────────────────────────────────────

/// Représentation uniforme des six arguments d'un appel système.
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

// ─────────────────────────────────────────────────────────────────────────────
// Dispatcher principal
// ─────────────────────────────────────────────────────────────────────────────

/// Dispatch un appel système ExoFS vers le handler correspondant.
///
/// # Sécurité
/// Tous les pointeurs userspace (a1..a6) sont validés dans chaque handler.
/// Cette fonction ne déréférence aucun pointeur.
///
/// # RECUR-01
/// Structure `match` plate — aucune récursion, aucune boucle imbriquée.
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
        // FIX BUG-01 : open() POSIX atomique
        SYS_EXOFS_OPEN_BY_PATH => {
            sys_exofs_open_by_path(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        // FIX BUG-02 : getdents64 ExoFS
        SYS_EXOFS_READDIR => {
            sys_exofs_readdir(args.a1, args.a2, args.a3, args.a4, args.a5, args.a6)
        }
        _ => ENOSYS,
    }
}

/// Version C-ABI pour intégration dans la table de dispatch du kernel.
///
/// Signature : `(nr: u64, a1..a6: u64) -> i64`
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

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires de portée publique
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne vrai si `nr` est un syscall ExoFS connu.
#[inline]
pub fn is_exofs_syscall(nr: u64) -> bool {
    nr >= SYS_EXOFS_FIRST && nr <= SYS_EXOFS_LAST
}

/// Retourne le nom textuel d'un syscall ExoFS (pour logs/debug).
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

/// Retourne le numéro de syscall depuis son nom (lookup inverse).
/// RECUR-01 : tableau statique + while.
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

/// Comparaison octet-à-octet de deux tranches.
/// RECUR-01 : while.
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

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques du dispatcher
// ─────────────────────────────────────────────────────────────────────────────

use core::sync::atomic::{AtomicU64, Ordering};

/// Compteurs d'appels par syscall (index = nr - SYS_EXOFS_FIRST).
static SYSCALL_COUNTERS: [AtomicU64; 19] = {
    // RECUR-01 : initialisé statiquement, pas de boucle at runtime
    [
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
    ]
};

/// Incrémente le compteur d'un syscall.
fn count_call(nr: u64) {
    let idx = nr.wrapping_sub(SYS_EXOFS_FIRST) as usize;
    if idx < SYSCALL_COUNTERS.len() {
        SYSCALL_COUNTERS[idx].fetch_add(1, Ordering::Relaxed);
    }
}

/// Dispatcher avec comptabilisation.
pub fn dispatch_exofs_syscall_counted(args: ExofsSyscallArgs) -> i64 {
    count_call(args.nr);
    dispatch_exofs_syscall(args)
}

/// Retourne le compteur d'appels pour un syscall donné.
pub fn syscall_call_count(nr: u64) -> u64 {
    let idx = nr.wrapping_sub(SYS_EXOFS_FIRST) as usize;
    if idx < SYSCALL_COUNTERS.len() {
        SYSCALL_COUNTERS[idx].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// Remet à zéro tous les compteurs (utile en test).
pub fn reset_all_counters() {
    let mut i = 0usize;
    while i < SYSCALL_COUNTERS.len() {
        SYSCALL_COUNTERS[i].store(0, Ordering::Relaxed);
        i = i.wrapping_add(1);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

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
        let nr = syscall_number(name).unwrap();
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
            a1: 0,
            a2: 0,
            a3: 0,
            a4: 0,
            a5: 0,
            a6: 0,
        };
        assert_eq!(dispatch_exofs_syscall(args), ENOSYS);
    }

    #[test]
    fn test_dispatch_null_args_returns_efault() {
        // SYS 500..518 : tous les handlers retournent EFAULT sur a1==0
        let mut nr = SYS_EXOFS_FIRST;
        while nr <= SYS_EXOFS_LAST {
            let args = ExofsSyscallArgs {
                nr,
                a1: 0,
                a2: 0,
                a3: 0,
                a4: 0,
                a5: 0,
                a6: 0,
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
            a1: 0,
            a2: 0,
            a3: 0,
            a4: 0,
            a5: 0,
            a6: 0,
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

// ─────────────────────────────────────────────────────────────────────────────
// Gamme de numéros de syscalls exportée
// ─────────────────────────────────────────────────────────────────────────────

/// Itère tous les numéros de syscalls ExoFS valides.
/// RECUR-01 : while, pas de for.
pub fn for_each_syscall_nr<F: FnMut(u64)>(mut f: F) {
    let mut nr = SYS_EXOFS_FIRST;
    while nr <= SYS_EXOFS_LAST {
        f(nr);
        nr = nr.wrapping_add(1);
    }
}

/// Retourne la liste statique des noms de tous les syscalls.
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
