// kernel/src/fs/exofs/core/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// core/ — Types fondamentaux ExoFS — ZÉRO dépendance externe
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce module contient UNIQUEMENT des types purs sans dépendance sur le reste du
// kernel (sauf core:: et alloc::). Il est importé par tous les autres modules
// ExoFS.

pub mod types;
pub mod constants;
pub mod error;
pub mod object_kind;
pub mod object_class;
pub mod object_id;
pub mod blob_id;
pub mod epoch_id;
pub mod rights;
pub mod flags;
pub mod version;
pub mod stats;
pub mod config;
// DAG-01 : abstraction horodatage interne (remplace arch::time)
pub mod clock;

// ── Re-exports fondamentaux ───────────────────────────────────────────────────

pub use types::{
    ObjectId, BlobId, EpochId, SnapshotId, DiskOffset, Extent, PhysAddr,
    TimeSpec, ByteRange, InlineData,
};
pub use constants::{
    EXOFS_MAGIC, EPOCH_ROOT_MAGIC, OBJECT_HEADER_MAGIC, EXOAR_MAGIC,
    EPOCH_SLOT_A_OFFSET, EPOCH_SLOT_B_OFFSET, EPOCH_SLOT_C_FROM_END,
    SB_PRIMARY_OFFSET, SB_MIRROR_12K_OFFSET, SB_MIRROR_END_FROM_END,
    HEAP_START_OFFSET, EPOCH_SLOT_SIZE, SUPERBLOCK_SIZE, BLOCK_SIZE,
    EPOCH_MAX_OBJECTS, SYMLINK_MAX_DEPTH, NAME_MAX, PATH_MAX,
    PATH_INDEX_SPLIT_THRESHOLD, PATH_INDEX_MERGE_THRESHOLD,
    INLINE_DATA_MAX_BYTES, INLINE_DATA_BUFFER_SIZE, GC_MIN_EPOCH_DELAY_DEFAULT, GC_MAX_GREY_QUEUE,
    GC_FREE_THRESHOLD_PCT, GC_TIMER_INTERVAL_SECS,
    WRITEBACK_INTERVAL_MS, PATH_CACHE_CAPACITY, COMPRESS_MIN_SIZE,
    FORMAT_VERSION_MAJOR, FORMAT_VERSION_MINOR,
    // Constantes additionnelles
    PAGE_SIZE, CACHE_LINE_SIZE, MAX_OBJECTS_PER_VOLUME, MAX_SNAPSHOTS,
    MAX_EXTENTS_PER_OBJECT, ENCRYPTION_KEY_SIZE, ENCRYPTION_NONCE_SIZE,
    ENCRYPTION_TAG_SIZE, CLASS2_MARKER, CLASS2_COUNTER_OFFSET,
    BLOB_HEADER_MAGIC, SNAPSHOT_RECORD_MAGIC, RELATION_RECORD_MAGIC,
    EPOCH_COMMIT_TIMEOUT_MS, CRC32C_POLY,
};
pub use error::{ExofsError, ExofsResult, ErrorCategory, ErrorSeverity};
pub use object_kind::{ObjectKind, KindOperation};
pub use object_class::{ObjectClass, ClassOperation, CowPolicy};
pub use object_id::{new_class1, new_class2};
pub use blob_id::{
    compute_blob_id, verify_blob_id, blake3_hash,
    BlobIdHasher, merkle_root, hash_concat,
};
// Module CRC32C intégré dans blob_id.rs
pub use blob_id::crc32c;

pub use epoch_id::{
    EpochState, EpochStats, EpochRange, EpochCommitSummary,
    max_epoch, min_epoch, epoch_distance_sane, epoch_in_window,
    epoch_distance, epoch_gc_eligible, epoch_prev, epoch_clamp,
    next_epoch_id, current_epoch_id, restore_epoch_counter,
};
pub use rights::{
    RIGHT_INSPECT_CONTENT, RIGHT_SNAPSHOT_CREATE,
    RIGHT_RELATION_CREATE, RIGHT_GC_TRIGGER,
    RIGHT_READ, RIGHT_WRITE, RIGHT_EXEC, RIGHT_DELETE,
    RIGHT_ADMIN,
    RightsMask,
    has_inspect_content, has_snapshot_create, has_write,
    would_expose_secret,
};
pub use flags::{
    ObjectFlags, ObjectFlagsBuilder,
    ExtentFlags, EpochFlags, SnapshotFlags, MigrationFlags, MountFlags,
};
pub use version::{
    FormatVersion, FeatureFlags, MountCompatibility, negotiate_mount,
    FeatureDependency, FeatureFlagsBuilder, VersionNegotiationResult,
    negotiate_mount_detailed, check_feature_dependencies,
    feature_overhead_pct10, total_features_overhead_pct10,
    version_history_entry, find_migration_path, validate_version_on_disk,
    VersionHistoryEntry, MigrationDescriptor,
};
pub use stats::{ExofsStats, ExofsStatsSnapshot, ExofsStatsExtended, EXOFS_STATS};
pub use config::{ExofsConfig, ConfigProfile, MountOptions, EXOFS_CONFIG};
