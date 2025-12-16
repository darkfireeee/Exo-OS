//! Block Device Layer

pub mod ahci;
pub mod nvme;
pub mod ramdisk;
// pub mod virtio_blk;  // ⏸️ Phase 2: Requires crate::drivers::virtio

use crate::drivers::{DriverError, DriverResult};

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
}

/// Block request operation type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockOp {
    Read,
    Write,
    Flush,
}

/// Initialize block subsystem
pub fn init() {
    log::info!("Block subsystem initialized");
    
    // Try to initialize VirtIO-Blk
    // ⏸️ Phase 2: if virtio_blk::init() {
    // ⏸️ Phase 2:     log::info!("  VirtIO-Blk driver loaded");
    // ⏸️ Phase 2: }
}
