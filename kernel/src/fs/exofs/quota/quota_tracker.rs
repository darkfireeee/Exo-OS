//! QuotaTracker — suivi de l'utilisation des quotas ExoFS (no_std).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::FsError;

pub static QUOTA_TRACKER: QuotaTracker = QuotaTracker::new_const();

/// Consommation courante d'une entité.
#[derive(Clone, Copy, Debug, Default)]
pub struct QuotaUsage {
    pub bytes_used:  u64,
    pub blobs_used:  u64,
    pub inodes_used: u64,
}

/// Clé de quota : (kind, entity_id).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct QuotaKey {
    pub kind:      u8,   // 0=User, 1=Group, 2=Project.
    pub entity_id: u64,
}

pub struct QuotaTracker {
    usage:        SpinLock<BTreeMap<QuotaKey, QuotaUsage>>,
    total_bytes:  AtomicU64,
    total_blobs:  AtomicU64,
}

impl QuotaTracker {
    pub const fn new_const() -> Self {
        Self {
            usage:       SpinLock::new(BTreeMap::new()),
            total_bytes: AtomicU64::new(0),
            total_blobs: AtomicU64::new(0),
        }
    }

    pub fn charge(
        &self,
        key: QuotaKey,
        delta_bytes: u64,
        delta_blobs: u64,
        delta_inodes: u64,
    ) -> Result<(), FsError> {
        let mut usage = self.usage.lock();
        let entry = if let Some(e) = usage.get_mut(&key) {
            e
        } else {
            usage.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
            usage.insert(key, QuotaUsage::default());
            usage.get_mut(&key).unwrap()
        };
        entry.bytes_used  = entry.bytes_used.saturating_add(delta_bytes);
        entry.blobs_used  = entry.blobs_used.saturating_add(delta_blobs);
        entry.inodes_used = entry.inodes_used.saturating_add(delta_inodes);
        self.total_bytes.fetch_add(delta_bytes, Ordering::Relaxed);
        self.total_blobs.fetch_add(delta_blobs, Ordering::Relaxed);
        Ok(())
    }

    pub fn uncharge(
        &self,
        key: QuotaKey,
        delta_bytes: u64,
        delta_blobs: u64,
        delta_inodes: u64,
    ) {
        let mut usage = self.usage.lock();
        if let Some(entry) = usage.get_mut(&key) {
            entry.bytes_used  = entry.bytes_used.saturating_sub(delta_bytes);
            entry.blobs_used  = entry.blobs_used.saturating_sub(delta_blobs);
            entry.inodes_used = entry.inodes_used.saturating_sub(delta_inodes);
        }
        self.total_bytes.fetch_sub(delta_bytes.min(self.total_bytes.load(Ordering::Relaxed)), Ordering::Relaxed);
        self.total_blobs.fetch_sub(delta_blobs.min(self.total_blobs.load(Ordering::Relaxed)), Ordering::Relaxed);
    }

    pub fn get_usage(&self, key: &QuotaKey) -> QuotaUsage {
        self.usage.lock().get(key).copied().unwrap_or_default()
    }

    pub fn reset(&self, key: &QuotaKey) {
        self.usage.lock().remove(key);
    }

    pub fn total_bytes(&self) -> u64 { self.total_bytes.load(Ordering::Relaxed) }
    pub fn total_blobs(&self) -> u64 { self.total_blobs.load(Ordering::Relaxed) }
}
