//! path/mod.rs — Module de résolution de chemins ExoFS
//!
//! Ce module regroupe l ensemble des sous-modules de gestion des chemins :
//!
//! * `path_component` — Composant de chemin validé
//! * `path_index_tree` — Table de hachage interne pour les index
//! * `path_index`      — Index on-disk de répertoire
//! * `path_index_split`— Découpe d index
//! * `path_index_merge`— Fusion d index
//! * `path_cache`      — Cache de résolution
//! * `canonicalize`    — Normalisation itérative de chemins
//! * `symlink`         — Liens symboliques
//! * `path_walker`     — Itération de chemin
//! * `mount_point`     — Points de montage
//! * `namespace`       — Espaces de nommage
//! * `resolver`        — Résolution itérative complète
//!
//! # Règles appliquées
//!
//! | Règle     | Description                                       |
//! |-----------|---------------------------------------------------|
//! | RECUR-01  | Aucune récursion — boucles itératives seulement   |
//! | PATH-07   | Jamais de [u8; PATH_MAX] sur la pile              |
//! | OOM-02    | try_reserve avant tout push / insert              |
//! | ARITH-02  | Arithmétique vérifiée (checked_add / checked_mul) |
//! | HDR-03    | Magic vérifié en premier dans tout parse on-disk  |
//! | ONDISK-03 | Pas d AtomicU64 dans les structs repr(C)          |


// ─────────────────────────────────────────────────────────────────────────────
// Sous-modules
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
// Réexports — path_component
// ─────────────────────────────────────────────────────────────────────────────

pub use path_component::{
    validate_component,
    fnv1a_hash,
    fnv1a_combine,
    PathComponent,
    PathComponentBuf,
    PathParser,
    NAME_MAX,
    PATH_MAX,
};

// ─────────────────────────────────────────────────────────────────────────────
// Réexports — path_index
// ─────────────────────────────────────────────────────────────────────────────

pub use path_index::{
    PathIndex,
    PathIndexHeader,
    PathIndexEntry,
    PATH_INDEX_MAGIC,
};

// ─────────────────────────────────────────────────────────────────────────────
// Réexports — path_cache
// ─────────────────────────────────────────────────────────────────────────────

pub use path_cache::{
    PATH_CACHE,
    PathCache,
    CacheLookup,
    CachePolicy,
    PathCacheStats,
    cached_lookup,
    cache_insert_with_hash,
    init_path_cache,
    invalidate_cache_for_oid,
};

// ─────────────────────────────────────────────────────────────────────────────
// Réexports — canonicalize
// ─────────────────────────────────────────────────────────────────────────────

pub use canonicalize::{
    canonicalize_path,
    canonicalize_to_vec,
    CanonicalPath,
    PathNormalizer,
};

// ─────────────────────────────────────────────────────────────────────────────
// Réexports — symlink
// ─────────────────────────────────────────────────────────────────────────────

pub use symlink::{
    SYMLINK_STORE,
    SYMLINK_MAX_DEPTH,
    SymlinkTarget,
    SymlinkStore,
    SymlinkResolution,
    resolve_symlink_chain,
    register_symlink,
    invalidate_symlink,
    is_valid_symlink_target,
};

// ─────────────────────────────────────────────────────────────────────────────
// Réexports — mount_point
// ─────────────────────────────────────────────────────────────────────────────

pub use mount_point::{
    MOUNT_TABLE,
    MountTable,
    MountPoint,
    MOUNT_TABLE_MAX,
    MOUNT_FLAG_READONLY,
    MOUNT_FLAG_NOEXEC,
    MOUNT_FLAG_NOSUID,
    MOUNT_FLAG_BIND,
    register_mount,
    unregister_mount_by_dir,
    lookup_mount,
    is_mount_point,
};

// ─────────────────────────────────────────────────────────────────────────────
// Réexports — namespace
// ─────────────────────────────────────────────────────────────────────────────

pub use namespace::{
    NAMESPACE_TABLE,
    NamespaceTable,
    Namespace,
    ROOT_NAMESPACE_ID,
    NAMESPACE_TABLE_MAX,
    NS_FLAG_READONLY,
    NS_FLAG_PRIVATE,
    NS_FLAG_SHARED,
    register_namespace,
    root_of,
    lookup_namespace_by_id,
    init_namespaces,
};

// ─────────────────────────────────────────────────────────────────────────────
// Réexports — resolver
// ─────────────────────────────────────────────────────────────────────────────

pub use resolver::{
    PathResolver,
    ResolveContext,
    ResolveFlags,
    ResolveResult,
    resolve_path,
    resolve_path_full,
    resolve_no_follow,
    path_exists,
    resolve_parent,
};

// ─────────────────────────────────────────────────────────────────────────────
// Réexports — path_walker
// ─────────────────────────────────────────────────────────────────────────────

pub use path_walker::{
    PathWalker,
    WalkerState,
    WalkerStepResult,
    WalkerBackend,
    walk_path,
    basename,
    walk_parent,
};

// ─────────────────────────────────────────────────────────────────────────────
// Réexports — path_index_split / merge
// ─────────────────────────────────────────────────────────────────────────────

pub use path_index_split::{PathIndexSplitter, SplitPolicy, SplitResult, SplitMetrics};
pub use path_index_merge::{PathIndexMerger, MergeConflictPolicy, MergeResult, MergeMetrics};

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation du module
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise tous les sous-systèmes du module path.
///
/// À appeler une seule fois au démarrage du noyau, après que l allocateur
/// est opérationnel.
pub fn init(fs_root_oid: crate::fs::exofs::core::ObjectId) {
    path_index::ensure_mount_key_initialized();
    init_path_cache();
    let _ = init_namespaces(fs_root_oid);
}

/// Libère / remet à zéro les ressources du module path.
///
/// À appeler au démontage propre ou en cas d erreur fatale.
pub fn shutdown() {
    PATH_CACHE.flush_all();
    MOUNT_TABLE.flush();
    NAMESPACE_TABLE.flush_non_root();
}

/// Vérifie la santé globale du module.
///
/// Retourne `true` si tous les invariants sont respectés.
pub fn verify_health() -> bool {
    let cache_ok  = PATH_CACHE.active_count() <= 256;
    let mounts_ok = MOUNT_TABLE.count() <= MOUNT_TABLE_MAX;
    let ns_ok     = NAMESPACE_TABLE.count() <= NAMESPACE_TABLE_MAX;
    cache_ok && mounts_ok && ns_ok
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests d intégration
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::fs::exofs::core::ObjectId;

    fn oid(b: u8) -> ObjectId { let mut a = [0u8; 32]; a[0] = b; ObjectId(a) }

    #[test] fn test_canonicalize_reexport() {
        let mut buf = [0u8; PATH_MAX];
        let n = canonicalize_path(b"/a/../b", &mut buf).unwrap();
        assert_eq!(&buf[..n], b"/b");
    }

    #[test] fn test_validate_component_reexport() {
        validate_component(b"hello").unwrap();
        assert!(validate_component(b"").is_err());
        assert!(validate_component(b"bad/comp").is_err());
    }

    #[test] fn test_health_initial() {
        assert!(verify_health());
    }

    #[test] fn test_mount_lookup_missing() {
        let o = oid(200);
        assert!(lookup_mount(&o).is_none());
    }
}
