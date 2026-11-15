//! path/mod.rs — ExoFS path resolution module
//!
//! This module aggregates all path management submodules:
//!
//! * `path_component`   — Validated path component
//! * `path_index_tree`  — Internal hash table for indices
//! * `path_index`       — On-disk directory index
//! * `path_index_split` — Index splitting
//! * `path_index_merge` — Index merging
//! * `path_cache`       — Resolution cache
//! * `canonicalize`     — Iterative path normalization
//! * `symlink`          — Symbolic links
//! * `path_walker`      — Path iteration
//! * `mount_point`      — Mount points
//! * `namespace`        — Namespaces
//! * `resolver`         — Full iterative resolution
//!
//! # Applied Rules
//!
//! | Rule      | Description                                       |
//! |-----------|---------------------------------------------------|
//! | RECUR-01  | No recursion — iterative loops only               |
//! | PATH-07   | Never allocate [u8; PATH_MAX] on stack            |
//! | OOM-02    | try_reserve before any push / insert              |
//! | ARITH-02  | Checked arithmetic (checked_add / checked_mul)    |
//! | HDR-03    | Magic checked first in any on-disk parse          |
//! | ONDISK-03 | No AtomicU64 in repr(C) structs                   |

// ─────────────────────────────────────────────────────────────────────────────
// Submodules
// ─────────────────────────────────────────────────────────────────────────────

pub mod canonicalize;
pub mod mount_point;
pub mod namespace;
pub mod path_cache;
pub mod path_component;
pub mod path_index;
pub mod path_index_merge;
pub mod path_index_split;
pub mod path_index_tree;
pub mod path_walker;
pub mod resolver;
pub mod symlink;

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — path_component
// ─────────────────────────────────────────────────────────────────────────────

pub use path_component::{
    fnv1a_combine, fnv1a_hash, validate_component, PathComponent, PathComponentBuf, PathParser,
    NAME_MAX, PATH_MAX,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — path_index
// ─────────────────────────────────────────────────────────────────────────────

pub use path_index::{PathIndex, PathIndexEntry, PathIndexHeader, PATH_INDEX_MAGIC};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — path_cache
// ─────────────────────────────────────────────────────────────────────────────

pub use path_cache::{
    cache_insert_with_hash, cached_lookup, init_path_cache, invalidate_cache_for_oid, CacheLookup,
    CachePolicy, PathCache, PathCacheStats, PATH_CACHE,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — canonicalize
// ─────────────────────────────────────────────────────────────────────────────

pub use canonicalize::{canonicalize_path, canonicalize_to_vec, CanonicalPath, PathNormalizer};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — symlink
// ─────────────────────────────────────────────────────────────────────────────

pub use symlink::{
    invalidate_symlink, is_valid_symlink_target, register_symlink, resolve_symlink_chain,
    SymlinkResolution, SymlinkStore, SymlinkTarget, SYMLINK_MAX_DEPTH, SYMLINK_STORE,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — mount_point
// ─────────────────────────────────────────────────────────────────────────────

pub use mount_point::{
    is_mount_point, lookup_mount, register_mount, unregister_mount_by_dir, MountPoint, MountTable,
    MOUNT_FLAG_BIND, MOUNT_FLAG_NOEXEC, MOUNT_FLAG_NOSUID, MOUNT_FLAG_READONLY, MOUNT_TABLE,
    MOUNT_TABLE_MAX,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — namespace
// ─────────────────────────────────────────────────────────────────────────────

pub use namespace::{
    init_namespaces, lookup_namespace_by_id, register_namespace, root_of, Namespace,
    NamespaceTable, NAMESPACE_TABLE, NAMESPACE_TABLE_MAX, NS_FLAG_PRIVATE, NS_FLAG_READONLY,
    NS_FLAG_SHARED, ROOT_NAMESPACE_ID,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — resolver
// ─────────────────────────────────────────────────────────────────────────────

pub use resolver::{
    path_exists, resolve_no_follow, resolve_parent, resolve_path, resolve_path_full, PathResolver,
    ResolveContext, ResolveFlags, ResolveResult,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — path_walker
// ─────────────────────────────────────────────────────────────────────────────

pub use path_walker::{
    basename, walk_parent, walk_path, PathWalker, WalkerBackend, WalkerState, WalkerStepResult,
};

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports — path_index_split / merge
// ─────────────────────────────────────────────────────────────────────────────

pub use path_index_merge::{MergeConflictPolicy, MergeMetrics, MergeResult, PathIndexMerger};
pub use path_index_split::{PathIndexSplitter, SplitMetrics, SplitPolicy, SplitResult};

// ─────────────────────────────────────────────────────────────────────────────
// Module initialization
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes all subsystems of the path module.
///
/// To be called once during kernel startup, after the allocator
/// is operational.
pub fn init(fs_root_oid: crate::fs::exofs::core::ObjectId) {
    path_index::ensure_mount_key_initialized();
    init_path_cache();
    let _ = init_namespaces(fs_root_oid);
}

/// Releases / resets resources of the path module.
///
/// To be called during clean unmount or fatal error.
pub fn shutdown() {
    PATH_CACHE.flush_all();
    MOUNT_TABLE.flush();
    NAMESPACE_TABLE.flush_non_root();
}

/// Verifies global module health.
///
/// Returns `true` if all invariants hold.
pub fn verify_health() -> bool {
    let cache_ok = PATH_CACHE.active_count() <= 256;
    let mounts_ok = MOUNT_TABLE.count() <= MOUNT_TABLE_MAX;
    let ns_ok = NAMESPACE_TABLE.count() <= NAMESPACE_TABLE_MAX;
    cache_ok && mounts_ok && ns_ok
}

// ─────────────────────────────────────────────────────────────────────────────
// Integration tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::fs::exofs::core::ObjectId;

    fn oid(b: u8) -> ObjectId {
        let mut a = [0u8; 32];
        a[0] = b;
        ObjectId(a)
    }

    #[test]
    fn test_canonicalize_reexport() {
        let mut buf = [0u8; PATH_MAX];
        let n = canonicalize_path(b"/a/../b", &mut buf).test_unwrap();
        assert_eq!(&buf[..n], b"/b");
    }

    #[test]
    fn test_validate_component_reexport() {
        validate_component(b"hello").test_unwrap();
        assert!(validate_component(b"").is_err());
        assert!(validate_component(b"bad/comp").is_err());
    }

    #[test]
    fn test_health_initial() {
        assert!(verify_health());
    }

    #[test]
    fn test_mount_lookup_missing() {
        let o = oid(200);
        assert!(lookup_mount(&o).is_none());
    }
}
