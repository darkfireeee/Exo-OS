//! Statistiques de compression ExoFS.
//!
//! Compteurs atomiques par algorithme, globalaux, et snapshots point-in-time.
//! Les compteurs AtomicU64 sont en mémoire uniquement (ONDISK-03 : interdit dans repr(C)).

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// AlgoStats — statistiques par algorithme
// ─────────────────────────────────────────────────────────────────────────────

/// Compteurs de compression/décompression pour un algorithme donné.
pub struct AlgoStats {
    pub compressed_count:     AtomicU64,
    pub compressed_bytes_in:  AtomicU64,
    pub compressed_bytes_out: AtomicU64,
    pub decompress_count:     AtomicU64,
    pub decompress_bytes:     AtomicU64,
    pub compress_errors:      AtomicU64,
    pub decompress_errors:    AtomicU64,
}

impl AlgoStats {
    pub const fn new() -> Self {
        Self {
            compressed_count:     AtomicU64::new(0),
            compressed_bytes_in:  AtomicU64::new(0),
            compressed_bytes_out: AtomicU64::new(0),
            decompress_count:     AtomicU64::new(0),
            decompress_bytes:     AtomicU64::new(0),
            compress_errors:      AtomicU64::new(0),
            decompress_errors:    AtomicU64::new(0),
        }
    }

    /// Enregistre une compression (succès ou échec).
    pub fn record_compress(&self, bytes_in: u64, bytes_out: u64, ok: bool) {
        self.compressed_count.fetch_add(1, Ordering::Relaxed);
        self.compressed_bytes_in.fetch_add(bytes_in, Ordering::Relaxed);
        if ok {
            self.compressed_bytes_out.fetch_add(bytes_out, Ordering::Relaxed);
        } else {
            self.compress_errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Enregistre une décompression (succès ou échec).
    pub fn record_decompress(&self, bytes_out: u64, ok: bool) {
        self.decompress_count.fetch_add(1, Ordering::Relaxed);
        if ok {
            self.decompress_bytes.fetch_add(bytes_out, Ordering::Relaxed);
        } else {
            self.decompress_errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Ratio de compression moyen (0–100, 100 = aucun bénéfice).
    pub fn avg_ratio_percent(&self) -> u64 {
        let out = self.compressed_bytes_out.load(Ordering::Relaxed);
        let inn = self.compressed_bytes_in.load(Ordering::Relaxed);
        if inn == 0 { return 100; }
        (out * 100) / inn
    }

    /// `true` si au moins une compression a réussi.
    pub fn has_activity(&self) -> bool {
        self.compressed_count.load(Ordering::Relaxed) > 0
    }

    /// Taux d'erreur de compression (0–100).
    pub fn error_rate_percent(&self) -> u64 {
        let total  = self.compressed_count.load(Ordering::Relaxed);
        let errors = self.compress_errors.load(Ordering::Relaxed);
        if total == 0 { return 0; }
        (errors * 100) / total
    }

    /// Snapshot point-in-time des compteurs de cet algorithme.
    pub fn snapshot(&self) -> AlgoStatsSnapshot {
        AlgoStatsSnapshot {
            compressed_count:     self.compressed_count.load(Ordering::Relaxed),
            compressed_bytes_in:  self.compressed_bytes_in.load(Ordering::Relaxed),
            compressed_bytes_out: self.compressed_bytes_out.load(Ordering::Relaxed),
            decompress_count:     self.decompress_count.load(Ordering::Relaxed),
            decompress_bytes:     self.decompress_bytes.load(Ordering::Relaxed),
            compress_errors:      self.compress_errors.load(Ordering::Relaxed),
            decompress_errors:    self.decompress_errors.load(Ordering::Relaxed),
        }
    }

    /// Remet tous les compteurs à zéro.
    pub fn reset(&self) {
        self.compressed_count.store(0, Ordering::Relaxed);
        self.compressed_bytes_in.store(0, Ordering::Relaxed);
        self.compressed_bytes_out.store(0, Ordering::Relaxed);
        self.decompress_count.store(0, Ordering::Relaxed);
        self.decompress_bytes.store(0, Ordering::Relaxed);
        self.compress_errors.store(0, Ordering::Relaxed);
        self.decompress_errors.store(0, Ordering::Relaxed);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AlgoStatsSnapshot — vue immuable
// ─────────────────────────────────────────────────────────────────────────────

/// Vue immuable des compteurs d'un algorithme.
#[derive(Debug, Clone, Default)]
pub struct AlgoStatsSnapshot {
    pub compressed_count:     u64,
    pub compressed_bytes_in:  u64,
    pub compressed_bytes_out: u64,
    pub decompress_count:     u64,
    pub decompress_bytes:     u64,
    pub compress_errors:      u64,
    pub decompress_errors:    u64,
}

impl AlgoStatsSnapshot {
    /// Ratio de compression moyen (0–100).
    pub fn avg_ratio_percent(&self) -> u64 {
        if self.compressed_bytes_in == 0 { return 100; }
        (self.compressed_bytes_out * 100) / self.compressed_bytes_in
    }

    /// Économie totale de bytes (bytes_in - bytes_out), saturating.
    pub fn bytes_saved(&self) -> u64 {
        self.compressed_bytes_in.saturating_sub(self.compressed_bytes_out)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressionStats — statistiques globales
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques globales de compression pour tous les algorithmes.
pub struct CompressionStats {
    pub lz4:                     AlgoStats,
    pub zstd:                    AlgoStats,
    /// Blobs ignorés car ratio ≥ seuil (données incompressibles).
    pub skipped_incompressible:  AtomicU64,
    /// Blobs ignorés car trop petits.
    pub skipped_too_small:       AtomicU64,
    /// Blobs ignorés car déjà compressés (magic détecté).
    pub skipped_already_compressed: AtomicU64,
    /// Nombre total d'appels au writer.
    pub total_requests:          AtomicU64,
}

impl CompressionStats {
    pub const fn new() -> Self {
        Self {
            lz4:                        AlgoStats::new(),
            zstd:                       AlgoStats::new(),
            skipped_incompressible:     AtomicU64::new(0),
            skipped_too_small:          AtomicU64::new(0),
            skipped_already_compressed: AtomicU64::new(0),
            total_requests:             AtomicU64::new(0),
        }
    }

    /// Enregistre un skip pour données incompressibles.
    pub fn record_skip_incompressible(&self) {
        self.skipped_incompressible.fetch_add(1, Ordering::Relaxed);
    }

    /// Alias de compatibilité avec le code existant.
    pub fn record_skip(&self) { self.record_skip_incompressible(); }

    /// Enregistre un skip pour données trop petites.
    pub fn record_skip_too_small(&self) {
        self.skipped_too_small.fetch_add(1, Ordering::Relaxed);
    }

    /// Enregistre un skip pour données déjà compressées.
    pub fn record_skip_already_compressed(&self) {
        self.skipped_already_compressed.fetch_add(1, Ordering::Relaxed);
    }

    /// Incrémente le compteur de requêtes totales.
    pub fn record_request(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Snapshot global combinant LZ4 + Zstd.
    pub fn global_snapshot(&self) -> CompressionStatsSnapshot {
        let lz4  = self.lz4.snapshot();
        let zstd = self.zstd.snapshot();
        CompressionStatsSnapshot {
            lz4,
            zstd,
            skipped_incompressible:     self.skipped_incompressible.load(Ordering::Relaxed),
            skipped_too_small:          self.skipped_too_small.load(Ordering::Relaxed),
            skipped_already_compressed: self.skipped_already_compressed.load(Ordering::Relaxed),
            total_requests:             self.total_requests.load(Ordering::Relaxed),
        }
    }

    /// Remet tous les compteurs à zéro.
    pub fn reset(&self) {
        self.lz4.reset();
        self.zstd.reset();
        self.skipped_incompressible.store(0, Ordering::Relaxed);
        self.skipped_too_small.store(0, Ordering::Relaxed);
        self.skipped_already_compressed.store(0, Ordering::Relaxed);
        self.total_requests.store(0, Ordering::Relaxed);
    }

    /// Total des bytes économisés par toutes les compressions réussies.
    pub fn total_bytes_saved(&self) -> u64 {
        self.lz4.snapshot().bytes_saved()
            .saturating_add(self.zstd.snapshot().bytes_saved())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressionStatsSnapshot — vue immuable globale
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot global immuable de toutes les statistiques de compression.
#[derive(Debug, Clone, Default)]
pub struct CompressionStatsSnapshot {
    pub lz4:                        AlgoStatsSnapshot,
    pub zstd:                       AlgoStatsSnapshot,
    pub skipped_incompressible:     u64,
    pub skipped_too_small:          u64,
    pub skipped_already_compressed: u64,
    pub total_requests:             u64,
}

impl CompressionStatsSnapshot {
    /// Total skips toutes raisons confondues.
    pub fn total_skipped(&self) -> u64 {
        self.skipped_incompressible
            .saturating_add(self.skipped_too_small)
            .saturating_add(self.skipped_already_compressed)
    }

    /// Total des bytes effectivement compressés (entrée LZ4 + Zstd).
    pub fn total_input_bytes(&self) -> u64 {
        self.lz4.compressed_bytes_in
            .saturating_add(self.zstd.compressed_bytes_in)
    }

    /// Total sorties compressées.
    pub fn total_output_bytes(&self) -> u64 {
        self.lz4.compressed_bytes_out
            .saturating_add(self.zstd.compressed_bytes_out)
    }

    /// Économies totales (bytes_in - bytes_out), saturating.
    pub fn total_bytes_saved(&self) -> u64 {
        self.total_input_bytes().saturating_sub(self.total_output_bytes())
    }

    /// Ratio global moyen (0–100).
    pub fn global_ratio_percent(&self) -> u64 {
        let inn = self.total_input_bytes();
        let out = self.total_output_bytes();
        if inn == 0 { return 100; }
        (out * 100) / inn
    }
}

/// Statistiques globales de compression (singleton statique).
pub static COMPRESSION_STATS: CompressionStats = CompressionStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn test_initial_ratio_is_100() {
        let stats = AlgoStats::new();
        assert_eq!(stats.avg_ratio_percent(), 100);
    }

    #[test] fn test_record_compress_ok() {
        let s = AlgoStats::new();
        s.record_compress(1000, 400, true);
        assert_eq!(s.compressed_count.load(Ordering::Relaxed), 1);
        assert_eq!(s.compressed_bytes_in.load(Ordering::Relaxed), 1000);
        assert_eq!(s.compressed_bytes_out.load(Ordering::Relaxed), 400);
    }

    #[test] fn test_record_compress_error() {
        let s = AlgoStats::new();
        s.record_compress(1000, 0, false);
        assert_eq!(s.compress_errors.load(Ordering::Relaxed), 1);
    }

    #[test] fn test_avg_ratio_calculation() {
        let s = AlgoStats::new();
        s.record_compress(1000, 500, true);
        assert_eq!(s.avg_ratio_percent(), 50);
    }

    #[test] fn test_snapshot_bytes_saved() {
        let s  = AlgoStats::new();
        s.record_compress(1000, 600, true);
        let sn = s.snapshot();
        assert_eq!(sn.bytes_saved(), 400);
    }

    #[test] fn test_global_stats_reset() {
        let g = CompressionStats::new();
        g.lz4.record_compress(500, 200, true);
        g.reset();
        let sn = g.global_snapshot();
        assert_eq!(sn.lz4.compressed_count, 0);
    }

    #[test] fn test_global_snapshot_total_skipped() {
        let g = CompressionStats::new();
        g.record_skip_incompressible();
        g.record_skip_too_small();
        g.record_skip_already_compressed();
        let sn = g.global_snapshot();
        assert_eq!(sn.total_skipped(), 3);
    }

    #[test] fn test_global_ratio_zero_input() {
        let g = CompressionStats::new();
        let sn = g.global_snapshot();
        assert_eq!(sn.global_ratio_percent(), 100);
    }

    #[test] fn test_error_rate_no_ops() {
        let s = AlgoStats::new();
        assert_eq!(s.error_rate_percent(), 0);
    }

    #[test] fn test_has_activity_false_initially() {
        let s = AlgoStats::new();
        assert!(!s.has_activity());
    }

    #[test] fn test_has_activity_after_compress() {
        let s = AlgoStats::new();
        s.record_compress(100, 80, true);
        assert!(s.has_activity());
    }

    #[test] fn test_total_bytes_saved_saturating() {
        let g = CompressionStats::new();
        g.lz4.record_compress(100, 200, true); // "sortie > entrée"
        // bytes_saved = 100 - 200 = saturate at 0
        assert_eq!(g.total_bytes_saved(), 0);
    }
}

