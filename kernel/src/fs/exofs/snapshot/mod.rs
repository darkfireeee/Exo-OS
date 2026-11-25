//! mod.rs — ExoFS snapshot module (wiring facade).

mod lifecycle;
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

#[cfg(test)]
pub use lifecycle::reset_for_test;
pub use lifecycle::{init, shutdown, verify_health, HealthReport};
pub use snapshot::{
    flags as snapshot_flags, make_snapshot_name, verify_snapshot_header, Snapshot, SnapshotChain,
    SnapshotHeaderDisk, SnapshotRef, SNAPSHOT_FORMAT_VERSION, SNAPSHOT_HEADER_SIZE, SNAPSHOT_MAGIC,
    SNAPSHOT_MAX_COUNT, SNAPSHOT_NAME_LEN,
};
pub use snapshot_create::{
    entries_from_raw, BlobEntry, SnapshotBlobSet, SnapshotCreateResult, SnapshotCreator,
    SnapshotParams, SNAPSHOT_MAX_BLOBS, SNAPSHOT_MAX_TOTAL_BYTES,
};
pub use snapshot_delete::{DeleteDenyReason, DeleteOptions, DeleteResult, SnapshotDeleter};
pub use snapshot_diff::{
    DiffEntry, DiffKind, DiffOptions, DiffSummary, MetaOnlyEnumerator, SnapshotBlobEnumerator,
    SnapshotDiff, SnapshotDiffReport,
};
pub use snapshot_gc::{
    GcCandidate, GcReason, SnapshotGc, SnapshotGcReport, SnapshotRetentionPolicy,
};
pub use snapshot_list::{ListConsistencyError, ListStats, SnapshotList, SNAPSHOT_LIST};
pub use snapshot_mount::{
    MountId, MountOptions, MountPoint, MountStats, SnapshotMountRegistry, SNAPSHOT_MOUNT,
};
pub use snapshot_protect::{
    ProtectEntry, ProtectStats, SnapshotProtect, WormPolicy, SNAPSHOT_PROTECT,
};
pub use snapshot_quota::{
    GlobalQuotaPolicy, SnapshotQuotaEntry, SnapshotQuotaTable, SNAPSHOT_QUOTA,
};
pub use snapshot_restore::{
    MemBlobSource, NullRestoreSink, RestoreError, RestoreErrorKind, RestoreOptions, RestoreResult,
    RestoreSink, SnapshotBlobSource, SnapshotRestore,
};
pub use snapshot_streaming::{
    MemStreamBlobSource, SnapshotStreamer, StreamBlobSource, StreamChunkHeader, StreamOptions,
    StreamResult, StreamWriter, VecStreamWriter, CHUNK_TYPE_BLOB, CHUNK_TYPE_END,
    CHUNK_TYPE_MANIFEST, STREAM_CHUNK_HDR_SIZE, STREAM_MAGIC,
};
