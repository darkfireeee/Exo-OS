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

#[cfg(test)]
extern crate std;

// ─────────────────────────────────────────────────────────────
// Sous-modules
// ─────────────────────────────────────────────────────────────

pub mod snapshot;
pub mod snapshot_create;
pub mod snapshot_delete;
pub mod snapshot_diff;
pub mod snapshot_gc;
pub mod snapshot_list;
pub mod snapshot_mount;
pub mod snapshot_protect;
pub mod snapshot_quota;
pub mod snapshot_restore;
pub mod snapshot_streaming;

// ─────────────────────────────────────────────────────────────
// Re-exports principaux
// ─────────────────────────────────────────────────────────────

// Types fondamentaux
pub use snapshot::{
    flags as snapshot_flags, make_snapshot_name, verify_snapshot_header, Snapshot, SnapshotChain,
    SnapshotHeaderDisk, SnapshotRef, SNAPSHOT_FORMAT_VERSION, SNAPSHOT_HEADER_SIZE, SNAPSHOT_MAGIC,
    SNAPSHOT_MAX_COUNT, SNAPSHOT_NAME_LEN,
};

// Registre global
pub use snapshot_list::{ListConsistencyError, ListStats, SnapshotList, SNAPSHOT_LIST};

// Création
pub use snapshot_create::{
    entries_from_raw, BlobEntry, SnapshotBlobSet, SnapshotCreateResult, SnapshotCreator,
    SnapshotParams, SNAPSHOT_MAX_BLOBS, SNAPSHOT_MAX_TOTAL_BYTES,
};

// Suppression
pub use snapshot_delete::{DeleteDenyReason, DeleteOptions, DeleteResult, SnapshotDeleter};

// Diff
pub use snapshot_diff::{
    DiffEntry, DiffKind, DiffOptions, DiffSummary, MetaOnlyEnumerator, SnapshotBlobEnumerator,
    SnapshotDiff, SnapshotDiffReport,
};

// GC
pub use snapshot_gc::{
    GcCandidate, GcReason, SnapshotGc, SnapshotGcReport, SnapshotRetentionPolicy,
};

// Restauration
pub use snapshot_restore::{
    MemBlobSource, NullRestoreSink, RestoreError, RestoreErrorKind, RestoreOptions, RestoreResult,
    RestoreSink, SnapshotBlobSource, SnapshotRestore,
};

// Protection
pub use snapshot_protect::{
    ProtectEntry, ProtectStats, SnapshotProtect, WormPolicy, SNAPSHOT_PROTECT,
};

// Montage
pub use snapshot_mount::{
    MountId, MountOptions, MountPoint, MountStats, SnapshotMountRegistry, SNAPSHOT_MOUNT,
};

// Quota
pub use snapshot_quota::{
    GlobalQuotaPolicy, SnapshotQuotaEntry, SnapshotQuotaTable, SNAPSHOT_QUOTA,
};

// Streaming
pub use snapshot_streaming::{
    MemStreamBlobSource, SnapshotStreamer, StreamBlobSource, StreamChunkHeader, StreamOptions,
    StreamResult, StreamWriter, VecStreamWriter, CHUNK_TYPE_BLOB, CHUNK_TYPE_END,
    CHUNK_TYPE_MANIFEST, STREAM_CHUNK_HDR_SIZE, STREAM_MAGIC,
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

#[cfg(test)]
static SNAPSHOT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub fn reset_for_test() -> std::sync::MutexGuard<'static, ()> {
    let guard = SNAPSHOT_TEST_LOCK.lock().unwrap();
    shutdown();
    guard
}

/// Vérifie la cohérence interne du sous-système (assertions de santé)
pub fn verify_health() -> HealthReport {
    let list_ok = SNAPSHOT_LIST.verify_consistency().is_ok();
    let n_snaps = SNAPSHOT_LIST.count();
    let n_mounts = SNAPSHOT_MOUNT.n_mounts();
    let n_prot = SNAPSHOT_PROTECT.n_protected();
    let n_worm = SNAPSHOT_PROTECT.n_worm();
    HealthReport {
        list_ok,
        n_snaps,
        n_mounts,
        n_protected: n_prot,
        n_worm,
    }
}

/// Rapport de santé du sous-système snapshot
#[derive(Debug, Clone, Copy)]
pub struct HealthReport {
    pub list_ok: bool,
    pub n_snaps: usize,
    pub n_mounts: usize,
    pub n_protected: usize,
    pub n_worm: usize,
}

impl HealthReport {
    pub fn is_healthy(&self) -> bool {
        self.list_ok
    }
}
