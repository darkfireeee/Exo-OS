//! ExoFS Dedup facade (`DedupConfig`, `DedupModule`, `DedupGarbageCollector`) and tests.
//!
//! Extracted from `exofs/dedup/mod.rs` so `mod.rs` remains a pure wiring facade.

use super::{
    blob_registry, chunk_cache, chunk_index, DedupApi, DedupApiSummary, DedupHealthStatus,
    DedupPolicy, DedupResult, DedupStatsSummary, BLOB_REGISTRY, BLOB_SHARING, CHUNK_INDEX,
    DEDUP_STATS, SIMILARITY_THRESHOLD_PCT,
};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

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

pub struct DedupModule {
    pub config: DedupConfig,
    api: DedupApi,
}

impl DedupModule {
    pub fn init(config: DedupConfig) -> ExofsResult<Self> {
        config.validate()?;
        let api = DedupApi::new(config.policy.clone())?;
        Ok(Self { config, api })
    }

    pub fn init_default() -> ExofsResult<Self> {
        Self::init(DedupConfig::default())
    }

    pub fn dedup(&self, data: &[u8]) -> ExofsResult<DedupResult> {
        self.api.dedup_blob(data)
    }

    pub fn dedup_batch(&self, blobs: &[&[u8]]) -> ExofsResult<alloc::vec::Vec<DedupResult>> {
        self.api.dedup_batch(blobs)
    }

    pub fn delete(&self, blob_id: &crate::fs::exofs::core::BlobId) -> ExofsResult<u32> {
        self.api.delete_blob(blob_id)
    }

    pub fn health_check(&self) -> DedupHealthStatus {
        self.api.health_check()
    }

    pub fn summary(&self) -> DedupApiSummary {
        self.api.summary()
    }

    pub fn stats_snapshot(&self) -> DedupStatsSummary {
        DEDUP_STATS.snapshot()
    }

    pub fn gc(&self) -> ExofsResult<GarbageCollectionResult> {
        DedupGarbageCollector::full_collect()
    }
}

pub fn dedup_blob(data: &[u8]) -> ExofsResult<DedupResult> {
    DedupApi::with_default_policy()?.dedup_blob(data)
}

pub fn verify_dedup_integrity() -> ExofsResult<()> {
    BLOB_REGISTRY.verify_integrity()?;
    BLOB_SHARING.verify_integrity()?;
    CHUNK_INDEX.verify_integrity()?;
    Ok(())
}

pub fn current_stats() -> DedupStatsSummary {
    DEDUP_STATS.snapshot()
}

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

pub struct DedupGarbageCollector;

impl DedupGarbageCollector {
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

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::super::{CdcChunker, Chunker, CDC_AVG_SIZE};
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
        assert!(m.health_check().overall_ok);
    }

    #[test]
    fn test_module_stats_snapshot() {
        let m = DedupModule::init_default().test_unwrap();
        assert!(m.stats_snapshot().is_consistent());
    }

    #[test]
    fn test_module_summary() {
        let m = DedupModule::init_default().test_unwrap();
        let _ = m.summary();
    }

    #[test]
    fn test_convenience_dedup_blob() {
        let data = &[0x55u8; 8192];
        assert_eq!(dedup_blob(data).test_unwrap().logical_bytes, 8192);
    }

    #[test]
    fn test_verify_integrity() {
        assert!(verify_dedup_integrity().is_ok());
    }

    #[test]
    fn test_current_stats() {
        assert!(current_stats().is_consistent());
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
        assert!(!c.chunk(d).test_unwrap().is_empty());
    }

    #[test]
    fn test_gc_empty_state() {
        let r = DedupGarbageCollector::full_collect().test_unwrap();
        let _ = r.orphan_blobs_cleaned;
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
        assert_eq!(GarbageCollectionResult::empty().bytes_reclaimed, 0);
    }

    #[test]
    fn test_full_dedup_pipeline_via_module() {
        let m = DedupModule::init_default().test_unwrap();
        let data = &[0xEEu8; CDC_AVG_SIZE];
        let r = m.dedup(data).test_unwrap();
        assert!(r.logical_bytes > 0);
        assert!(m.health_check().overall_ok);
        let _ = m.gc().test_unwrap().bytes_reclaimed;
    }

    #[test]
    fn test_dedup_same_blob_twice() {
        let m = DedupModule::init_default().test_unwrap();
        let data = &[0xEEu8; CDC_AVG_SIZE];
        let r1 = m.dedup(data).test_unwrap();
        let r2 = m.dedup(data).test_unwrap();
        assert!(!r2.is_new);
        assert_eq!(r1.blob_id.as_bytes(), r2.blob_id.as_bytes());
    }
}
