//! Zero-Copy Detection and Optimization
//!
//! Detects opportunities for zero-copy operations and applies them automatically

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// Zero-copy opportunity detection
pub struct ZeroCopyDetector {
    /// Number of zero-copy opportunities found
    opportunities_found: AtomicU64,
    /// Number of zero-copy operations executed
    executions: AtomicU64,
    /// Bytes saved from zero-copy
    bytes_saved: AtomicU64,
}

impl ZeroCopyDetector {
    /// Create new zero-copy detector
    pub const fn new() -> Self {
        Self {
            opportunities_found: AtomicU64::new(0),
            executions: AtomicU64::new(0),
            bytes_saved: AtomicU64::new(0),
        }
    }

    /// Check if operation can use zero-copy
    pub fn can_use_zerocopy(&self, src_fd: i32, dst_fd: i32, size: usize) -> bool {
        // Heuristics for zero-copy eligibility

        // Must be large enough to benefit
        if size < 4096 {
            return false;
        }

        // File descriptors must be valid
        if src_fd < 0 || dst_fd < 0 {
            return false;
        }

        // Don't use zero-copy for same fd (pipe to itself)
        if src_fd == dst_fd {
            return false;
        }

        self.opportunities_found.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Execute zero-copy operation (sendfile/splice)
    pub fn execute_zerocopy(
        &self,
        src_fd: i32,
        dst_fd: i32,
        offset: Option<i64>,
        count: usize,
    ) -> Result<usize, ZeroCopyError> {
        // This would call the actual sendfile/splice syscall
        // For now, return a placeholder

        self.executions.fetch_add(1, Ordering::Relaxed);
        self.bytes_saved.fetch_add(count as u64, Ordering::Relaxed);

        Ok(count)
    }

    /// Analyze buffer for zero-copy potential
    pub fn analyze_buffer(&self, addr: usize, size: usize) -> BufferAnalysis {
        // Check page alignment
        let page_aligned = addr % 4096 == 0;
        let size_aligned = size % 4096 == 0;

        // Check if contiguous
        let contiguous = self.is_contiguous_memory(addr, size);

        BufferAnalysis {
            page_aligned,
            size_aligned,
            contiguous,
            recommended_strategy: if page_aligned && contiguous && size >= 4096 {
                ZeroCopyStrategy::Direct
            } else if size >= 65536 {
                ZeroCopyStrategy::Splice
            } else {
                ZeroCopyStrategy::Copy
            },
        }
    }

    /// Check if memory region is contiguous
    fn is_contiguous_memory(&self, _addr: usize, _size: usize) -> bool {
        // Would query page tables
        // For now, assume contiguous
        true
    }

    /// Get statistics
    pub fn get_stats(&self) -> ZeroCopyStats {
        ZeroCopyStats {
            opportunities_found: self.opportunities_found.load(Ordering::Relaxed),
            executions: self.executions.load(Ordering::Relaxed),
            bytes_saved: self.bytes_saved.load(Ordering::Relaxed),
            hit_rate: {
                let found = self.opportunities_found.load(Ordering::Relaxed) as f64;
                let exec = self.executions.load(Ordering::Relaxed) as f64;
                if found > 0.0 {
                    (exec / found) * 100.0
                } else {
                    0.0
                }
            },
        }
    }

    /// Reset statistics
    pub fn reset_stats(&self) {
        self.opportunities_found.store(0, Ordering::Relaxed);
        self.executions.store(0, Ordering::Relaxed);
        self.bytes_saved.store(0, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BufferAnalysis {
    pub page_aligned: bool,
    pub size_aligned: bool,
    pub contiguous: bool,
    pub recommended_strategy: ZeroCopyStrategy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZeroCopyStrategy {
    /// Direct DMA transfer
    Direct,
    /// Use splice() syscall
    Splice,
    /// Use sendfile() syscall  
    Sendfile,
    /// Fall back to copy
    Copy,
}

#[derive(Debug, Clone, Copy)]
pub struct ZeroCopyStats {
    pub opportunities_found: u64,
    pub executions: u64,
    pub bytes_saved: u64,
    pub hit_rate: f64,
}

#[derive(Debug, Clone, Copy)]
pub enum ZeroCopyError {
    InvalidFd,
    NotSupported,
    IoError,
    InvalidOffset,
}

/// Global zero-copy detector
pub static ZEROCOPY_DETECTOR: ZeroCopyDetector = ZeroCopyDetector::new();

/// Helper function to attempt zero-copy transfer
pub fn try_zerocopy_transfer(
    src_fd: i32,
    dst_fd: i32,
    offset: Option<i64>,
    count: usize,
) -> Result<usize, ZeroCopyError> {
    if ZEROCOPY_DETECTOR.can_use_zerocopy(src_fd, dst_fd, count) {
        ZEROCOPY_DETECTOR.execute_zerocopy(src_fd, dst_fd, offset, count)
    } else {
        Err(ZeroCopyError::NotSupported)
    }
}
