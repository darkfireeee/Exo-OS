//! Checkpoint — points de reprise recovery ExoFS (no_std).

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::arch::time::read_ticks;
use crate::fs::exofs::core::{EpochId, FsError};

pub static CHECKPOINT_STORE: CheckpointStore = CheckpointStore::new_const();

/// Identifiant de checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CheckpointId(pub u64);

/// Phase de recovery atteinte.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryPhase {
    None       = 0,
    SlotRead   = 1,
    EpochFound = 2,
    Replayed   = 3,
    Phase1Done = 4,
    Phase2Done = 5,
    Phase3Done = 6,
    Phase4Done = 7,
    Complete   = 8,
}

/// Entrée de checkpoint.
#[derive(Clone, Copy, Debug)]
pub struct Checkpoint {
    pub id:        CheckpointId,
    pub phase:     RecoveryPhase,
    pub epoch_id:  EpochId,
    pub tick:      u64,
    pub errors:    u32,
}

pub struct CheckpointStore {
    checkpoints: SpinLock<BTreeMap<u64, Checkpoint>>,
    next_id:     AtomicU64,
}

impl CheckpointStore {
    pub const fn new_const() -> Self {
        Self {
            checkpoints: SpinLock::new(BTreeMap::new()),
            next_id:     AtomicU64::new(1),
        }
    }

    pub fn record(
        &self,
        phase:    RecoveryPhase,
        epoch_id: EpochId,
        errors:   u32,
    ) -> Result<CheckpointId, FsError> {
        let id = CheckpointId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let cp = Checkpoint { id, phase, epoch_id, tick: read_ticks(), errors };
        let mut store = self.checkpoints.lock();
        store.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        store.insert(id.0, cp);
        Ok(id)
    }

    pub fn latest(&self) -> Option<Checkpoint> {
        let store = self.checkpoints.lock();
        store.values().max_by_key(|c| c.tick).copied()
    }

    pub fn get(&self, id: CheckpointId) -> Option<Checkpoint> {
        self.checkpoints.lock().get(&id.0).copied()
    }

    pub fn clear(&self) {
        self.checkpoints.lock().clear();
    }
}
