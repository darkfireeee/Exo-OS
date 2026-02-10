//! Block Device Layer
//!
//! Comprehensive block device abstraction with advanced features:
//! - Unified BlockDevice trait for all storage types
//! - Automatic partition detection (MBR/GPT)
//! - Multiple I/O schedulers (Deadline, CFQ, Noop)
//! - NVMe optimizations (queue depth tuning, command prioritization)
//! - Detailed I/O statistics and latency tracking
//! - Software RAID support (levels 0, 1, 5, 6, 10)
//!
//! # Performance Philosophy
//! - Zero-copy I/O via slice-based operations
//! - Lock-free statistics using atomics
//! - Minimal overhead for high-performance devices (NVMe, SSD)
//! - Adaptive scheduling based on device characteristics
//!
//! # Usage Example
//! ```no_run
//! use exo_kernel::fs::block::*;
//!
//! // Create a RAM disk
//! let device = device::RamDisk::new("ramdisk0".into(), 1024 * 1024, 512);
//!
//! // Detect partitions
//! let partition_table = partition::PartitionTable::detect(&*device.read())?;
//!
//! // Wrap with I/O scheduler
//! let scheduled = scheduler::ScheduledDevice::new(
//!     device.clone(),
//!     scheduler::SchedulerType::Deadline,
//! );
//!
//! // Add statistics tracking
//! let stats = stats::IoStats::new();
//! ```

pub mod device;
pub mod partition;
pub mod scheduler;
pub mod nvme;
pub mod stats;
pub mod raid;

#[cfg(test)]
pub mod examples;

#[cfg(test)]
pub mod quickstart;

pub use device::{
    BlockDevice,
    BlockDeviceInfo,
    DeviceType,
    RamDisk,
    BlockDeviceRegistry,
    AsyncRead,
    AsyncWrite,
};

pub use partition::{
    Partition,
    PartitionType,
    PartitionFlags,
    FileSystemType,
    PartitionTable,
    PartitionTableType,
    PartitionedDevice,
};

pub use scheduler::{
    IoRequest,
    IoOperation,
    IoScheduler,
    SchedulerType,
    DeadlineScheduler,
    CFQScheduler,
    NoopScheduler,
    ScheduledDevice,
};

pub use nvme::{
    NvmePriority,
    NvmeCommand,
    NvmeOptimizer,
    NvmeStats,
    NvmeDevice,
    ParallelIoManager,
    NVME_MIN_QUEUE_DEPTH,
    NVME_MAX_QUEUE_DEPTH,
    NVME_DEFAULT_QUEUE_DEPTH,
};

pub use stats::{
    IoStats,
    IoStatsSnapshot,
    LatencyHistogram,
};

pub use raid::{
    RaidLevel,
    RaidConfig,
    RaidArray,
    RaidStats,
};

use alloc::sync::Arc;
use spin::RwLock;
use lazy_static::lazy_static;

lazy_static! {
    /// Global block device registry
    pub static ref BLOCK_DEVICE_REGISTRY: BlockDeviceRegistry = BlockDeviceRegistry::new();
}

/// Initialize block device subsystem
pub fn init() {
    log::info!("Block device subsystem initialized");
    log::info!("  Supported schedulers: Deadline, CFQ, Noop");
    log::info!("  NVMe optimizations: Queue depth tuning, parallel I/O");
    log::info!("  RAID levels: 0, 1, 5, 6, 10");
    log::info!("  Partition tables: MBR, GPT");
}

/// Create a test RAM disk and register it
pub fn create_test_ramdisk(name: &str, size_mb: u64) -> Arc<RwLock<RamDisk>> {
    let size = size_mb * 1024 * 1024;
    let device = RamDisk::new(name.into(), size, 512);
    BLOCK_DEVICE_REGISTRY.register(device.clone());
    device
}

/// Get device by name from registry
pub fn get_device(name: &str) -> Option<Arc<RwLock<dyn BlockDevice>>> {
    BLOCK_DEVICE_REGISTRY.get(name)
}

/// List all registered devices
pub fn list_devices() -> alloc::vec::Vec<BlockDeviceInfo> {
    BLOCK_DEVICE_REGISTRY.list()
}

/// Get total number of devices
pub fn device_count() -> usize {
    BLOCK_DEVICE_REGISTRY.count()
}
