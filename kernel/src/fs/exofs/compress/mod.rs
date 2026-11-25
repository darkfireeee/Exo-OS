//! ExoFS compression module — LZ4 + Zstd, adaptive selection (wiring facade).

pub mod algorithm;
pub mod compress_benchmark;
pub mod compress_choice;
pub mod compress_header;
pub mod compress_stats;
pub mod compress_threshold;
pub mod compress_writer;
pub mod decompress_reader;
mod facade;
pub mod lz4_wrapper;
pub mod zstd_wrapper;

pub use algorithm::{
    AlgorithmCapabilities, CompressLevel, CompressionAlgorithm, CompressionProfile,
};
pub use compress_benchmark::{BenchHistory, BenchResult, BenchSummary, CompressBenchmark};
pub use compress_choice::{
    CompressDecision, CompressPolicy, CompressionChoice, DecisionReason, PolicyPresets,
};
pub use compress_header::{
    CompressedBlobView, CompressionHeader, HeaderBuilder, COMPRESSION_HEADER_SIZE,
    COMPRESSION_MAGIC, HEADER_VERSION,
};
pub use compress_stats::{
    AlgoStats, AlgoStatsSnapshot, CompressionStats, CompressionStatsSnapshot, COMPRESSION_STATS,
};
pub use compress_threshold::CompressionThreshold;
pub use compress_writer::{CompressResult, CompressWriter, CompressionPipeline};
pub use decompress_reader::{DecompressReader, DecompressStats};
pub use facade::{
    compress_blob, compress_validated, decompress_blob, is_compressed_blob, CompressModule,
    CompressorConfig,
};
pub use lz4_wrapper::Lz4Compressor;
pub use zstd_wrapper::{
    ZstdCompressor, ZstdConfig, ZstdDecoder, ZSTD_DEFAULT_LEVEL, ZSTD_MAX_LEVEL,
};
