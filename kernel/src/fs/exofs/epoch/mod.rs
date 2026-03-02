// kernel/src/fs/exofs/epoch/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Module epoch — Gestion du journal d'epochs ExoFS
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// L'epoch manager est le cœur de la cohérence ExoFS.
// Il garantit que chaque commit est atomique et durably persisté via
// le protocole à 3 barrières NVMe.

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

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports publics
// ─────────────────────────────────────────────────────────────────────────────

pub use epoch_record::EpochRecord;
pub use epoch_commit_lock::{EPOCH_COMMIT_LOCK, EpochCommitState};
pub use epoch_barriers::{
    nvme_barrier_after_data, nvme_barrier_after_root, nvme_barrier_after_record,
};
pub use epoch_root::{
    EpochRootInMemory, EpochRootEntry, EpochRootPageHeader,
    verify_epoch_root_page,
};
pub use epoch_root_chain::{
    serialize_epoch_root_chain, deserialize_epoch_root_chain,
    EPOCH_ROOT_PAGE_SIZE,
};
pub use epoch_commit::{commit_epoch, CommitInput, CommitResult};
pub use epoch_slots::{EpochSlot, EpochSlotSelector, parse_slot_data};
pub use epoch_recovery::{recover_active_epoch, RecoveryResult, ReadFn, ReadPageFn};
pub use epoch_gc::{compute_gc_window, epoch_is_collectable, gc_epoch_lag, GcEpochWindow};
pub use epoch_pin::{EpochPin, oldest_pinned_epoch, is_epoch_pinned};
pub use epoch_snapshot::{
    SnapshotDescriptor, SnapshotName, create_snapshot, delete_snapshot,
};
pub use epoch_delta::{EpochDelta, DeltaEntry, DeltaOpKind};
pub use epoch_checksum::{
    compute_epoch_record_checksum, verify_epoch_record_checksum,
    compute_epoch_root_page_checksum, seal_epoch_root_page,
    verify_page_integrity,
};
pub use epoch_writeback::{
    WritebackController, FlushReason, WRITEBACK_CTL,
    should_flush_now, record_flush,
};
pub use epoch_stats::{EpochStats, EpochStatsSnapshot, EPOCH_STATS};
