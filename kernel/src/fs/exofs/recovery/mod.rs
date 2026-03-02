//! recovery/ — Module de récupération et fsck ExoFS (no_std).

pub mod boot_recovery;
pub mod slot_recovery;
pub mod epoch_replay;
pub mod fsck;
pub mod fsck_phase1;
pub mod fsck_phase2;
pub mod fsck_phase3;
pub mod fsck_phase4;
pub mod fsck_repair;
pub mod checkpoint;
pub mod recovery_log;
pub mod recovery_audit;

pub use boot_recovery::{BootRecovery, BootRecoveryResult};
pub use slot_recovery::{SlotRecovery, SlotId, SlotRecoveryResult};
pub use epoch_replay::{EpochReplay, ReplayResult};
pub use fsck::{Fsck, FsckResult, FsckOptions};
pub use checkpoint::{CHECKPOINT_STORE, Checkpoint, CheckpointId};
pub use recovery_log::RECOVERY_LOG;
pub use recovery_audit::RECOVERY_AUDIT;
