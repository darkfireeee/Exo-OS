//! Statistiques de compression ExoFS.

use core::sync::atomic::{AtomicU64, Ordering};

/// Compteurs de compression par algorithme.
pub struct AlgoStats {
    pub compressed_count: AtomicU64,
    pub compressed_bytes_in: AtomicU64,
    pub compressed_bytes_out: AtomicU64,
    pub decompress_count: AtomicU64,
    pub compress_errors: AtomicU64,
}

impl AlgoStats {
    pub const fn new() -> Self {
        Self {
            compressed_count: AtomicU64::new(0),
            compressed_bytes_in: AtomicU64::new(0),
            compressed_bytes_out: AtomicU64::new(0),
            decompress_count: AtomicU64::new(0),
            compress_errors: AtomicU64::new(0),
        }
    }

    pub fn record_compress(&self, bytes_in: u64, bytes_out: u64, ok: bool) {
        self.compressed_count.fetch_add(1, Ordering::Relaxed);
        self.compressed_bytes_in.fetch_add(bytes_in, Ordering::Relaxed);
        if ok {
            self.compressed_bytes_out.fetch_add(bytes_out, Ordering::Relaxed);
        } else {
            self.compress_errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Ratio de compression moyen (0–100, 100 = aucun bénéfice).
    pub fn avg_ratio_percent(&self) -> u64 {
        let out = self.compressed_bytes_out.load(Ordering::Relaxed);
        let inn = self.compressed_bytes_in.load(Ordering::Relaxed);
        if inn == 0 { return 100; }
        (out * 100) / inn
    }
}

/// Statistiques globales de compression.
pub struct CompressionStats {
    pub lz4: AlgoStats,
    pub zstd: AlgoStats,
    /// Blobs ignorés (ratio ≥ seuil → stockés sans compression).
    pub skipped_incompressible: AtomicU64,
}

impl CompressionStats {
    pub const fn new() -> Self {
        Self {
            lz4: AlgoStats::new(),
            zstd: AlgoStats::new(),
            skipped_incompressible: AtomicU64::new(0),
        }
    }

    pub fn record_skip(&self) {
        self.skipped_incompressible.fetch_add(1, Ordering::Relaxed);
    }
}

/// Statistiques globales de compression.
pub static COMPRESSION_STATS: CompressionStats = CompressionStats::new();
