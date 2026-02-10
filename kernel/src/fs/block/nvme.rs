//! NVMe Optimizations
//!
//! Specialized optimizations for NVMe devices including queue depth tuning,
//! command prioritization, and parallel I/O.

use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::fs::FsResult;
use super::device::BlockDevice;
use super::scheduler::{IoRequest, IoOperation};

/// NVMe queue depth limits
pub const NVME_MIN_QUEUE_DEPTH: u32 = 2;
pub const NVME_MAX_QUEUE_DEPTH: u32 = 65536;
pub const NVME_DEFAULT_QUEUE_DEPTH: u32 = 1024;

/// NVMe command priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum NvmePriority {
    /// Urgent priority
    Urgent = 0,
    /// High priority
    High = 1,
    /// Medium priority
    Medium = 2,
    /// Low priority
    Low = 3,
}

impl From<u8> for NvmePriority {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::Urgent,
            1 => Self::High,
            2 => Self::Medium,
            _ => Self::Low,
        }
    }
}

/// NVMe command
#[derive(Debug, Clone)]
pub struct NvmeCommand {
    /// Command ID
    pub id: u64,
    /// Operation type
    pub operation: IoOperation,
    /// Starting LBA
    pub lba: u64,
    /// Number of blocks
    pub count: u32,
    /// Priority
    pub priority: NvmePriority,
    /// Queue ID
    pub queue_id: u16,
}

impl NvmeCommand {
    /// Create from I/O request
    pub fn from_io_request(request: &IoRequest, queue_id: u16) -> Self {
        Self {
            id: request.id,
            operation: request.operation,
            lba: request.lba,
            count: request.count,
            priority: NvmePriority::from(request.priority),
            queue_id,
        }
    }
}

/// NVMe optimizer - Manages queue depth and command prioritization
pub struct NvmeOptimizer {
    /// Current queue depth
    queue_depth: AtomicU32,
    /// Maximum queue depth
    max_queue_depth: u32,
    /// Number of I/O queues
    num_queues: u16,
    /// Commands in flight
    commands_in_flight: AtomicU32,
    /// Total commands processed
    total_commands: AtomicU64,
    /// Total read operations
    total_reads: AtomicU64,
    /// Total write operations
    total_writes: AtomicU64,
}

impl NvmeOptimizer {
    /// Create a new NVMe optimizer
    pub fn new(max_queue_depth: u32, num_queues: u16) -> Arc<RwLock<Self>> {
        let max_depth = max_queue_depth
            .min(NVME_MAX_QUEUE_DEPTH)
            .max(NVME_MIN_QUEUE_DEPTH);

        Arc::new(RwLock::new(Self {
            queue_depth: AtomicU32::new(NVME_DEFAULT_QUEUE_DEPTH.min(max_depth)),
            max_queue_depth: max_depth,
            num_queues,
            commands_in_flight: AtomicU32::new(0),
            total_commands: AtomicU64::new(0),
            total_reads: AtomicU64::new(0),
            total_writes: AtomicU64::new(0),
        }))
    }

    /// Get current queue depth
    pub fn queue_depth(&self) -> u32 {
        self.queue_depth.load(Ordering::Relaxed)
    }

    /// Set queue depth
    pub fn set_queue_depth(&self, depth: u32) -> FsResult<()> {
        let depth = depth
            .min(self.max_queue_depth)
            .max(NVME_MIN_QUEUE_DEPTH);

        self.queue_depth.store(depth, Ordering::Relaxed);
        Ok(())
    }

    /// Get number of I/O queues
    pub fn num_queues(&self) -> u16 {
        self.num_queues
    }

    /// Select optimal queue for a command
    pub fn select_queue(&self, command: &NvmeCommand) -> u16 {
        if self.num_queues <= 1 {
            return 0;
        }

        match command.priority {
            NvmePriority::Urgent | NvmePriority::High => 0,
            NvmePriority::Medium => (command.lba % self.num_queues as u64) as u16,
            NvmePriority::Low => {
                ((command.lba / 1024) % self.num_queues as u64) as u16
            }
        }
    }

    /// Check if can submit more commands
    pub fn can_submit(&self) -> bool {
        self.commands_in_flight.load(Ordering::Relaxed) < self.queue_depth()
    }

    /// Submit a command
    pub fn submit_command(&self, command: &NvmeCommand) -> FsResult<()> {
        if !self.can_submit() {
            return Err(crate::fs::FsError::Again);
        }

        self.commands_in_flight.fetch_add(1, Ordering::Relaxed);
        self.total_commands.fetch_add(1, Ordering::Relaxed);

        match command.operation {
            IoOperation::Read => {
                self.total_reads.fetch_add(1, Ordering::Relaxed);
            }
            IoOperation::Write => {
                self.total_writes.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }

        Ok(())
    }

    /// Complete a command
    pub fn complete_command(&self) {
        self.commands_in_flight.fetch_sub(1, Ordering::Relaxed);
    }

    /// Get commands in flight
    pub fn commands_in_flight(&self) -> u32 {
        self.commands_in_flight.load(Ordering::Relaxed)
    }

    /// Auto-tune queue depth based on latency
    pub fn auto_tune(&self, avg_latency_us: u64) {
        let current_depth = self.queue_depth();

        let new_depth = if avg_latency_us < 100 {
            (current_depth + (current_depth / 8)).min(self.max_queue_depth)
        } else if avg_latency_us > 500 {
            (current_depth - (current_depth / 8)).max(NVME_MIN_QUEUE_DEPTH)
        } else {
            current_depth
        };

        if new_depth != current_depth {
            self.queue_depth.store(new_depth, Ordering::Relaxed);
        }
    }

    /// Get statistics
    pub fn stats(&self) -> NvmeStats {
        NvmeStats {
            queue_depth: self.queue_depth(),
            max_queue_depth: self.max_queue_depth,
            num_queues: self.num_queues,
            commands_in_flight: self.commands_in_flight.load(Ordering::Relaxed),
            total_commands: self.total_commands.load(Ordering::Relaxed),
            total_reads: self.total_reads.load(Ordering::Relaxed),
            total_writes: self.total_writes.load(Ordering::Relaxed),
        }
    }

    /// Reset statistics
    pub fn reset_stats(&self) {
        self.total_commands.store(0, Ordering::Relaxed);
        self.total_reads.store(0, Ordering::Relaxed);
        self.total_writes.store(0, Ordering::Relaxed);
    }
}

/// NVMe statistics
#[derive(Debug, Clone, Copy)]
pub struct NvmeStats {
    /// Current queue depth
    pub queue_depth: u32,
    /// Maximum queue depth
    pub max_queue_depth: u32,
    /// Number of I/O queues
    pub num_queues: u16,
    /// Commands currently in flight
    pub commands_in_flight: u32,
    /// Total commands processed
    pub total_commands: u64,
    /// Total read operations
    pub total_reads: u64,
    /// Total write operations
    pub total_writes: u64,
}

/// NVMe device wrapper with optimizations
pub struct NvmeDevice {
    /// Underlying block device
    device: Arc<RwLock<dyn BlockDevice>>,
    /// Optimizer
    optimizer: Arc<RwLock<NvmeOptimizer>>,
}

impl NvmeDevice {
    /// Create a new NVMe device wrapper
    pub fn new(
        device: Arc<RwLock<dyn BlockDevice>>,
        max_queue_depth: u32,
        num_queues: u16,
    ) -> Arc<RwLock<Self>> {
        Arc::new(RwLock::new(Self {
            device,
            optimizer: NvmeOptimizer::new(max_queue_depth, num_queues),
        }))
    }

    /// Get optimizer
    pub fn optimizer(&self) -> Arc<RwLock<NvmeOptimizer>> {
        self.optimizer.clone()
    }

    /// Submit I/O request with NVMe optimizations
    pub fn submit_io(&self, request: &IoRequest) -> FsResult<()> {
        let optimizer = self.optimizer.read();

        if !optimizer.can_submit() {
            return Err(crate::fs::FsError::Again);
        }

        let queue_id = optimizer.select_queue(&NvmeCommand::from_io_request(request, 0));
        let command = NvmeCommand::from_io_request(request, queue_id);

        optimizer.submit_command(&command)?;

        Ok(())
    }

    /// Complete I/O request
    pub fn complete_io(&self) {
        let optimizer = self.optimizer.read();
        optimizer.complete_command();
    }

    /// Get statistics
    pub fn stats(&self) -> NvmeStats {
        self.optimizer.read().stats()
    }

    /// Auto-tune based on performance
    pub fn auto_tune(&self, avg_latency_us: u64) {
        self.optimizer.read().auto_tune(avg_latency_us);
    }
}

/// Parallel I/O manager for NVMe
pub struct ParallelIoManager {
    /// Number of parallel streams
    num_streams: u16,
    /// Stream assignment counter
    next_stream: AtomicU32,
}

impl ParallelIoManager {
    /// Create a new parallel I/O manager
    pub fn new(num_streams: u16) -> Arc<Self> {
        Arc::new(Self {
            num_streams: num_streams.max(1),
            next_stream: AtomicU32::new(0),
        })
    }

    /// Get stream ID for a request
    pub fn get_stream(&self, lba: u64) -> u16 {
        (lba % self.num_streams as u64) as u16
    }

    /// Get next stream in round-robin fashion
    pub fn next_stream(&self) -> u16 {
        let stream = self.next_stream.fetch_add(1, Ordering::Relaxed);
        (stream % self.num_streams as u32) as u16
    }

    /// Number of streams
    pub fn num_streams(&self) -> u16 {
        self.num_streams
    }
}
