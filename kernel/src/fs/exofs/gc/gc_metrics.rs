//! Métriques du Garbage Collector ExoFS.
//!
//! Accumule les statistiques de chaque passe pour l'observabilité.

use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::gc::blob_gc::GcPassResult;

/// Compteurs cumulatifs du GC.
pub struct GcMetrics {
    pub total_passes: AtomicU64,
    pub total_blobs_swept: AtomicU64,
    pub total_bytes_freed: AtomicU64,
    pub total_orphans_collected: AtomicU64,
    pub total_duration_ticks: AtomicU64,
    pub max_pass_duration_ticks: AtomicU64,
    pub last_pass_epoch: AtomicU64,
}

impl GcMetrics {
    pub fn new() -> Self {
        Self {
            total_passes: AtomicU64::new(0),
            total_blobs_swept: AtomicU64::new(0),
            total_bytes_freed: AtomicU64::new(0),
            total_orphans_collected: AtomicU64::new(0),
            total_duration_ticks: AtomicU64::new(0),
            max_pass_duration_ticks: AtomicU64::new(0),
            last_pass_epoch: AtomicU64::new(0),
        }
    }

    /// Enregistre les statistiques d'une passe terminée.
    pub fn record_pass(&self, r: &GcPassResult) {
        self.total_passes.fetch_add(1, Ordering::Relaxed);
        self.total_blobs_swept.fetch_add(r.blobs_swept, Ordering::Relaxed);
        self.total_bytes_freed.fetch_add(r.bytes_freed, Ordering::Relaxed);
        self.total_orphans_collected.fetch_add(r.orphans_collected, Ordering::Relaxed);
        self.total_duration_ticks.fetch_add(r.duration_ticks, Ordering::Relaxed);
        self.last_pass_epoch.store(r.epoch, Ordering::Release);

        // Met à jour le max de durée.
        let mut current = self.max_pass_duration_ticks.load(Ordering::Acquire);
        while r.duration_ticks > current {
            match self.max_pass_duration_ticks.compare_exchange_weak(
                current,
                r.duration_ticks,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(v) => current = v,
            }
        }
    }

    /// Snapshot instantané des compteurs.
    pub fn snapshot(&self) -> GcMetricsSnapshot {
        GcMetricsSnapshot {
            total_passes: self.total_passes.load(Ordering::Acquire),
            total_blobs_swept: self.total_blobs_swept.load(Ordering::Acquire),
            total_bytes_freed: self.total_bytes_freed.load(Ordering::Acquire),
            total_orphans_collected: self.total_orphans_collected.load(Ordering::Acquire),
            total_duration_ticks: self.total_duration_ticks.load(Ordering::Acquire),
            max_pass_duration_ticks: self.max_pass_duration_ticks.load(Ordering::Acquire),
            last_pass_epoch: self.last_pass_epoch.load(Ordering::Acquire),
        }
    }
}

/// Snapshot immuable des métriques GC (pour export).
#[derive(Debug, Clone)]
pub struct GcMetricsSnapshot {
    pub total_passes: u64,
    pub total_blobs_swept: u64,
    pub total_bytes_freed: u64,
    pub total_orphans_collected: u64,
    pub total_duration_ticks: u64,
    pub max_pass_duration_ticks: u64,
    pub last_pass_epoch: u64,
}

/// Métriques globales du GC.
pub static GC_METRICS: GcMetrics = GcMetrics {
    total_passes: AtomicU64::new(0),
    total_blobs_swept: AtomicU64::new(0),
    total_bytes_freed: AtomicU64::new(0),
    total_orphans_collected: AtomicU64::new(0),
    total_duration_ticks: AtomicU64::new(0),
    max_pass_duration_ticks: AtomicU64::new(0),
    last_pass_epoch: AtomicU64::new(0),
};
