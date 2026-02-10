//! I/O Engine - Ultra-fast I/O subsystem
//!
//! ## Modules
//! - `uring`: io_uring async I/O framework
//! - `zero_copy`: Zero-copy DMA transfers
//! - `aio`: POSIX AIO compatibility
//! - `mmap`: Memory-mapped I/O
//! - `direct_io`: Direct I/O (O_DIRECT)
//! - `completion`: I/O completion queues
//!
//! ## Performance Targets
//! - Latency: < 1µs (polling mode)
//! - Throughput: > 2M IOPS per core
//! - CPU efficiency: < 10% overhead

pub mod uring;
pub mod zero_copy;
pub mod aio;
pub mod mmap;
pub mod direct_io;
pub mod completion;

use crate::fs::FsResult;

/// Initialize I/O subsystem
pub fn init() {
    log::info!("Initializing I/O engine subsystem");

    // Initialize all I/O components
    uring::init();
    zero_copy::init();
    aio::init();
    mmap::init();
    direct_io::init();
    completion::init();

    log::info!("✓ I/O engine initialized");
}

/// Get I/O engine statistics
pub fn get_stats() -> IoEngineStats {
    IoEngineStats {
        uring_submitted: uring::global_uring().stats().submitted.load(core::sync::atomic::Ordering::Relaxed),
        uring_completed: uring::global_uring().stats().completed.load(core::sync::atomic::Ordering::Relaxed),
        aio_submitted: aio::global_aio_context().stats().submitted.load(core::sync::atomic::Ordering::Relaxed),
        aio_completed: aio::global_aio_context().stats().completed.load(core::sync::atomic::Ordering::Relaxed),
        direct_reads: direct_io::global_direct_io().stats().reads.load(core::sync::atomic::Ordering::Relaxed),
        direct_writes: direct_io::global_direct_io().stats().writes.load(core::sync::atomic::Ordering::Relaxed),
        completions_posted: {
            let dispatcher = completion::global_dispatcher();
            dispatcher.total_stats().posted.load(core::sync::atomic::Ordering::Relaxed)
        },
    }
}

#[derive(Debug, Clone)]
pub struct IoEngineStats {
    pub uring_submitted: u64,
    pub uring_completed: u64,
    pub aio_submitted: u64,
    pub aio_completed: u64,
    pub direct_reads: u64,
    pub direct_writes: u64,
    pub completions_posted: u64,
}
