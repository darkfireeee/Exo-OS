//! space_tracker.rs — Suivi de l'utilisation de l'espace disque ExoFS (no_std).

use core::sync::atomic::{AtomicU64, Ordering};

pub static SPACE_TRACKER: SpaceTracker = SpaceTracker::new_const();

pub struct SpaceTracker {
    total_blocks:    AtomicU64,
    used_blocks:     AtomicU64,
    reserved_blocks: AtomicU64,   // Blocs réservés pour metadata/GC.
    block_size:      AtomicU64,
}

impl SpaceTracker {
    pub const fn new_const() -> Self {
        Self {
            total_blocks:    AtomicU64::new(0),
            used_blocks:     AtomicU64::new(0),
            reserved_blocks: AtomicU64::new(0),
            block_size:      AtomicU64::new(4096),
        }
    }

    pub fn init(&self, total: u64, reserved: u64, block_size: u64) {
        self.total_blocks.store(total, Ordering::Relaxed);
        self.reserved_blocks.store(reserved, Ordering::Relaxed);
        self.block_size.store(block_size, Ordering::Relaxed);
    }

    pub fn alloc_blocks(&self, n: u64) {
        self.used_blocks.fetch_add(n, Ordering::Relaxed);
    }

    pub fn free_blocks(&self, n: u64) {
        let current = self.used_blocks.load(Ordering::Relaxed);
        self.used_blocks.store(current.saturating_sub(n), Ordering::Relaxed);
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_blocks.load(Ordering::Relaxed)
            .saturating_mul(self.block_size.load(Ordering::Relaxed))
    }

    pub fn used_bytes(&self) -> u64 {
        self.used_blocks.load(Ordering::Relaxed)
            .saturating_mul(self.block_size.load(Ordering::Relaxed))
    }

    pub fn free_bytes(&self) -> u64 {
        self.total_bytes().saturating_sub(self.used_bytes())
    }

    /// Utilisation en pourcents (0-100).
    pub fn usage_pct(&self) -> u8 {
        let total = self.total_blocks.load(Ordering::Relaxed);
        if total == 0 { return 0; }
        let used = self.used_blocks.load(Ordering::Relaxed);
        ((used * 100) / total).min(100) as u8
    }
}
