//! path/mod.rs — ExoFS path resolution module (wiring facade).

pub mod canonicalize;
mod lifecycle;
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

pub use canonicalize::{canonicalize_path, canonicalize_to_vec, CanonicalPath, PathNormalizer};
pub use lifecycle::{init, shutdown, verify_health};
pub use mount_point::{
    is_mount_point, lookup_mount, register_mount, unregister_mount_by_dir, MountPoint, MountTable,
    MOUNT_FLAG_BIND, MOUNT_FLAG_NOEXEC, MOUNT_FLAG_NOSUID, MOUNT_FLAG_READONLY, MOUNT_TABLE,
    MOUNT_TABLE_MAX,
};
pub use namespace::{
    init_namespaces, lookup_namespace_by_id, register_namespace, root_of, Namespace,
    NamespaceTable, NAMESPACE_TABLE, NAMESPACE_TABLE_MAX, NS_FLAG_PRIVATE, NS_FLAG_READONLY,
    NS_FLAG_SHARED, ROOT_NAMESPACE_ID,
};
pub use path_cache::{
    cache_insert_with_hash, cached_lookup, init_path_cache, invalidate_cache_for_oid, CacheLookup,
    CachePolicy, PathCache, PathCacheStats, PATH_CACHE,
};
pub use path_component::{
    fnv1a_combine, fnv1a_hash, validate_component, PathComponent, PathComponentBuf, PathParser,
    NAME_MAX, PATH_MAX,
};
pub use path_index::{PathIndex, PathIndexEntry, PathIndexHeader, PATH_INDEX_MAGIC};
pub use path_index_merge::{MergeConflictPolicy, MergeMetrics, MergeResult, PathIndexMerger};
pub use path_index_split::{PathIndexSplitter, SplitMetrics, SplitPolicy, SplitResult};
pub use path_walker::{
    basename, walk_parent, walk_path, PathWalker, WalkerBackend, WalkerState, WalkerStepResult,
};
pub use resolver::{
    path_exists, resolve_no_follow, resolve_parent, resolve_path, resolve_path_full, PathResolver,
    ResolveContext, ResolveFlags, ResolveResult,
};
pub use symlink::{
    invalidate_symlink, is_valid_symlink_target, register_symlink, resolve_symlink_chain,
    SymlinkResolution, SymlinkStore, SymlinkTarget, SYMLINK_MAX_DEPTH, SYMLINK_STORE,
};
