//! mod.rs — ExoFS snapshot module
//!
//! # Snapshot Module Architecture
//!
//! The `snapshot` module manages the complete lifecycle of snapshots in
//! ExoFS (Exo-OS file system). A snapshot is an immutable and consistent
//! view of the volume state at a given point in time.
//!
//! ## Submodule Organization
//!
//! | Submodule             | Role |
//! |------------------------|------|
//! | `snapshot`             | Fundamental types: `Snapshot`, `SnapshotHeaderDisk`, flags |
//! | `snapshot_list`        | Memory registry (`SNAPSHOT_LIST`): BTreeMap index + stats |
//! | `snapshot_create`      | Creation pipeline, Merkle root calculation (HASH-02) |
//! | `snapshot_delete`      | Safe deletion: protection check + cascade |
//! | `snapshot_diff`        | Comparison of two snapshots (merge-sort O(n)) |
//! | `snapshot_gc`          | Garbage Collector: retention policy (age, count, quota) |
//! | `snapshot_restore`     | Restoration pipeline: verify_blob_id (HASH-02) |
//! | `snapshot_protect`     | WORM protection + PROTECTED flag management |
//! | `snapshot_mount`       | Read-only mount, mount point registry |
//! | `snapshot_quota`       | Per-snapshot quotas + global policy |
//! | `snapshot_streaming`   | Chunked TLV export with headers (HDR-03) |
//!
//! ## Compliance Rules (ExoFS Reference v2)
//!
//! - **ONDISK-03**: no `AtomicXxx` in `#[repr(C)]` structs
//! - **HDR-03**   : magic verified FIRST, then Blake3 checksum
//! - **HASH-02**  : `compute_blob_id` / `verify_blob_id` on RAW data
//! - **OOM-02**   : `try_reserve(1)` before each `Vec::push` / `BTreeMap::insert`
//! - **ARITH-02** : `checked_add` / `checked_mul` for all arithmetic
//! - **WRITE-02** : `bytes_written == expected` verified after each write
//!
//! ## Snapshot Lifecycle
//!
//! ```text
//! SnapshotCreator::create()
//!   └─► SNAPSHOT_LIST.register()
//!         ├─► SNAPSHOT_PROTECT.protect()    (optional)
//!         ├─► SNAPSHOT_QUOTA.set()          (optional)
//!         ├─► SNAPSHOT_MOUNT.mount()        (optional)
//!         ├─► SnapshotStreamer::stream()    (export)
//!         ├─► SnapshotRestore::restore()   (restore)
//!         ├─► SnapshotDiff::compare()      (diff with other snapshot)
//!         └─► SnapshotDeleter::delete()    (deletion)
//!
//! SnapshotGc::run()  ─► SnapshotDeleter::delete()  (x N)
//! ```
//!
//! ## Static Singletons
//!
//! ```text
//! SNAPSHOT_LIST     — global registry (always active)
//! SNAPSHOT_PROTECT  — protection table
//! SNAPSHOT_MOUNT    — mount registry
//! SNAPSHOT_QUOTA    — quota table
//! ```

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
extern crate std;

// ─────────────────────────────────────────────────────────────
// Submodules
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
// Main Re-exports
// ─────────────────────────────────────────────────────────────

// Fundamental types
pub use snapshot::{
    flags as snapshot_flags, make_snapshot_name, verify_snapshot_header, Snapshot, SnapshotChain,
    SnapshotHeaderDisk, SnapshotRef, SNAPSHOT_FORMAT_VERSION, SNAPSHOT_HEADER_SIZE, SNAPSHOT_MAGIC,
    SNAPSHOT_MAX_COUNT, SNAPSHOT_NAME_LEN,
};

// Global registry
pub use snapshot_list::{ListConsistencyError, ListStats, SnapshotList, SNAPSHOT_LIST};

// Creation
pub use snapshot_create::{
    entries_from_raw, BlobEntry, SnapshotBlobSet, SnapshotCreateResult, SnapshotCreator,
    SnapshotParams, SNAPSHOT_MAX_BLOBS, SNAPSHOT_MAX_TOTAL_BYTES,
};

// Deletion
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

// Restore
pub use snapshot_restore::{
    MemBlobSource, NullRestoreSink, RestoreError, RestoreErrorKind, RestoreOptions, RestoreResult,
    RestoreSink, SnapshotBlobSource, SnapshotRestore,
};

// Protection
pub use snapshot_protect::{
    ProtectEntry, ProtectStats, SnapshotProtect, WormPolicy, SNAPSHOT_PROTECT,
};

// Mount
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
// Initialization / Shutdown
// ─────────────────────────────────────────────────────────────

/// Initializes the snapshot subsystem.
///
/// Called once during ExoFS volume mounting.
pub fn init() {
    // Static singletons are initialized via new_const(): no action needed.
}

/// Resets all singletons (used during volume unmounting).
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
    let guard = SNAPSHOT_TEST_LOCK.lock().test_unwrap();
    shutdown();
    guard
}

/// Verifies subsystem internal consistency (health assertions).
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

/// Health report of the snapshot subsystem.
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
