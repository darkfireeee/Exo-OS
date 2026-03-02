//! CacheShrinker — réducteur de cache sous pression mémoire (no_std).

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::FsError;
use super::cache_pressure::{CACHE_PRESSURE, PressureLevel};
use super::blob_cache::BLOB_CACHE;

pub static CACHE_SHRINKER: CacheShrinker = CacheShrinker::new_const();

pub struct CacheShrinker {
    total_shrinks:  AtomicU64,
    bytes_reclaimed: AtomicU64,
}

impl CacheShrinker {
    pub const fn new_const() -> Self {
        Self {
            total_shrinks:   AtomicU64::new(0),
            bytes_reclaimed: AtomicU64::new(0),
        }
    }

    /// Démarre un cycle de shrinking selon la pression mémoire courante.
    pub fn shrink(&self) -> Result<ShrinkResult, FsError> {
        let pressure = CACHE_PRESSURE.level();
        let target_reclaim = match pressure {
            PressureLevel::Normal   => return Ok(ShrinkResult::default()),
            PressureLevel::Low      => BLOB_CACHE.used_bytes() / 10,
            PressureLevel::Medium   => BLOB_CACHE.used_bytes() / 5,
            PressureLevel::High     => BLOB_CACHE.used_bytes() / 3,
            PressureLevel::Critical => BLOB_CACHE.used_bytes() / 2,
        };

        let reclaimed = self.reclaim_blob_cache(target_reclaim);
        self.total_shrinks.fetch_add(1, Ordering::Relaxed);
        self.bytes_reclaimed.fetch_add(reclaimed, Ordering::Relaxed);

        Ok(ShrinkResult {
            pressure,
            bytes_reclaimed: reclaimed,
            target:          target_reclaim,
        })
    }

    fn reclaim_blob_cache(&self, _target: u64) -> u64 {
        // Déclenche l'éviction dans le BlobCache via sa méthode interne.
        // Pour l'instant on retourne 0 (le BlobCache évince automatiquement à l'insertion).
        0
    }

    pub fn total_shrinks(&self) -> u64 { self.total_shrinks.load(Ordering::Relaxed) }
    pub fn bytes_reclaimed(&self) -> u64 { self.bytes_reclaimed.load(Ordering::Relaxed) }
}

#[derive(Default, Debug)]
pub struct ShrinkResult {
    pub pressure:        PressureLevel,
    pub bytes_reclaimed: u64,
    pub target:          u64,
}

impl Default for PressureLevel {
    fn default() -> Self { PressureLevel::Normal }
}
