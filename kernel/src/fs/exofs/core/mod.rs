// kernel/src/fs/exofs/core/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// core/ — Types fondamentaux ExoFS — ZÉRO dépendance externe
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce module contient UNIQUEMENT des types purs sans dépendance sur le reste du
// kernel (sauf core:: et alloc::). Il est importé par tous les autres modules
// ExoFS.

pub mod blob_id;
pub mod config;
pub mod constants;
pub mod epoch_id;
pub mod error;
pub mod flags;
pub mod object_class;
pub mod object_id;
pub mod object_kind;
pub mod rights;
pub mod stats;
pub mod types;
pub mod version;
// DAG-01 : abstraction horodatage interne (remplace arch::time)
pub mod clock;

// ── Re-exports fondamentaux ───────────────────────────────────────────────────

pub use blob_id::{
    blake3_hash, compute_blob_id, hash_concat, merkle_root, verify_blob_id, BlobIdHasher,
};
pub use constants::{
    BLOB_HEADER_MAGIC,
    BLOCK_SIZE,
    CACHE_LINE_SIZE,
    CLASS2_COUNTER_OFFSET,
    CLASS2_MARKER,
    COMPRESS_MIN_SIZE,
    CRC32C_POLY,
    ENCRYPTION_KEY_SIZE,
    ENCRYPTION_NONCE_SIZE,
    ENCRYPTION_TAG_SIZE,
    EPOCH_COMMIT_TIMEOUT_MS,
    EPOCH_MAX_OBJECTS,
    EPOCH_ROOT_MAGIC,
    EPOCH_SLOT_A_OFFSET,
    EPOCH_SLOT_B_OFFSET,
    EPOCH_SLOT_C_FROM_END,
    EPOCH_SLOT_SIZE,
    EXOAR_MAGIC,
    EXOFS_MAGIC,
    FORMAT_VERSION_MAJOR,
    FORMAT_VERSION_MINOR,
    GC_FREE_THRESHOLD_PCT,
    GC_MAX_GREY_QUEUE,
    GC_MIN_EPOCH_DELAY,
    GC_TIMER_INTERVAL_SECS,
    HEAP_START_OFFSET,
    INLINE_DATA_MAX,
    MAX_EXTENTS_PER_OBJECT,
    MAX_OBJECTS_PER_VOLUME,
    MAX_SNAPSHOTS,
    NAME_MAX,
    OBJECT_HEADER_MAGIC,
    // Constantes additionnelles
    PAGE_SIZE,
    PATH_CACHE_CAPACITY,
    PATH_INDEX_MERGE_THRESHOLD,
    PATH_INDEX_SPLIT_THRESHOLD,
    PATH_MAX,
    RELATION_RECORD_MAGIC,
    SB_MIRROR_12K_OFFSET,
    SB_MIRROR_END_FROM_END,
    SB_PRIMARY_OFFSET,
    SNAPSHOT_RECORD_MAGIC,
    SUPERBLOCK_SIZE,
    SYMLINK_MAX_DEPTH,
    WRITEBACK_INTERVAL_MS,
};
pub use error::{ErrorCategory, ErrorSeverity, ExofsError, ExofsResult};
pub use object_class::{ClassOperation, CowPolicy, ObjectClass};
pub use object_id::{new_class1, new_class2};
pub use object_kind::{KindOperation, ObjectKind};
pub use types::{
    BlobId, ByteRange, DiskOffset, EpochId, Extent, InlineData, ObjectId, PhysAddr, SnapshotId,
    TimeSpec,
};
// Module CRC32C intégré dans blob_id.rs
pub use blob_id::crc32c;

pub use config::{ConfigProfile, ExofsConfig, MountOptions, EXOFS_CONFIG};
pub use epoch_id::{
    current_epoch_id, epoch_clamp, epoch_distance, epoch_distance_sane, epoch_gc_eligible,
    epoch_in_window, epoch_prev, max_epoch, min_epoch, next_epoch_id, restore_epoch_counter,
    EpochCommitSummary, EpochRange, EpochState, EpochStats,
};
pub use flags::{
    EpochFlags, ExtentFlags, MigrationFlags, MountFlags, ObjectFlags, ObjectFlagsBuilder,
    SnapshotFlags,
};
pub use rights::{
    has_inspect_content, has_snapshot_create, has_write, would_expose_secret, RightsMask,
    RIGHT_ADMIN, RIGHT_DELETE, RIGHT_EXEC, RIGHT_GC_TRIGGER, RIGHT_INSPECT_CONTENT, RIGHT_READ,
    RIGHT_RELATION_CREATE, RIGHT_SNAPSHOT_CREATE, RIGHT_WRITE,
};
pub use stats::{ExofsStats, ExofsStatsExtended, ExofsStatsSnapshot, EXOFS_STATS};
pub use version::{
    check_feature_dependencies, feature_overhead_pct10, find_migration_path, negotiate_mount,
    negotiate_mount_detailed, total_features_overhead_pct10, validate_version_on_disk,
    version_history_entry, FeatureDependency, FeatureFlags, FeatureFlagsBuilder, FormatVersion,
    MigrationDescriptor, MountCompatibility, VersionHistoryEntry, VersionNegotiationResult,
};
