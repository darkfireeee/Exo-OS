//! mod.rs — Module snapshot ExoFS
//!
//! # Architecture du module snapshot
//!
//! Le module `snapshot` gère le cycle de vie complet des snapshots dans
//! ExoFS (système de fichiers Exo-OS). Un snapshot est une vue immutable
//! et cohérente de l'état d'un volume à un instant donné.
//!
//! ## Organisation des sous-modules
//!
//! | Sous-module            | Rôle |
//! |------------------------|------|
//! | `snapshot`             | Types fondamentaux : `Snapshot`, `SnapshotHeaderDisk`, flags |
//! | `snapshot_list`        | Registre mémoire (`SNAPSHOT_LIST`) : index BTreeMap + stats |
//! | `snapshot_create`      | Pipeline de création, calcul de racine Merkle (HASH-02) |
//! | `snapshot_delete`      | Suppression sécurisée : vérif protection + cascade |
//! | `snapshot_diff`        | Comparaison de deux snapshots (merge-sort O(n)) |
//! | `snapshot_gc`          | Garbage Collector : politique rétention (âge, count, quota) |
//! | `snapshot_restore`     | Restauration pipeline : verify_blob_id (HASH-02) |
//! | `snapshot_protect`     | Protection WORM + gestion du flag PROTECTED |
//! | `snapshot_mount`       | Montage en lecture seule, registre de points de montage |
//! | `snapshot_quota`       | Quotas par snapshot + politique globale |
//! | `snapshot_streaming`   | Export TLV chunked avec headers (HDR-03) |
//!
//! ## Règles de conformité (ExoFS Reference v2)
//!
//! - **ONDISK-03** : aucun `AtomicXxx` dans les structs `#[repr(C)]`
//! - **HDR-03**    : magic vérifié EN PREMIER, puis checksum Blake3
//! - **HASH-02**   : `compute_blob_id` / `verify_blob_id` sur données RAW
//! - **OOM-02**    : `try_reserve(1)` avant chaque `Vec::push` / `BTreeMap::insert`
//! - **ARITH-02**  : `checked_add` / `checked_mul` pour toute arithmétique
//! - **WRITE-02**  : `bytes_written == expected` vérifié après chaque écriture
//!
//! ## Cycle de vie d'un snapshot
//!
//! ```text
//! SnapshotCreator::create()
//!   └─► SNAPSHOT_LIST.register()
//!         ├─► SNAPSHOT_PROTECT.protect()    (optionnel)
//!         ├─► SNAPSHOT_QUOTA.set()          (optionnel)
//!         ├─► SNAPSHOT_MOUNT.mount()        (optionnel)
//!         ├─► SnapshotStreamer::stream()    (export)
//!         ├─► SnapshotRestore::restore()   (restauration)
//!         ├─► SnapshotDiff::compare()      (diff avec autre snapshot)
//!         └─► SnapshotDeleter::delete()    (suppression)
//!
//! SnapshotGc::run()  ─► SnapshotDeleter::delete()  (x N)
//! ```
//!
//! ## Singletons statiques
//!
//! ```text
//! SNAPSHOT_LIST     — registre global (toujours actif)
//! SNAPSHOT_PROTECT  — table des protections
//! SNAPSHOT_MOUNT    — registre des montages
//! SNAPSHOT_QUOTA    — table des quotas
//! ```

// ─────────────────────────────────────────────────────────────
// Sous-modules
// ─────────────────────────────────────────────────────────────

pub mod snapshot;
pub mod snapshot_list;
pub mod snapshot_create;
pub mod snapshot_delete;
pub mod snapshot_diff;
pub mod snapshot_gc;
pub mod snapshot_restore;
pub mod snapshot_protect;
pub mod snapshot_mount;
pub mod snapshot_quota;
pub mod snapshot_streaming;

// ─────────────────────────────────────────────────────────────
// Re-exports principaux
// ─────────────────────────────────────────────────────────────

// Types fondamentaux
pub use snapshot::{
    Snapshot, SnapshotRef, SnapshotChain,
    SnapshotHeaderDisk, flags as snapshot_flags,
    SNAPSHOT_MAGIC, SNAPSHOT_HEADER_SIZE, SNAPSHOT_FORMAT_VERSION,
    SNAPSHOT_NAME_LEN, SNAPSHOT_MAX_COUNT,
    verify_snapshot_header, make_snapshot_name,
};

// Registre global
pub use snapshot_list::{
    SNAPSHOT_LIST, SnapshotList, ListStats, ListConsistencyError,
};

// Création
pub use snapshot_create::{
    SnapshotCreator, SnapshotParams, SnapshotCreateResult,
    SnapshotBlobSet, BlobEntry,
    SNAPSHOT_MAX_BLOBS, SNAPSHOT_MAX_TOTAL_BYTES,
    entries_from_raw,
};

// Suppression
pub use snapshot_delete::{
    SnapshotDeleter, DeleteOptions, DeleteResult,
    DeleteDenyReason,
};

// Diff
pub use snapshot_diff::{
    SnapshotDiff, SnapshotDiffReport, DiffEntry, DiffKind, DiffOptions, DiffSummary,
    SnapshotBlobEnumerator, MetaOnlyEnumerator,
};

// GC
pub use snapshot_gc::{
    SnapshotGc, SnapshotGcReport, SnapshotRetentionPolicy,
    GcCandidate, GcReason,
};

// Restauration
pub use snapshot_restore::{
    SnapshotRestore, RestoreResult, RestoreError, RestoreErrorKind, RestoreOptions,
    RestoreSink, SnapshotBlobSource,
    NullRestoreSink, MemBlobSource,
};

// Protection
pub use snapshot_protect::{
    SNAPSHOT_PROTECT, SnapshotProtect, ProtectEntry, WormPolicy, ProtectStats,
};

// Montage
pub use snapshot_mount::{
    SNAPSHOT_MOUNT, SnapshotMountRegistry, MountPoint, MountId, MountOptions, MountStats,
};

// Quota
pub use snapshot_quota::{
    SNAPSHOT_QUOTA, SnapshotQuotaTable, SnapshotQuotaEntry, GlobalQuotaPolicy,
};

// Streaming
pub use snapshot_streaming::{
    SnapshotStreamer, StreamChunkHeader, StreamResult, StreamOptions,
    StreamWriter, StreamBlobSource,
    VecStreamWriter, MemStreamBlobSource,
    STREAM_MAGIC, STREAM_CHUNK_HDR_SIZE,
    CHUNK_TYPE_MANIFEST, CHUNK_TYPE_BLOB, CHUNK_TYPE_END,
};

// ─────────────────────────────────────────────────────────────
// Initialisation / Shutdown
// ─────────────────────────────────────────────────────────────

/// Initialise le sous-système snapshot
///
/// Appelé une seule fois lors du montage du volume ExoFS.
pub fn init() {
    // Les singletons statiques sont initialisés via new_const() : pas d'action nécessaire.
}

/// Réinitialise tous les singletons (utilisé lors du démontage du volume)
pub fn shutdown() {
    SNAPSHOT_LIST.clear();
    SNAPSHOT_PROTECT.clear();
    SNAPSHOT_MOUNT.clear();
    SNAPSHOT_QUOTA.clear();
}

/// Vérifie la cohérence interne du sous-système (assertions de santé)
pub fn verify_health() -> HealthReport {
    let list_ok  = SNAPSHOT_LIST.verify_consistency().is_ok();
    let n_snaps  = SNAPSHOT_LIST.count();
    let n_mounts = SNAPSHOT_MOUNT.n_mounts();
    let n_prot   = SNAPSHOT_PROTECT.n_protected();
    let n_worm   = SNAPSHOT_PROTECT.n_worm();
    HealthReport { list_ok, n_snaps, n_mounts, n_protected: n_prot, n_worm }
}

/// Rapport de santé du sous-système snapshot
#[derive(Debug, Clone, Copy)]
pub struct HealthReport {
    pub list_ok:     bool,
    pub n_snaps:     usize,
    pub n_mounts:    usize,
    pub n_protected: usize,
    pub n_worm:      usize,
}

impl HealthReport {
    pub fn is_healthy(&self) -> bool { self.list_ok }
}
