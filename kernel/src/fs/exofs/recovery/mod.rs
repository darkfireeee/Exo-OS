//! mod.rs — Module `recovery` du système de fichiers ExoFS (façade de câblage).

pub mod block_io;
pub mod boot_recovery;
pub mod checkpoint;
pub mod epoch_replay;
pub mod fsck;
pub mod fsck_phase1;
pub mod fsck_phase2;
pub mod fsck_phase3;
pub mod fsck_phase4;
pub mod fsck_repair;
mod lifecycle;
pub mod ondisk;
pub mod recovery_audit;
pub mod recovery_log;
pub mod slot_recovery;

pub use boot_recovery::{
    boot_recovery_sequence, BlockDevice, BootRecovery, BootRecoveryOptions, BootRecoveryResult,
};
pub use checkpoint::{Checkpoint, CheckpointId, CheckpointStore, RecoveryPhase, CHECKPOINT_STORE};
pub use epoch_replay::{EpochReplay, EpochReplayOptions, ReplayResult};
pub use fsck::{Fsck, FsckOptions, FsckResult};
pub use fsck_phase1::{
    FsckPhase1, Phase1Error, Phase1ErrorKind, Phase1Options, Phase1Report, SuperblockDisk,
    SUPERBLOCK_HDR_MAGIC, SUPERBLOCK_LBA,
};
pub use fsck_phase2::{
    AllocEntry, BlobRefCounter, FsckPhase2, Phase2Error, Phase2ErrorKind, Phase2Options,
    Phase2Report,
};
pub use fsck_phase3::{
    FsckPhase3, Phase3Error, Phase3ErrorKind, Phase3Options, Phase3Report, SnapshotHeaderDisk,
    SNAPSHOT_HDR_MAGIC,
};
pub use fsck_phase4::{
    FsckPhase4, LostFoundEntry, Phase4Error, Phase4ErrorKind, Phase4Options, Phase4Report,
};
pub use fsck_repair::{FsckRepair, RepairAction, RepairLog, RepairRecord, REPAIR_LOG};
pub use lifecycle::{init, is_initialized, shutdown, verify_health, RecoveryHealth};
pub use recovery_audit::{
    AuditEntry, AuditEventKind, AuditSeverity, RecoveryAudit, RECOVERY_AUDIT,
};
pub use recovery_log::{
    RecoveryEvent, RecoveryLog, RecoveryLogCategory, RecoveryLogEntry, RECOVERY_LOG,
};
pub use slot_recovery::{SlotId, SlotRecovery, SlotRecoveryResult, SLOT_A, SLOT_B, SLOT_C};
