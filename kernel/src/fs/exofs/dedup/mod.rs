//! Module dedup/ — déduplication de blobs ExoFS (no_std).
//!
//! Regroupe : découpage (CDC/fixe), empreintes (Blake3), index de chunks,
//! registre de blobs partagés et politiques de déduplication.

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

pub use blob_registry::BlobRegistry;
pub use blob_sharing::BlobSharing;
pub use chunk_cache::ChunkCache;
pub use chunk_fingerprint::{ChunkFingerprint, FingerprintAlgorithm};
pub use chunk_index::{ChunkIndex, ChunkEntry};
pub use chunker_cdc::CdcChunker;
pub use chunker_fixed::FixedChunker;
pub use chunking::{Chunker, ChunkBoundary, DedupChunk};
pub use content_hash::{ContentHash, CONTENT_HASH};
pub use dedup_api::{DedupApi, DedupResult};
pub use dedup_policy::{DedupPolicy, DedupMode};
pub use dedup_stats::{DedupStats, DEDUP_STATS};
pub use similarity_detect::{SimilarityDetector, SimilarityMatch};
