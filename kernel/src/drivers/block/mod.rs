//! Block Device Layer

pub mod ahci;
pub mod nvme;
pub mod ramdisk;
pub mod virtio_blk;  // ✅ Phase 3: VirtIO-Block enabled
pub mod partition;   // ✅ Phase 3: Partition table support

use crate::drivers::{DriverError, DriverResult};
use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::sync::Mutex;
use partition::Partition;

/// Block device trait
pub trait BlockDevice: Send + Sync {
    /// Read sectors from the device
    fn read(&mut self, sector: u64, buffer: &mut [u8]) -> DriverResult<usize>;
    
    /// Write sectors to the device
    fn write(&mut self, sector: u64, data: &[u8]) -> DriverResult<usize>;
    
    /// Get sector size in bytes
    fn sector_size(&self) -> usize;
    
    /// Get total number of sectors
    fn total_sectors(&self) -> u64;
    
    /// Flush any cached writes
    fn flush(&mut self) -> DriverResult<()> {
        Ok(())
    }
    
    /// Get device name
    fn name(&self) -> &str {
        "unknown"
    }
    
    /// Get device capacity in bytes
    fn capacity(&self) -> u64 {
        self.total_sectors() * self.sector_size() as u64
    }
}

/// Block request operation type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockOp {
    Read,
    Write,
    Flush,
}

/// Global block device registry
static BLOCK_DEVICES: Mutex<Vec<Arc<Mutex<dyn BlockDevice>>>> = Mutex::new(Vec::new());

/// Register a block device
pub fn register_block_device(device: Arc<Mutex<dyn BlockDevice>>) {
    let mut devices = BLOCK_DEVICES.lock();
    devices.push(device);
    log::info!("Block device registered (total: {})", devices.len());
}

/// Get all registered block devices
pub fn get_block_devices() -> Vec<Arc<Mutex<dyn BlockDevice>>> {
    BLOCK_DEVICES.lock().clone()
}

/// Get block device by name
pub fn get_block_device(name: &str) -> Option<Arc<Mutex<dyn BlockDevice>>> {
    let devices = BLOCK_DEVICES.lock();
    devices.iter()
        .find(|dev| {
            let guard = dev.lock();
            guard.name() == name
        })
        .cloned()
}

/// Enumerate all partitions on all block devices
pub fn enumerate_all_partitions() -> Vec<(Arc<Mutex<dyn BlockDevice>>, Vec<Partition>)> {
    let devices = get_block_devices();
    let mut result = Vec::new();
    
    for device in devices {
        let mut dev_guard = device.lock();
        if let Ok(partitions) = partition::parse_partitions(&mut *dev_guard) {
            drop(dev_guard);
            result.push((device.clone(), partitions));
        }
    }
    
    result
}

/// Initialize block subsystem
pub fn init() {
    log::info!("Block subsystem initialized");
    
    // Try to initialize VirtIO-Blk
    // ⏸️ Phase 2: if virtio_blk::init() {
    // ⏸️ Phase 2:     log::info!("  VirtIO-Blk driver loaded");
    // ⏸️ Phase 2: }
}
