//! Module ExoFS Dedup — déduplication de données (no_std).
//!
//! Sous-modules :
//!  - `chunking`           : traits et structures de base pour le découpage.
//!  - `chunk_fingerprint`  : empreintes cryptographiques et rapides de chunks.
//!  - `content_hash`       : calcul du BlobId (RÈGLE 11 : blake3 avant compres.).
//!  - `chunker_fixed`      : découpage à taille fixe.
//!  - `chunker_cdc`        : découpage content-defined (Rabin-Karp).
//!  - `chunk_cache`        : cache LRU de chunks.
//!  - `chunk_index`        : index global de tous les chunks.
//!  - `blob_registry`      : registre des blobs avec ref-counting.
//!  - `blob_sharing`       : suivi chunk ↔ blobs partagés.
//!  - `dedup_policy`       : politiques de déduplication configurables.
//!  - `dedup_stats`        : statistiques et métriques du moteur.
//!  - `similarity_detect`  : détection de similarité par min-hashing.
//!  - `dedup_api`          : API haut niveau du moteur.
//!
//! RÈGLE 11 : BlobId = blake3(données AVANT compression/chiffrement).
//!
//! RECUR-01 : aucune récursion dans tout le module.
//! OOM-02   : try_reserve sur tous les Vec.
//! ARITH-02 : saturating / checked / wrapping sur toute l'arithmétique.

pub mod chunking;
pub mod chunk_fingerprint;
pub mod content_hash;
pub mod chunker_fixed;
pub mod chunker_cdc;
pub mod chunk_cache;
pub mod chunk_index;
pub mod blob_registry;
pub mod blob_sharing;
pub mod dedup_policy;
pub mod dedup_stats;
pub mod similarity_detect;
pub mod dedup_api;

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports publics
// ─────────────────────────────────────────────────────────────────────────────

pub use chunking::{
    Chunker, ChunkBoundary, DedupChunk, ChunkKind, ChunkStats, ChunkList,
    CHUNK_MIN_SIZE, CHUNK_TARGET_SIZE, CHUNK_MAX_SIZE, CHUNK_MAX_PER_BLOB,
};

pub use chunk_fingerprint::{
    ChunkFingerprint, FingerprintAlgorithm, FingerprintSet,
    blake3_hash, fnv1a64,
};

pub use content_hash::{ContentHash, ContentHashResult, HashAlgorithm, CONTENT_HASH};

pub use chunker_fixed::{FixedChunker, FixedChunkerConfig, BatchFixedChunker};

pub use chunker_cdc::{
    CdcChunker, CdcConfig, CdcStats, AdaptiveCdcChunker, CdcSharpness,
    CDC_MIN_SIZE, CDC_AVG_SIZE, CDC_MAX_SIZE,
};

pub use chunk_cache::{ChunkCache, ChunkCacheStats, CHUNK_CACHE};

pub use chunk_index::{ChunkIndex, ChunkEntry, ChunkIndexStats, CHUNK_INDEX};

pub use blob_registry::{BlobRegistry, BlobEntry, BlobRegistryStats, BlobSummary, BLOB_REGISTRY};

pub use blob_sharing::{BlobSharing, SharedChunkRef, BlobSharingStats, BlobDeletionAnalysis, BLOB_SHARING};

pub use dedup_policy::{
    DedupPolicy, DedupMode, DedupPriority, DedupPolicyEngine,
    DedupPolicyRule, PolicyCondition, DedupPolicyPreset, DedupPolicyReport,
};

pub use dedup_stats::{
    DedupStats, DedupStatsSummary, DedupStatsHistory,
    DedupStatsDelta, DedupEfficiencyMetrics, DEDUP_STATS,
};

pub use similarity_detect::{
    BlobSignature, SimilarityDetector, SimilarityMatch, SignatureStore,
    SHINGLE_SIZE, MIN_HASH_COUNT, SIMILARITY_THRESHOLD_PCT,
};

pub use dedup_api::{
    DedupApi, DedupResult, DedupBatchReport,
    DedupHealthStatus, DedupApiSummary,
};

// ─────────────────────────────────────────────────────────────────────────────
// DedupConfig — configuration centrale du module
// ─────────────────────────────────────────────────────────────────────────────

use crate::fs::exofs::core::{ExofsError, ExofsResult};

/// Configuration globale du module dedup.
#[derive(Debug, Clone)]
pub struct DedupConfig {
    pub policy:               DedupPolicy,
    pub cache_capacity:       usize,
    pub index_max_entries:    usize,
    pub registry_max_entries: usize,
    pub enable_similarity:    bool,
    pub similarity_threshold: u8,
}

impl DedupConfig {
    pub fn default() -> Self {
        Self {
            policy:               DedupPolicy::default(),
            cache_capacity:       chunk_cache::CHUNK_CACHE_DEFAULT_CAPACITY,
            index_max_entries:    chunk_index::CHUNK_INDEX_MAX_ENTRIES,
            registry_max_entries: blob_registry::BLOB_REGISTRY_MAX_ENTRIES,
            enable_similarity:    false,
            similarity_threshold: SIMILARITY_THRESHOLD_PCT,
        }
    }

    pub fn validate(&self) -> ExofsResult<()> {
        self.policy.validate()?;
        if self.cache_capacity == 0    { return Err(ExofsError::InvalidArgument); }
        if self.similarity_threshold > 100 { return Err(ExofsError::InvalidArgument); }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupModule — orchestrateur du module
// ─────────────────────────────────────────────────────────────────────────────

/// Point d'entrée unifié du module de déduplication.
pub struct DedupModule {
    pub config: DedupConfig,
    api:        DedupApi,
}

impl DedupModule {
    /// Initialise le module avec une configuration.
    pub fn init(config: DedupConfig) -> ExofsResult<Self> {
        config.validate()?;
        let api = DedupApi::new(config.policy.clone())?;
        Ok(Self { config, api })
    }

    /// Initialise avec la configuration par défaut.
    pub fn init_default() -> ExofsResult<Self> {
        Self::init(DedupConfig::default())
    }

    /// Déduplique un blob.
    pub fn dedup(&self, data: &[u8]) -> ExofsResult<DedupResult> {
        self.api.dedup_blob(data)
    }

    /// Déduplique un lot de blobs.
    pub fn dedup_batch(&self, blobs: &[&[u8]]) -> ExofsResult<alloc::vec::Vec<DedupResult>> {
        self.api.dedup_batch(blobs)
    }

    /// Supprime un blob et libère les chunks orphelins.
    pub fn delete(&self, blob_id: &crate::fs::exofs::core::BlobId) -> ExofsResult<u32> {
        self.api.delete_blob(blob_id)
    }

    /// Contrôle de santé.
    pub fn health_check(&self) -> DedupHealthStatus {
        self.api.health_check()
    }

    /// Résumé de l'état.
    pub fn summary(&self) -> DedupApiSummary {
        self.api.summary()
    }

    /// Statistiques complètes.
    pub fn stats_snapshot(&self) -> DedupStatsSummary {
        DEDUP_STATS.snapshot()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions de commodité de niveau module
// ─────────────────────────────────────────────────────────────────────────────

/// Déduplique un blob avec la politique par défaut.
/// Fonction de commodité — utilise les singletons globaux.
pub fn dedup_blob(data: &[u8]) -> ExofsResult<DedupResult> {
    DedupApi::with_default_policy()?.dedup_blob(data)
}

/// Vérifie l'intégrité de tout le sous-système de déduplication.
pub fn verify_dedup_integrity() -> ExofsResult<()> {
    BLOB_REGISTRY.verify_integrity()?;
    BLOB_SHARING.verify_integrity()?;
    CHUNK_INDEX.verify_integrity()?;
    Ok(())
}

/// Retourne les statistiques actuelles de déduplication.
pub fn current_stats() -> DedupStatsSummary {
    DEDUP_STATS.snapshot()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests d'intégration du module
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn test_module_init_default() {
        let m = DedupModule::init_default().unwrap();
        assert!(m.config.validate().is_ok());
    }

    #[test] fn test_module_dedup_blob() {
        let m    = DedupModule::init_default().unwrap();
        let data = &[0xA5u8; 8192];
        let r    = m.dedup(data).unwrap();
        assert!(r.n_chunks > 0 || r.logical_bytes == data.len() as u64);
    }

    #[test] fn test_module_dedup_batch() {
        let m   = DedupModule::init_default().unwrap();
        let d1  = &[0x11u8; 8192] as &[u8];
        let d2  = &[0x22u8; 8192] as &[u8];
        let res = m.dedup_batch(&[d1, d2]).unwrap();
        assert_eq!(res.len(), 2);
    }

    #[test] fn test_module_health_check() {
        let m = DedupModule::init_default().unwrap();
        let h = m.health_check();
        assert!(h.overall_ok);
    }

    #[test] fn test_module_stats_snapshot() {
        let m = DedupModule::init_default().unwrap();
        let s = m.stats_snapshot();
        assert!(s.is_consistent());
    }

    #[test] fn test_module_summary() {
        let m = DedupModule::init_default().unwrap();
        let _ = m.summary();
    }

    #[test] fn test_convenience_dedup_blob() {
        let data = &[0x55u8; 8192];
        let r    = dedup_blob(data).unwrap();
        assert!(r.logical_bytes == 8192);
    }

    #[test] fn test_verify_integrity() {
        assert!(verify_dedup_integrity().is_ok());
    }

    #[test] fn test_current_stats() {
        let s = current_stats();
        assert!(s.is_consistent());
    }

    #[test] fn test_dedup_config_default_valid() {
        assert!(DedupConfig::default().validate().is_ok());
    }

    #[test] fn test_dedup_config_invalid_cache() {
        let mut c = DedupConfig::default();
        c.cache_capacity = 0;
        assert!(c.validate().is_err());
    }

    #[test] fn test_cdc_chunker_via_module() {
        let c  = CdcChunker::default_chunker();
        let d  = &[0xFEu8; CDC_AVG_SIZE * 2];
        let cs = c.chunk(d).unwrap();
        assert!(!cs.is_empty());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupGarbageCollector — collecteur de garbage pour les chunks orphelins
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'un cycle de GC.
#[derive(Debug, Clone, Copy)]
pub struct GarbageCollectionResult {
    pub orphan_chunks_cleaned:  usize,
    pub orphan_blobs_cleaned:   usize,
    pub bytes_reclaimed:        u64,
}

impl GarbageCollectionResult {
    pub fn empty() -> Self {
        Self { orphan_chunks_cleaned: 0, orphan_blobs_cleaned: 0, bytes_reclaimed: 0 }
    }
}

/// Collecteur de chunks et blobs orphelins.
pub struct DedupGarbageCollector;

impl DedupGarbageCollector {
    /// Supprime tous les blobs orphelins (ref_count == 0).
    ///
    /// RECUR-01 : boucle for.
    /// ARITH-02 : saturating_add.
    pub fn collect_orphan_blobs() -> ExofsResult<GarbageCollectionResult> {
        let orphans  = BLOB_REGISTRY.orphan_blobs()?;
        let mut cleaned = 0usize;
        let mut bytes   = 0u64;
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
            orphan_blobs_cleaned:  cleaned,
            bytes_reclaimed:       bytes,
        })
    }

    /// Supprime tous les chunks orphelins de l'index.
    ///
    /// RECUR-01 : boucle for.
    /// ARITH-02 : saturating_add.
    pub fn collect_orphan_chunks() -> ExofsResult<GarbageCollectionResult> {
        let keys    = CHUNK_INDEX.orphan_keys()?;
        let mut cleaned = 0usize;
        for key in &keys {
            CHUNK_INDEX.force_remove(key);
            cleaned = cleaned.saturating_add(1);
        }
        Ok(GarbageCollectionResult {
            orphan_chunks_cleaned: cleaned,
            orphan_blobs_cleaned:  0,
            bytes_reclaimed:       0,
        })
    }

    /// Cycle de GC complet.
    pub fn full_collect() -> ExofsResult<GarbageCollectionResult> {
        let r1 = Self::collect_orphan_blobs()?;
        let r2 = Self::collect_orphan_chunks()?;
        Ok(GarbageCollectionResult {
            orphan_blobs_cleaned:  r1.orphan_blobs_cleaned,
            orphan_chunks_cleaned: r2.orphan_chunks_cleaned,
            bytes_reclaimed:       r1.bytes_reclaimed.saturating_add(r2.bytes_reclaimed),
        })
    }
}

impl DedupModule {
    /// Lance un cycle de garbage collection.
    pub fn gc(&self) -> ExofsResult<GarbageCollectionResult> {
        DedupGarbageCollector::full_collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests GC
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_gc {
    use super::*;

    #[test] fn test_gc_empty_state() {
        let r = DedupGarbageCollector::full_collect().unwrap();
        let _ = r.orphan_blobs_cleaned;  // accès sans panique
    }

    #[test] fn test_module_gc() {
        let m = DedupModule::init_default().unwrap();
        let r = m.gc().unwrap();
        assert_eq!(r.orphan_blobs_cleaned, 0);
        assert_eq!(r.orphan_chunks_cleaned, 0);
    }

    #[test] fn test_gc_result_empty() {
        let r = GarbageCollectionResult::empty();
        assert_eq!(r.bytes_reclaimed, 0);
    }

    #[test] fn test_full_dedup_pipeline_via_module() {
        let m    = DedupModule::init_default().unwrap();
        let data = &[0xEEu8; CDC_AVG_SIZE];
        let r    = m.dedup(data).unwrap();
        assert!(r.logical_bytes > 0);
        let h = m.health_check();
        assert!(h.overall_ok);
        let gc = m.gc().unwrap();
        let _  = gc.bytes_reclaimed;
    }

    #[test] fn test_dedup_same_blob_twice() {
        let m    = DedupModule::init_default().unwrap();
        let data = &[0xEEu8; CDC_AVG_SIZE];
        let r1   = m.dedup(data).unwrap();
        let r2   = m.dedup(data).unwrap();
        // Le second appel doit reconnaître le blob.
        assert!(!r2.is_new);
        assert_eq!(r1.blob_id.as_bytes(), r2.blob_id.as_bytes());
    }
}
