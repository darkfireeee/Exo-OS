//! Module epoch — Gestion du journal d'epochs ExoFS (façade de câblage, EPOCH-01).

pub mod epoch_barriers;
pub mod epoch_checksum;
pub mod epoch_commit;
pub mod epoch_commit_lock;
pub mod epoch_delta;
pub mod epoch_gc;
pub mod epoch_id;
pub mod epoch_pin;
pub mod epoch_record;
pub mod epoch_recovery;
pub mod epoch_root;
pub mod epoch_root_chain;
pub mod epoch_slots;
pub mod epoch_snapshot;
pub mod epoch_stats;
pub mod epoch_writeback;

pub use epoch_barriers::{
    execute_three_phase_barriers, nvme_barrier_after_data, nvme_barrier_after_record,
    nvme_barrier_after_root, register_nvme_flush_fn, BarrierHealth, BarrierStats,
    ThreePhaseBarrierResult,
};
pub use epoch_checksum::{
    compute_epoch_record_checksum, ct_eq_32, ct_eq_slice, seal_epoch_root_page,
    verify_epoch_record_checksum, verify_epoch_root_page_checksum, IncrementalChecksum,
    EPOCH_RECORD_BODY_LEN,
};
pub use epoch_commit::{
    commit_epoch, forced_commit_flags, should_force_commit, AbortReason, CommitCallbacks,
    CommitInput, CommitPhase, CommitResult,
};
pub use epoch_commit_lock::{
    release_commit_lock, try_acquire_commit_lock, CommitLockSnapshot, CommitStatus,
    EpochCommitState, EPOCH_COMMIT_LOCK,
};
pub use epoch_delta::{
    DeltaEntry, DeltaOpCounts, DeltaOpKind, DeltaSortOrder, DeltaStats, EpochDelta,
};
pub use epoch_gc::{
    compute_gc_window, epoch_is_collectable, gc_epoch_lag, run_gc_cycle, DeferReason,
    DeferredDeleteEntry, DeferredDeleteQueue, GcBlockReason, GcCheckResult, GcEpochWindow,
    GcSafetyCheck, GcStats, GcStatsSnapshot, GC_STATS,
};
pub use epoch_id::{
    allocate_next_epoch_id, commit_sequence, current_epoch_id, durable_epoch_id, epoch_after,
    epoch_before, epoch_cmp, epoch_distance, epoch_is_old_enough, epoch_max, epoch_min,
    epoch_within_grace, epochs_in_flight, init_epoch_counter, is_future_epoch, mark_epoch_durable,
    reset_epoch_counter, set_epoch_id_from_recovery, validate_epoch_id_from_disk,
    validate_epoch_sequence, EpochCounterSnapshot, EpochIdExt, EpochRange, DEFAULT_GC_GRACE_WINDOW,
    EPOCH_FIRST, EPOCH_INVALID, EPOCH_MIN_COLLECT_AGE, EPOCH_WRITEBACK_MAX_PENDING,
};
pub use epoch_pin::{
    active_pin_count, is_epoch_pinned, list_active_pins, oldest_pinned_epoch, pin_table_stats,
    validate_pin_table, EpochPin, PinReason, PinSnapshot, PinTableStats, MAX_EPOCH_PINS,
};
pub use epoch_record::EpochRecord;
pub use epoch_recovery::{
    is_degraded_recovery, max_valid_epoch, recover_active_epoch, recovery_summary,
    snapshot_recovery_stats, validate_recovery_result, ReadFn, ReadPageFn, RecoveryDiagnostics,
    RecoveryParams, RecoveryPhase, RecoveryResult, RecoveryStats, RecoverySummary, SetEpochFn,
    SlotCheckResult, TscFn,
};
pub use epoch_root::{
    read_page_entries, read_page_header, verify_epoch_root_page, EpochRootBuilder, EpochRootEntry,
    EpochRootInMemory, EpochRootPageHeader, EpochRootStats,
};
pub use epoch_root_chain::{
    count_pages_needed, deserialize_epoch_root_chain, rebuild_chain_offsets,
    serialize_epoch_root_chain, validate_chain_integrity, ChainStats, PageStats, ENTRIES_PER_PAGE,
    EPOCH_CHAIN_NEXT_PLACEHOLDER, EPOCH_ROOT_PAGE_SIZE,
};
pub use epoch_slots::{
    parse_slot_data, EpochSlot, EpochSlotSelector, RecoverySlotReason, SlotReadResult, SlotStatus,
};
pub use epoch_snapshot::{
    create_snapshot, create_snapshot_with_expiry, delete_snapshot, epoch_has_snapshot,
    expire_snapshots, has_snapshot_create, has_snapshot_delete, has_snapshot_list,
    snapshot_epoch_id, snapshot_registry_stats, SnapshotDescriptor, SnapshotName, SnapshotRegistry,
    SnapshotRegistryStats, MAX_SNAPSHOTS, RIGHT_SNAPSHOT_CREATE, RIGHT_SNAPSHOT_DELETE,
    RIGHT_SNAPSHOT_LIST, RIGHT_SNAPSHOT_READ, SNAPSHOT_REGISTRY,
};
pub use epoch_stats::{
    EpochRecoveryStats, EpochStats, EpochStatsSnapshot, LatencyHistogram, LatencyHistogramSnapshot,
    EPOCH_STATS,
};
pub use epoch_writeback::{
    current_backpressure, record_flush, request_force_commit, should_flush_now,
    should_flush_now_simple, writeback_stats, BackpressurePolicy, FlushReason, FlushSchedule,
    GroupCommitBuffer, GroupCommitEntry, GroupCommitStats, GroupCommitSummary, WritebackController,
    WritebackCycleResult, WritebackDecision, WritebackStats, FLUSH_SCHEDULE, GROUP_COMMIT_CAPACITY,
    WRITEBACK_CTL,
};
