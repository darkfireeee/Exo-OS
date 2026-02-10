//! Compression Support
//!
//! Transparent file compression with multiple algorithms:
//! - LZ4 (fast compression/decompression)
//! - Zstandard (high compression ratio)
//! - DEFLATE (compatibility)

use crate::fs::{FsError, FsResult};
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Compression algorithms
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionAlgorithm {
    None = 0,
    LZ4 = 1,
    Zstandard = 2,
    Deflate = 3,
}

impl CompressionAlgorithm {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(CompressionAlgorithm::None),
            1 => Some(CompressionAlgorithm::LZ4),
            2 => Some(CompressionAlgorithm::Zstandard),
            3 => Some(CompressionAlgorithm::Deflate),
            _ => None,
        }
    }
}

/// Compressed block metadata
#[derive(Debug, Clone)]
struct CompressedBlock {
    /// Compressed size
    compressed_size: u32,
    /// Uncompressed size
    uncompressed_size: u32,
    /// Algorithm used
    algorithm: CompressionAlgorithm,
}

/// Compression Manager
pub struct CompressionManager {
    /// Compression settings per inode
    inode_settings: Mutex<BTreeMap<u64, CompressionAlgorithm>>,
    /// Compressed block metadata
    block_metadata: Mutex<BTreeMap<u64, CompressedBlock>>,
    /// Statistics
    stats: CompressionStats,
    /// Default algorithm
    default_algorithm: CompressionAlgorithm,
}

impl CompressionManager {
    /// Create new compression manager
    pub fn new() -> Self {
        Self {
            inode_settings: Mutex::new(BTreeMap::new()),
            block_metadata: Mutex::new(BTreeMap::new()),
            stats: CompressionStats::new(),
            default_algorithm: CompressionAlgorithm::LZ4,
        }
    }

    /// Enable compression for inode
    pub fn enable(&self, inode: u64, algorithm: CompressionAlgorithm) {
        let mut settings = self.inode_settings.lock();
        settings.insert(inode, algorithm);
        log::debug!("ext4plus: Enabled {:?} compression for inode {}", algorithm, inode);
    }

    /// Disable compression for inode
    pub fn disable(&self, inode: u64) {
        let mut settings = self.inode_settings.lock();
        settings.remove(&inode);
        log::debug!("ext4plus: Disabled compression for inode {}", inode);
    }

    /// Check if compression is enabled for inode
    pub fn is_enabled(&self, inode: u64) -> bool {
        let settings = self.inode_settings.lock();
        settings.contains_key(&inode)
    }

    /// Get algorithm for inode
    pub fn get_algorithm(&self, inode: u64) -> CompressionAlgorithm {
        let settings = self.inode_settings.lock();
        settings.get(&inode).cloned().unwrap_or(self.default_algorithm)
    }

    /// Compress data
    pub fn compress(&self, inode: u64, data: &[u8]) -> FsResult<Vec<u8>> {
        let algorithm = self.get_algorithm(inode);

        let compressed = match algorithm {
            CompressionAlgorithm::None => data.to_vec(),
            CompressionAlgorithm::LZ4 => self.lz4_compress(data)?,
            CompressionAlgorithm::Zstandard => self.zstd_compress(data)?,
            CompressionAlgorithm::Deflate => self.deflate_compress(data)?,
        };

        // Update statistics
        self.stats.bytes_uncompressed.fetch_add(data.len() as u64, Ordering::Relaxed);
        self.stats.bytes_compressed.fetch_add(compressed.len() as u64, Ordering::Relaxed);
        self.stats.compressions.fetch_add(1, Ordering::Relaxed);

        log::trace!("ext4plus: Compressed {} -> {} bytes ({:.1}%)",
            data.len(), compressed.len(),
            (compressed.len() as f64 / data.len() as f64) * 100.0
        );

        Ok(compressed)
    }

    /// Decompress data
    pub fn decompress(&self, block: u64, data: &[u8]) -> FsResult<Vec<u8>> {
        let metadata = {
            let block_metadata = self.block_metadata.lock();
            block_metadata.get(&block).cloned()
        };

        let decompressed = if let Some(meta) = metadata {
            match meta.algorithm {
                CompressionAlgorithm::None => data.to_vec(),
                CompressionAlgorithm::LZ4 => self.lz4_decompress(data, meta.uncompressed_size)?,
                CompressionAlgorithm::Zstandard => self.zstd_decompress(data)?,
                CompressionAlgorithm::Deflate => self.deflate_decompress(data)?,
            }
        } else {
            data.to_vec()
        };

        self.stats.decompressions.fetch_add(1, Ordering::Relaxed);

        Ok(decompressed)
    }

    /// LZ4 compression (simplified)
    fn lz4_compress(&self, data: &[u8]) -> FsResult<Vec<u8>> {
        // In production, would use actual LZ4 implementation
        // For now, return copy (no compression)
        Ok(data.to_vec())
    }

    /// LZ4 decompression (simplified)
    fn lz4_decompress(&self, data: &[u8], _uncompressed_size: u32) -> FsResult<Vec<u8>> {
        // In production, would use actual LZ4 implementation
        Ok(data.to_vec())
    }

    /// Zstandard compression (simplified)
    fn zstd_compress(&self, data: &[u8]) -> FsResult<Vec<u8>> {
        // In production, would use actual Zstandard implementation
        Ok(data.to_vec())
    }

    /// Zstandard decompression (simplified)
    fn zstd_decompress(&self, data: &[u8]) -> FsResult<Vec<u8>> {
        // In production, would use actual Zstandard implementation
        Ok(data.to_vec())
    }

    /// DEFLATE compression (simplified)
    fn deflate_compress(&self, data: &[u8]) -> FsResult<Vec<u8>> {
        // In production, would use actual DEFLATE implementation
        Ok(data.to_vec())
    }

    /// DEFLATE decompression (simplified)
    fn deflate_decompress(&self, data: &[u8]) -> FsResult<Vec<u8>> {
        // In production, would use actual DEFLATE implementation
        Ok(data.to_vec())
    }

    /// Get compression ratio
    pub fn compression_ratio(&self) -> f64 {
        let uncompressed = self.stats.bytes_uncompressed.load(Ordering::Relaxed);
        let compressed = self.stats.bytes_compressed.load(Ordering::Relaxed);

        if uncompressed == 0 {
            1.0
        } else {
            compressed as f64 / uncompressed as f64
        }
    }

    /// Get statistics
    pub fn stats(&self) -> CompressionStatsSnapshot {
        CompressionStatsSnapshot {
            compressions: self.stats.compressions.load(Ordering::Relaxed),
            decompressions: self.stats.decompressions.load(Ordering::Relaxed),
            bytes_uncompressed: self.stats.bytes_uncompressed.load(Ordering::Relaxed),
            bytes_compressed: self.stats.bytes_compressed.load(Ordering::Relaxed),
            compression_ratio: self.compression_ratio(),
        }
    }
}

/// Compression statistics
struct CompressionStats {
    compressions: AtomicU64,
    decompressions: AtomicU64,
    bytes_uncompressed: AtomicU64,
    bytes_compressed: AtomicU64,
}

impl CompressionStats {
    fn new() -> Self {
        Self {
            compressions: AtomicU64::new(0),
            decompressions: AtomicU64::new(0),
            bytes_uncompressed: AtomicU64::new(0),
            bytes_compressed: AtomicU64::new(0),
        }
    }
}

/// Statistics snapshot
#[derive(Debug, Clone, Copy)]
pub struct CompressionStatsSnapshot {
    pub compressions: u64,
    pub decompressions: u64,
    pub bytes_uncompressed: u64,
    pub bytes_compressed: u64,
    pub compression_ratio: f64,
}
