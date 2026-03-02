//! Module snapshot/ — snapshots immuables ExoFS (no_std).

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

pub use snapshot::{Snapshot, SnapshotId, SnapshotHeader, SNAPSHOT_MAGIC, flags};
pub use snapshot_create::{SnapshotCreator, SnapshotParams};
pub use snapshot_delete::{SnapshotDeleter, DeleteResult};
pub use snapshot_diff::{SnapshotDiff, SnapshotDiffReport, DiffEntry, DiffKind, SnapshotBlobEnumerator};
pub use snapshot_gc::{SnapshotGc, SnapshotGcReport, SnapshotRetentionPolicy};
pub use snapshot_list::{SnapshotList, SNAPSHOT_LIST};
pub use snapshot_mount::{SnapshotMount, MountPoint, SNAPSHOT_MOUNT};
pub use snapshot_protect::SnapshotProtect;
pub use snapshot_quota::{SnapshotQuota, SnapshotQuotaEntry, SNAPSHOT_QUOTA};
pub use snapshot_restore::{SnapshotRestore, RestoreResult, RestoreSink, SnapshotBlobSource};
pub use snapshot_streaming::{SnapshotStreamer, StreamChunkHeader, StreamWriter};
