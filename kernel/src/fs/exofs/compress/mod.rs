//! Module de compression ExoFS — LZ4 + Zstd, sélection adaptative.
//!
//! RÈGLE 11 : BlobId = Blake3(données AVANT compression).
//! Le BlobId est toujours calculé sur les données brutes, pas compressées.

#![allow(dead_code)]

pub mod algorithm;
pub mod compress_benchmark;
pub mod compress_choice;
pub mod compress_header;
pub mod compress_stats;
pub mod compress_threshold;
pub mod compress_writer;
pub mod decompress_reader;
pub mod lz4_wrapper;
pub mod zstd_wrapper;

pub use algorithm::{CompressionAlgorithm, CompressLevel};
pub use compress_choice::CompressionChoice;
pub use compress_header::{CompressionHeader, COMPRESSION_MAGIC};
pub use compress_stats::CompressionStats;
pub use compress_threshold::CompressionThreshold;
pub use compress_writer::CompressWriter;
pub use decompress_reader::DecompressReader;
pub use lz4_wrapper::Lz4Compressor;
pub use zstd_wrapper::ZstdCompressor;
