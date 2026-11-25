//! ExoFS Dedup module — data deduplication (wiring facade, `no_std`).

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
mod facade;
pub mod similarity_detect;

pub use blob_registry::{BlobEntry, BlobRegistry, BlobRegistryStats, BlobSummary, BLOB_REGISTRY};
pub use blob_sharing::{
    BlobDeletionAnalysis, BlobSharing, BlobSharingStats, SharedChunkRef, BLOB_SHARING,
};
pub use chunk_cache::{ChunkCache, ChunkCacheStats, CHUNK_CACHE};
pub use chunk_fingerprint::{
    blake3_hash, fnv1a64, ChunkFingerprint, FingerprintAlgorithm, FingerprintSet,
};
pub use chunk_index::{ChunkEntry, ChunkIndex, ChunkIndexStats, CHUNK_INDEX};
pub use chunker_cdc::{
    AdaptiveCdcChunker, CdcChunker, CdcConfig, CdcSharpness, CdcStats, CDC_AVG_SIZE, CDC_MAX_SIZE,
    CDC_MIN_SIZE,
};
pub use chunker_fixed::{BatchFixedChunker, FixedChunker, FixedChunkerConfig};
pub use chunking::{
    ChunkBoundary, ChunkKind, ChunkList, ChunkStats, Chunker, DedupChunk, CHUNK_MAX_PER_BLOB,
    CHUNK_MAX_SIZE, CHUNK_MIN_SIZE, CHUNK_TARGET_SIZE,
};
pub use content_hash::{ContentHash, ContentHashResult, HashAlgorithm, CONTENT_HASH};
pub use dedup_api::{DedupApi, DedupApiSummary, DedupBatchReport, DedupHealthStatus, DedupResult};
pub use dedup_policy::{
    DedupMode, DedupPolicy, DedupPolicyEngine, DedupPolicyPreset, DedupPolicyReport,
    DedupPolicyRule, DedupPriority, PolicyCondition,
};
pub use dedup_stats::{
    DedupEfficiencyMetrics, DedupStats, DedupStatsDelta, DedupStatsHistory, DedupStatsSummary,
    DEDUP_STATS,
};
pub use facade::{
    current_stats, dedup_blob, verify_dedup_integrity, DedupConfig, DedupGarbageCollector,
    DedupModule, GarbageCollectionResult,
};
pub use similarity_detect::{
    BlobSignature, SignatureStore, SimilarityDetector, SimilarityMatch, MIN_HASH_COUNT,
    SHINGLE_SIZE, SIMILARITY_THRESHOLD_PCT,
};
