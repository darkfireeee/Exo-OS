// kernel/src/fs/exofs/epoch/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Module epoch — Gestion du journal d'epochs ExoFS
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// L'epoch manager est le cœur de la cohérence ExoFS.
// Il garantit que chaque commit est atomique et durably persisté via
// le protocole à 3 barrières NVMe (EPOCH-01).
//
// Dépendances autorisées (DAG-01) :
//   epoch/ → core/, memory/, scheduler/, security/
//   epoch/ ↛ storage/, ipc/, process/, arch/

pub mod epoch_id;
pub mod epoch_record;
pub mod epoch_barriers;
pub mod epoch_commit_lock;
pub mod epoch_root;
pub mod epoch_root_chain;
pub mod epoch_commit;
pub mod epoch_slots;
pub mod epoch_recovery;
pub mod epoch_gc;
pub mod epoch_pin;
pub mod epoch_snapshot;
pub mod epoch_delta;
pub mod epoch_checksum;
pub mod epoch_writeback;
pub mod epoch_stats;

// =============================================================================
// Re-exports — epoch_id
// =============================================================================

pub use epoch_id::{
    EPOCH_INVALID,
    EPOCH_FIRST,
    DEFAULT_GC_GRACE_WINDOW,
    EPOCH_WRITEBACK_MAX_PENDING,
    EPOCH_MIN_COLLECT_AGE,
    EpochIdExt,
    EpochRange,
    EpochCounterSnapshot,
    init_epoch_counter,
    reset_epoch_counter,
    current_epoch_id,
    durable_epoch_id,
    commit_sequence,
    epochs_in_flight,
    allocate_next_epoch_id,
    mark_epoch_durable,
    set_epoch_id_from_recovery,
    is_future_epoch,
    epoch_within_grace,
    epoch_is_old_enough,
    epoch_distance,
    epoch_max,
    epoch_min,
    epoch_cmp,
    epoch_before,
    epoch_after,
    validate_epoch_id_from_disk,
    validate_epoch_sequence,
};

// =============================================================================
// Re-exports — epoch_record
// =============================================================================

pub use epoch_record::EpochRecord;

// =============================================================================
// Re-exports — epoch_barriers
// =============================================================================

pub use epoch_barriers::{
    nvme_barrier_after_data,
    nvme_barrier_after_root,
    nvme_barrier_after_record,
    execute_three_phase_barriers,
    ThreePhaseBarrierResult,
    BarrierStats,
    BarrierHealth,
    register_nvme_flush_fn,
};

// =============================================================================
// Re-exports — epoch_commit_lock
// =============================================================================

pub use epoch_commit_lock::{
    EPOCH_COMMIT_LOCK,
    EpochCommitState,
    CommitStatus,
    CommitLockSnapshot,
    try_acquire_commit_lock,
    release_commit_lock,
};

// =============================================================================
// Re-exports — epoch_root
// =============================================================================

pub use epoch_root::{
    EpochRootInMemory,
    EpochRootEntry,
    EpochRootPageHeader,
    EpochRootStats,
    EpochRootBuilder,
    verify_epoch_root_page,
    read_page_header,
    read_page_entries,
};

// =============================================================================
// Re-exports — epoch_root_chain
// =============================================================================

pub use epoch_root_chain::{
    serialize_epoch_root_chain,
    deserialize_epoch_root_chain,
    validate_chain_integrity,
    rebuild_chain_offsets,
    count_pages_needed,
    EPOCH_ROOT_PAGE_SIZE,
    ENTRIES_PER_PAGE,
    EPOCH_CHAIN_NEXT_PLACEHOLDER,
    ChainStats,
    PageStats,
};

// =============================================================================
// Re-exports — epoch_commit
// =============================================================================

pub use epoch_commit::{
    commit_epoch,
    CommitInput,
    CommitResult,
    CommitCallbacks,
    CommitPhase,
    AbortReason,
    should_force_commit,
    forced_commit_flags,
};

// =============================================================================
// Re-exports — epoch_slots
// =============================================================================

pub use epoch_slots::{
    EpochSlot,
    EpochSlotSelector,
    SlotStatus,
    SlotReadResult,
    RecoverySlotReason,
    parse_slot_data,
};

// =============================================================================
// Re-exports — epoch_recovery
// =============================================================================

pub use epoch_recovery::{
    recover_active_epoch,
    RecoveryResult,
    RecoveryParams,
    RecoveryDiagnostics,
    RecoveryPhase,
    RecoveryStats,
    RecoverySummary,
    SlotCheckResult,
    ReadFn,
    ReadPageFn,
    SetEpochFn,
    TscFn,
    snapshot_recovery_stats,
    validate_recovery_result,
    recovery_summary,
    is_degraded_recovery,
    max_valid_epoch,
};

// =============================================================================
// Re-exports — epoch_gc
// =============================================================================

pub use epoch_gc::{
    GcEpochWindow,
    compute_gc_window,
    epoch_is_collectable,
    gc_epoch_lag,
    DeferredDeleteQueue,
    DeferredDeleteEntry,
    DeferReason,
    GcSafetyCheck,
    GcCheckResult,
    GcBlockReason,
    GcStats,
    GcStatsSnapshot,
    GC_STATS,
    run_gc_cycle,
};

// =============================================================================
// Re-exports — epoch_pin
// =============================================================================

pub use epoch_pin::{
    EpochPin,
    PinReason,
    PinTableStats,
    PinSnapshot,
    MAX_EPOCH_PINS,
    oldest_pinned_epoch,
    is_epoch_pinned,
    active_pin_count,
    pin_table_stats,
    list_active_pins,
    validate_pin_table,
};

// =============================================================================
// Re-exports — epoch_snapshot
// =============================================================================

pub use epoch_snapshot::{
    SnapshotDescriptor,
    SnapshotName,
    SnapshotRegistry,
    SnapshotRegistryStats,
    SNAPSHOT_REGISTRY,
    MAX_SNAPSHOTS,
    RIGHT_SNAPSHOT_CREATE,
    RIGHT_SNAPSHOT_DELETE,
    RIGHT_SNAPSHOT_LIST,
    RIGHT_SNAPSHOT_READ,
    has_snapshot_create,
    has_snapshot_delete,
    has_snapshot_list,
    create_snapshot,
    create_snapshot_with_expiry,
    delete_snapshot,
    snapshot_epoch_id,
    epoch_has_snapshot,
    snapshot_registry_stats,
    expire_snapshots,
};

// =============================================================================
// Re-exports — epoch_delta
// =============================================================================

pub use epoch_delta::{
    EpochDelta,
    DeltaEntry,
    DeltaOpKind,
    DeltaSortOrder,
    DeltaStats,
    DeltaOpCounts,
};

// =============================================================================
// Re-exports — epoch_checksum
// =============================================================================

pub use epoch_checksum::{
    compute_epoch_record_checksum,
    verify_epoch_record_checksum,
    seal_epoch_root_page,
    verify_epoch_root_page_checksum,
    IncrementalChecksum,
    ct_eq_32,
    ct_eq_slice,
    EPOCH_RECORD_BODY_LEN,
};

// =============================================================================
// Re-exports — epoch_writeback
// =============================================================================

pub use epoch_writeback::{
    WritebackController,
    FlushReason,
    WritebackDecision,
    BackpressurePolicy,
    WritebackStats,
    WritebackCycleResult,
    FlushSchedule,
    GroupCommitBuffer,
    GroupCommitEntry,
    GroupCommitStats,
    GroupCommitSummary,
    WRITEBACK_CTL,
    FLUSH_SCHEDULE,
    GROUP_COMMIT_CAPACITY,
    should_flush_now,
    should_flush_now_simple,
    record_flush,
    request_force_commit,
    writeback_stats,
    current_backpressure,
};

// =============================================================================
// Re-exports — epoch_stats
// =============================================================================

pub use epoch_stats::{
    EpochStats,
    EpochStatsSnapshot,
    EpochRecoveryStats,
    LatencyHistogram,
    LatencyHistogramSnapshot,
    EPOCH_STATS,
};
