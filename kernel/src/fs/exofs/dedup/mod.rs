//! ExoFS Dedup module — data deduplication (no_std).
//!
//! Submodules:
//!  - `chunking`           : base traits and structures for chunk splitting.
//!  - `chunk_fingerprint`  : cryptographic and fast chunk fingerprints.
//!  - `content_hash`       : BlobId computation (RULE 11: blake3 before compression).
//!  - `chunker_fixed`      : fixed-size chunking.
//!  - `chunker_cdc`        : content-defined chunking (Rabin-Karp).
//!  - `chunk_cache`        : chunk LRU cache.
//!  - `chunk_index`        : global index of all chunks.
//!  - `blob_registry`      : blob registry with ref-counting.
//!  - `blob_sharing`       : chunk <-> shared blobs tracking.
//!  - `dedup_policy`       : configurable deduplication policies.
//!  - `dedup_stats`        : engine statistics and metrics.
//!  - `similarity_detect`  : similarity detection via min-hashing.
//!  - `dedup_api`          : high-level engine API.
//!
//! RULE 11: BlobId = blake3(data BEFORE compression/encryption).
//!
//! RECUR-01: zero recursion across the entire module.
//! OOM-02  : try_reserve on all Vec allocations.
//! ARITH-02: saturating / checked / wrapping on all arithmetic.

pub mod blob_registry;
pub mod blob_sharing;
pub mod chunk_cache;
pub mod chunk_fingerprint;
pub mod chunk_index;
pub mod chunker_cdc;
pub mod chunker_fixed;
pub mod chunking;
pub mod content_hash;
pub mod dedup_api;
pub mod dedup_policy;
pub mod dedup_stats;
pub mod similarity_detect;

// ─────────────────────────────────────────────────────────────────────────────
// Public re-exports
// ─────────────────────────────────────────────────────────────────────────────

pub use chunking::{
    ChunkBoundary, ChunkKind, ChunkList, ChunkStats, Chunker, DedupChunk, CHUNK_MAX_PER_BLOB,
    CHUNK_MAX_SIZE, CHUNK_MIN_SIZE, CHUNK_TARGET_SIZE,
};

pub use chunk_fingerprint::{
    blake3_hash, fnv1a64, ChunkFingerprint, FingerprintAlgorithm, FingerprintSet,
};

pub use content_hash::{ContentHash, ContentHashResult, HashAlgorithm, CONTENT_HASH};

pub use chunker_fixed::{BatchFixedChunker, FixedChunker, FixedChunkerConfig};

pub use chunker_cdc::{
    AdaptiveCdcChunker, CdcChunker, CdcConfig, CdcSharpness, CdcStats, CDC_AVG_SIZE, CDC_MAX_SIZE,
    CDC_MIN_SIZE,
};

pub use chunk_cache::{ChunkCache, ChunkCacheStats, CHUNK_CACHE};

pub use chunk_index::{ChunkEntry, ChunkIndex, ChunkIndexStats, CHUNK_INDEX};

pub use blob_registry::{BlobEntry, BlobRegistry, BlobRegistryStats, BlobSummary, BLOB_REGISTRY};

pub use blob_sharing::{
    BlobDeletionAnalysis, BlobSharing, BlobSharingStats, SharedChunkRef, BLOB_SHARING,
};

pub use dedup_policy::{
    DedupMode, DedupPolicy, DedupPolicyEngine, DedupPolicyPreset, DedupPolicyReport,
    DedupPolicyRule, DedupPriority, PolicyCondition,
};

pub use dedup_stats::{
    DedupEfficiencyMetrics, DedupStats, DedupStatsDelta, DedupStatsHistory, DedupStatsSummary,
    DEDUP_STATS,
};

pub use similarity_detect::{
    BlobSignature, SignatureStore, SimilarityDetector, SimilarityMatch, MIN_HASH_COUNT,
    SHINGLE_SIZE, SIMILARITY_THRESHOLD_PCT,
};

pub use dedup_api::{DedupApi, DedupApiSummary, DedupBatchReport, DedupHealthStatus, DedupResult};

// ─────────────────────────────────────────────────────────────────────────────
// DedupConfig — central module configuration
// ─────────────────────────────────────────────────────────────────────────────

use crate::fs::exofs::core::{ExofsError, ExofsResult};

/// Global configuration for the dedup module.
#[derive(Debug, Clone)]
pub struct DedupConfig {
    pub policy: DedupPolicy,
    pub cache_capacity: usize,
    pub index_max_entries: usize,
    pub registry_max_entries: usize,
    pub enable_similarity: bool,
    pub similarity_threshold: u8,
}

impl DedupConfig {
    pub fn default() -> Self {
        Self {
            policy: DedupPolicy::default(),
            cache_capacity: chunk_cache::CHUNK_CACHE_DEFAULT_CAPACITY,
            index_max_entries: chunk_index::CHUNK_INDEX_MAX_ENTRIES,
            registry_max_entries: blob_registry::BLOB_REGISTRY_MAX_ENTRIES,
            enable_similarity: false,
            similarity_threshold: SIMILARITY_THRESHOLD_PCT,
        }
    }

    pub fn validate(&self) -> ExofsResult<()> {
        self.policy.validate()?;
        if self.cache_capacity == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        if self.similarity_threshold > 100 {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupModule — module orchestrator
// ─────────────────────────────────────────────────────────────────────────────

/// Unified entry point for the deduplication module.
pub struct DedupModule {
    pub config: DedupConfig,
    api: DedupApi,
}

impl DedupModule {
    /// Initializes the module with a configuration.
    pub fn init(config: DedupConfig) -> ExofsResult<Self> {
        config.validate()?;
        let api = DedupApi::new(config.policy.clone())?;
        Ok(Self { config, api })
    }

    /// Initializes with default configuration.
    pub fn init_default() -> ExofsResult<Self> {
        Self::init(DedupConfig::default())
    }

    /// Deduplicates a blob.
    pub fn dedup(&self, data: &[u8]) -> ExofsResult<DedupResult> {
        self.api.dedup_blob(data)
    }

    /// Deduplicates a batch of blobs.
    pub fn dedup_batch(&self, blobs: &[&[u8]]) -> ExofsResult<alloc::vec::Vec<DedupResult>> {
        self.api.dedup_batch(blobs)
    }

    /// Deletes a blob and frees orphan chunks.
    pub fn delete(&self, blob_id: &crate::fs::exofs::core::BlobId) -> ExofsResult<u32> {
        self.api.delete_blob(blob_id)
    }

    /// Health check.
    pub fn health_check(&self) -> DedupHealthStatus {
        self.api.health_check()
    }

    /// State summary.
    pub fn summary(&self) -> DedupApiSummary {
        self.api.summary()
    }

    /// Complete statistics snapshot.
    pub fn stats_snapshot(&self) -> DedupStatsSummary {
        DEDUP_STATS.snapshot()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Module-level convenience functions
// ─────────────────────────────────────────────────────────────────────────────

/// Deduplicates a blob using the default policy.
/// Convenience function — uses global singletons.
pub fn dedup_blob(data: &[u8]) -> ExofsResult<DedupResult> {
    DedupApi::with_default_policy()?.dedup_blob(data)
}

/// Verifies integrity of the entire deduplication subsystem.
pub fn verify_dedup_integrity() -> ExofsResult<()> {
    BLOB_REGISTRY.verify_integrity()?;
    BLOB_SHARING.verify_integrity()?;
    CHUNK_INDEX.verify_integrity()?;
    Ok(())
}

/// Returns current deduplication statistics.
pub fn current_stats() -> DedupStatsSummary {
    DEDUP_STATS.snapshot()
}

// ─────────────────────────────────────────────────────────────────────────────
// Module integration tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_init_default() {
        let m = DedupModule::init_default().test_unwrap();
        assert!(m.config.validate().is_ok());
    }

    #[test]
    fn test_module_dedup_blob() {
        let m = DedupModule::init_default().test_unwrap();
        let data = &[0xA5u8; 8192];
        let r = m.dedup(data).test_unwrap();
        assert!(r.n_chunks > 0 || r.logical_bytes == data.len() as u64);
    }

    #[test]
    fn test_module_dedup_batch() {
        let m = DedupModule::init_default().test_unwrap();
        let d1 = &[0x11u8; 8192] as &[u8];
        let d2 = &[0x22u8; 8192] as &[u8];
        let res = m.dedup_batch(&[d1, d2]).test_unwrap();
        assert_eq!(res.len(), 2);
    }

    #[test]
    fn test_module_health_check() {
        let m = DedupModule::init_default().test_unwrap();
        let h = m.health_check();
        assert!(h.overall_ok);
    }

    #[test]
    fn test_module_stats_snapshot() {
        let m = DedupModule::init_default().test_unwrap();
        let s = m.stats_snapshot();
        assert!(s.is_consistent());
    }

    #[test]
    fn test_module_summary() {
        let m = DedupModule::init_default().test_unwrap();
        let _ = m.summary();
    }

    #[test]
    fn test_convenience_dedup_blob() {
        let data = &[0x55u8; 8192];
        let r = dedup_blob(data).test_unwrap();
        assert!(r.logical_bytes == 8192);
    }

    #[test]
    fn test_verify_integrity() {
        assert!(verify_dedup_integrity().is_ok());
    }

    #[test]
    fn test_current_stats() {
        let s = current_stats();
        assert!(s.is_consistent());
    }

    #[test]
    fn test_dedup_config_default_valid() {
        assert!(DedupConfig::default().validate().is_ok());
    }

    #[test]
    fn test_dedup_config_invalid_cache() {
        let mut c = DedupConfig::default();
        c.cache_capacity = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn test_cdc_chunker_via_module() {
        let c = CdcChunker::default_chunker();
        let d = &[0xFEu8; CDC_AVG_SIZE * 2];
        let cs = c.chunk(d).test_unwrap();
        assert!(!cs.is_empty());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupGarbageCollector — garbage collector for orphan chunks
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a GC cycle.
#[derive(Debug, Clone, Copy)]
pub struct GarbageCollectionResult {
    pub orphan_chunks_cleaned: usize,
    pub orphan_blobs_cleaned: usize,
    pub bytes_reclaimed: u64,
}

impl GarbageCollectionResult {
    pub fn empty() -> Self {
        Self {
            orphan_chunks_cleaned: 0,
            orphan_blobs_cleaned: 0,
            bytes_reclaimed: 0,
        }
    }
}

/// Garbage collector for orphan chunks and blobs.
pub struct DedupGarbageCollector;

impl DedupGarbageCollector {
    /// Removes all orphan blobs (ref_count == 0).
    ///
    /// RECUR-01 : for loop.
    /// ARITH-02 : saturating_add.
    pub fn collect_orphan_blobs() -> ExofsResult<GarbageCollectionResult> {
        let orphans = BLOB_REGISTRY.orphan_blobs()?;
        let mut cleaned = 0usize;
        let mut bytes = 0u64;
        for bid in &orphans {
            if let Some(entry) = BLOB_REGISTRY.lookup(bid) {
                bytes = bytes.saturating_add(entry.total_size);
                BLOB_SHARING.remove_blob_refs(&entry.chunk_keys, bid);
            }
            BLOB_REGISTRY.deregister(bid);
            cleaned = cleaned.saturating_add(1);
        }
        Ok(GarbageCollectionResult {
            orphan_chunks_cleaned: 0,
            orphan_blobs_cleaned: cleaned,
            bytes_reclaimed: bytes,
        })
    }

    /// Removes all orphan chunks from index.
    ///
    /// RECUR-01 : for loop.
    /// ARITH-02 : saturating_add.
    pub fn collect_orphan_chunks() -> ExofsResult<GarbageCollectionResult> {
        let keys = CHUNK_INDEX.orphan_keys()?;
        let mut cleaned = 0usize;
        for key in &keys {
            CHUNK_INDEX.force_remove(key);
            cleaned = cleaned.saturating_add(1);
        }
        Ok(GarbageCollectionResult {
            orphan_chunks_cleaned: cleaned,
            orphan_blobs_cleaned: 0,
            bytes_reclaimed: 0,
        })
    }

    /// Full GC cycle.
    pub fn full_collect() -> ExofsResult<GarbageCollectionResult> {
        let r1 = Self::collect_orphan_blobs()?;
        let r2 = Self::collect_orphan_chunks()?;
        Ok(GarbageCollectionResult {
            orphan_blobs_cleaned: r1.orphan_blobs_cleaned,
            orphan_chunks_cleaned: r2.orphan_chunks_cleaned,
            bytes_reclaimed: r1.bytes_reclaimed.saturating_add(r2.bytes_reclaimed),
        })
    }
}

impl DedupModule {
    /// Triggers a garbage collection cycle.
    pub fn gc(&self) -> ExofsResult<GarbageCollectionResult> {
        DedupGarbageCollector::full_collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GC tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_gc {
    use super::*;

    #[test]
    fn test_gc_empty_state() {
        let r = DedupGarbageCollector::full_collect().test_unwrap();
        let _ = r.orphan_blobs_cleaned; // access without panic
    }

    #[test]
    fn test_module_gc() {
        let m = DedupModule::init_default().test_unwrap();
        let r = m.gc().test_unwrap();
        assert_eq!(r.orphan_blobs_cleaned, 0);
        assert_eq!(r.orphan_chunks_cleaned, 0);
    }

    #[test]
    fn test_gc_result_empty() {
        let r = GarbageCollectionResult::empty();
        assert_eq!(r.bytes_reclaimed, 0);
    }

    #[test]
    fn test_full_dedup_pipeline_via_module() {
        let m = DedupModule::init_default().test_unwrap();
        let data = &[0xEEu8; CDC_AVG_SIZE];
        let r = m.dedup(data).test_unwrap();
        assert!(r.logical_bytes > 0);
        let h = m.health_check();
        assert!(h.overall_ok);
        let gc = m.gc().test_unwrap();
        let _ = gc.bytes_reclaimed;
    }

    #[test]
    fn test_dedup_same_blob_twice() {
        let m = DedupModule::init_default().test_unwrap();
        let data = &[0xEEu8; CDC_AVG_SIZE];
        let r1 = m.dedup(data).test_unwrap();
        let r2 = m.dedup(data).test_unwrap();
        // The second call must recognize the blob.
        assert!(!r2.is_new);
        assert_eq!(r1.blob_id.as_bytes(), r2.blob_id.as_bytes());
    }
}
