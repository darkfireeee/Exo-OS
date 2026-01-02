//! VirtIO Block Driver
//!
//! Virtual block device driver for QEMU/KVM environments.
//! Provides disk I/O operations through VirtIO interface.
//!
//! ## Features
//! - Read/write sectors
//! - Request queuing
//! - Multi-queue support (future)
//! - Flush support
//!
//! ## Architecture
//! ```text
//! Filesystem Layer
//!       ↓
//! Block Cache
//!       ↓
//! VirtIO-Block Driver
//!       ↓
//! VirtQueue (I/O requests)
//!       ↓
//! QEMU Block Backend
//! ```

use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::sync::Arc;
use spin::Mutex;
use crate::drivers::virtio::{VirtioPciDevice, VirtQueue, DeviceType, status, features};
use crate::drivers::virtio::desc_flags;
use crate::drivers::pci::PciDevice;
use crate::memory::{PhysicalAddress, VirtualAddress};

/// VirtIO-Block feature bits
pub mod blk_features {
    pub const SIZE_MAX: u64 = 1 << 1;
    pub const SEG_MAX: u64 = 1 << 2;
    pub const GEOMETRY: u64 = 1 << 4;
    pub const RO: u64 = 1 << 5;
    pub const BLK_SIZE: u64 = 1 << 6;
    pub const FLUSH: u64 = 1 << 9;
    pub const TOPOLOGY: u64 = 1 << 10;
    pub const CONFIG_WCE: u64 = 1 << 11;
    pub const MQ: u64 = 1 << 12;
}

/// VirtIO-Block request types
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlkReqType {
    In = 0,             // Read
    Out = 1,            // Write
    Flush = 4,          // Flush
    GetId = 8,          // Get device ID
    GetLifetime = 10,   // Get lifetime hint
    Discard = 11,       // Discard/TRIM
    WriteZeroes = 13,   // Write zeros
}

/// VirtIO-Block request status
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlkStatus {
    Ok = 0,
    IoErr = 1,
    Unsupported = 2,
}

/// VirtIO-Block request header
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BlkReq {
    pub req_type: u32,
    pub reserved: u32,
    pub sector: u64,
}

impl BlkReq {
    pub fn new_read(sector: u64) -> Self {
        Self {
            req_type: BlkReqType::In as u32,
            reserved: 0,
            sector,
        }
    }
    
    pub fn new_write(sector: u64) -> Self {
        Self {
            req_type: BlkReqType::Out as u32,
            reserved: 0,
            sector,
        }
    }
    
    pub fn new_flush() -> Self {
        Self {
            req_type: BlkReqType::Flush as u32,
            reserved: 0,
            sector: 0,
        }
    }
}

/// VirtIO-Block configuration space
#[repr(C, packed)]
pub struct BlkConfig {
    pub capacity: u64,
    pub size_max: u32,
    pub seg_max: u32,
    pub cylinders: u16,
    pub heads: u8,
    pub sectors: u8,
    pub blk_size: u32,
    pub physical_block_exp: u8,
    pub alignment_offset: u8,
    pub min_io_size: u16,
    pub opt_io_size: u32,
    pub writeback: u8,
    pub unused: u8,
    pub num_queues: u16,
    pub max_discard_sectors: u32,
    pub max_discard_seg: u32,
    pub discard_sector_alignment: u32,
    pub max_write_zeroes_sectors: u32,
    pub max_write_zeroes_seg: u32,
    pub write_zeroes_may_unmap: u8,
    pub unused1: [u8; 3],
}

/// Pending I/O request
struct PendingRequest {
    /// Request header buffer
    header: DmaBuffer,
    
    /// Data buffer
    data: DmaBuffer,
    
    /// Status buffer
    status: DmaBuffer,
    
    /// Callback when complete
    callback: Option<Box<dyn FnOnce(Result<(), BlkStatus>) + Send>>,
}

/// DMA buffer for I/O
struct DmaBuffer {
    virt: VirtAddr,
    phys: PhysAddr,
    size: usize,
}

impl DmaBuffer {
    fn new(size: usize) -> Result<Self, &'static str> {
        let layout = core::alloc::Layout::from_size_align(size, 512)
            .map_err(|_| "Invalid layout")?;
        
        let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err("Failed to allocate DMA buffer");
        }
        
        let virt = VirtAddr::new(ptr as u64);
        let phys = PhysAddr::new(virt.as_u64()); // TODO: Real phys addr
        
        Ok(Self { virt, phys, size })
    }
}

impl Drop for DmaBuffer {
    fn drop(&mut self) {
        unsafe {
            let layout = core::alloc::Layout::from_size_align_unchecked(self.size, 512);
            alloc::alloc::dealloc(self.virt.as_u64() as *mut u8, layout);
        }
    }
}

/// VirtIO-Block Driver
pub struct VirtioBlock {
    /// Base VirtIO device
    device: VirtioPciDevice,
    
    /// Block queue
    queue: VirtQueue,
    
    /// Device capacity (sectors)
    capacity: u64,
    
    /// Block size (bytes)
    block_size: u32,
    
    /// Pending requests
    pending: Vec<Option<PendingRequest>>,
    
    /// Statistics
    stats: BlockStats,
}

/// Block device statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct BlockStats {
    pub read_requests: u64,
    pub write_requests: u64,
    pub read_sectors: u64,
    pub write_sectors: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub errors: u64,
}

impl VirtioBlock {
    /// Sector size (512 bytes)
    pub const SECTOR_SIZE: usize = 512;
    
    /// Create from PCI device
    pub fn new(pci_dev: PciDevice) -> Result<Arc<Mutex<Self>>, &'static str> {
        let mut device = VirtioPciDevice::from_pci(pci_dev)?;
        
        if device.device_type != DeviceType::Block {
            return Err("Not a block device");
        }
        
        // Initialize device
        device.init()?;
        
        // Read device features
        let dev_features = device.device_features;
        
        // Negotiate features
        let mut driver_features = features::VERSION_1;
        
        if (dev_features & blk_features::FLUSH) != 0 {
            driver_features |= blk_features::FLUSH;
        }
        
        if (dev_features & blk_features::BLK_SIZE) != 0 {
            driver_features |= blk_features::BLK_SIZE;
        }
        
        device.write_driver_features(driver_features);
        
        // Read capacity (8 bytes at offset 0)
        let capacity = device.read_u32(0) as u64 | ((device.read_u32(4) as u64) << 32);
        
        // Read block size (or default to 512)
        let block_size = if (driver_features & blk_features::BLK_SIZE) != 0 {
            device.read_u32(20)
        } else {
            512
        };
        
        crate::logger::info(&alloc::format!(
            "[VirtIO-Block] Capacity: {} sectors ({} MB), block size: {} bytes",
            capacity,
            (capacity * 512) / (1024 * 1024),
            block_size
        ));
        
        // Create virtqueue
        let queue = VirtQueue::new(128)?;
        
        // Initialize pending requests
        let pending = vec![None; 128];
        
        let mut blk = Self {
            device,
            queue,
            capacity,
            block_size,
            pending,
            stats: BlockStats::default(),
        };
        
        // Finalize device
        blk.device.finalize();
        
        Ok(Arc::new(Mutex::new(blk)))
    }
    
    /// Read sectors
    pub fn read_sectors(&mut self, sector: u64, count: u64, buffer: &mut [u8]) -> Result<(), &'static str> {
        if sector + count > self.capacity {
            return Err("Read beyond device capacity");
        }
        
        let bytes = (count * Self::SECTOR_SIZE as u64) as usize;
        if buffer.len() < bytes {
            return Err("Buffer too small");
        }
        
        // For simplicity, read one sector at a time (can be optimized)
        for i in 0..count {
            self.read_sector_sync(sector + i, &mut buffer[(i as usize * Self::SECTOR_SIZE)..][..Self::SECTOR_SIZE])?;
        }
        
        self.stats.read_requests += 1;
        self.stats.read_sectors += count;
        self.stats.read_bytes += bytes as u64;
        
        Ok(())
    }
    
    /// Write sectors
    pub fn write_sectors(&mut self, sector: u64, count: u64, buffer: &[u8]) -> Result<(), &'static str> {
        if sector + count > self.capacity {
            return Err("Write beyond device capacity");
        }
        
        let bytes = (count * Self::SECTOR_SIZE as u64) as usize;
        if buffer.len() < bytes {
            return Err("Buffer too small");
        }
        
        // For simplicity, write one sector at a time (can be optimized)
        for i in 0..count {
            self.write_sector_sync(sector + i, &buffer[(i as usize * Self::SECTOR_SIZE)..][..Self::SECTOR_SIZE])?;
        }
        
        self.stats.write_requests += 1;
        self.stats.write_sectors += count;
        self.stats.write_bytes += bytes as u64;
        
        Ok(())
    }
    
    /// Read single sector synchronously
    fn read_sector_sync(&mut self, sector: u64, buffer: &mut [u8]) -> Result<(), &'static str> {
        if buffer.len() < Self::SECTOR_SIZE {
            return Err("Buffer too small");
        }
        
        // Allocate buffers
        let header_buf = DmaBuffer::new(core::mem::size_of::<BlkReq>())?;
        let data_buf = DmaBuffer::new(Self::SECTOR_SIZE)?;
        let status_buf = DmaBuffer::new(1)?;
        
        // Write request header
        unsafe {
            let req = BlkReq::new_read(sector);
            core::ptr::write(header_buf.virt.as_u64() as *mut BlkReq, req);
        }
        
        // Allocate 3-descriptor chain: header (read), data (write), status (write)
        let desc_idx = self.queue.alloc_desc_chain(3)?;
        
        unsafe {
            let desc_base = self.queue.desc.as_u64() as *mut crate::drivers::virtio::VirtqDesc;
            
            // Descriptor 0: Request header (device reads)
            let desc0 = &mut *desc_base.add(desc_idx as usize);
            desc0.addr = header_buf.phys.as_u64();
            desc0.len = core::mem::size_of::<BlkReq>() as u32;
            desc0.flags = desc_flags::NEXT;
            desc0.next = desc_idx + 1;
            
            // Descriptor 1: Data buffer (device writes)
            let desc1 = &mut *desc_base.add((desc_idx + 1) as usize);
            desc1.addr = data_buf.phys.as_u64();
            desc1.len = Self::SECTOR_SIZE as u32;
            desc1.flags = desc_flags::WRITE | desc_flags::NEXT;
            desc1.next = desc_idx + 2;
            
            // Descriptor 2: Status byte (device writes)
            let desc2 = &mut *desc_base.add((desc_idx + 2) as usize);
            desc2.addr = status_buf.phys.as_u64();
            desc2.len = 1;
            desc2.flags = desc_flags::WRITE;
            desc2.next = 0;
        }
        
        // Add to available ring
        self.queue.add_buffer(desc_idx);
        
        // Notify device
        self.device.write_u16(16, 0); // Queue notify
        
        // Wait for completion (polling - can be improved with interrupts)
        loop {
            if let Some((used_id, _len)) = self.queue.get_used() {
                if used_id == desc_idx as u32 {
                    break;
                }
            }
        }
        
        // Check status
        let status = unsafe {
            let status_ptr = status_buf.virt.as_u64() as *const u8;
            *status_ptr
        };
        
        if status != BlkStatus::Ok as u8 {
            self.queue.free_desc_chain(desc_idx);
            self.stats.errors += 1;
            return Err("Block read failed");
        }
        
        // Copy data
        unsafe {
            core::ptr::copy_nonoverlapping(
                data_buf.virt.as_u64() as *const u8,
                buffer.as_mut_ptr(),
                Self::SECTOR_SIZE,
            );
        }
        
        // Free descriptors
        self.queue.free_desc_chain(desc_idx);
        
        Ok(())
    }
    
    /// Write single sector synchronously
    fn write_sector_sync(&mut self, sector: u64, buffer: &[u8]) -> Result<(), &'static str> {
        if buffer.len() < Self::SECTOR_SIZE {
            return Err("Buffer too small");
        }
        
        // Allocate buffers
        let header_buf = DmaBuffer::new(core::mem::size_of::<BlkReq>())?;
        let data_buf = DmaBuffer::new(Self::SECTOR_SIZE)?;
        let status_buf = DmaBuffer::new(1)?;
        
        // Write request header
        unsafe {
            let req = BlkReq::new_write(sector);
            core::ptr::write(header_buf.virt.as_u64() as *mut BlkReq, req);
        }
        
        // Copy data to DMA buffer
        unsafe {
            core::ptr::copy_nonoverlapping(
                buffer.as_ptr(),
                data_buf.virt.as_u64() as *mut u8,
                Self::SECTOR_SIZE,
            );
        }
        
        // Allocate 3-descriptor chain
        let desc_idx = self.queue.alloc_desc_chain(3)?;
        
        unsafe {
            let desc_base = self.queue.desc.as_u64() as *mut crate::drivers::virtio::VirtqDesc;
            
            // Descriptor 0: Request header (device reads)
            let desc0 = &mut *desc_base.add(desc_idx as usize);
            desc0.addr = header_buf.phys.as_u64();
            desc0.len = core::mem::size_of::<BlkReq>() as u32;
            desc0.flags = desc_flags::NEXT;
            desc0.next = desc_idx + 1;
            
            // Descriptor 1: Data buffer (device reads)
            let desc1 = &mut *desc_base.add((desc_idx + 1) as usize);
            desc1.addr = data_buf.phys.as_u64();
            desc1.len = Self::SECTOR_SIZE as u32;
            desc1.flags = desc_flags::NEXT;
            desc1.next = desc_idx + 2;
            
            // Descriptor 2: Status byte (device writes)
            let desc2 = &mut *desc_base.add((desc_idx + 2) as usize);
            desc2.addr = status_buf.phys.as_u64();
            desc2.len = 1;
            desc2.flags = desc_flags::WRITE;
            desc2.next = 0;
        }
        
        // Add to available ring
        self.queue.add_buffer(desc_idx);
        
        // Notify device
        self.device.write_u16(16, 0); // Queue notify
        
        // Wait for completion
        loop {
            if let Some((used_id, _len)) = self.queue.get_used() {
                if used_id == desc_idx as u32 {
                    break;
                }
            }
        }
        
        // Check status
        let status = unsafe {
            let status_ptr = status_buf.virt.as_u64() as *const u8;
            *status_ptr
        };
        
        if status != BlkStatus::Ok as u8 {
            self.queue.free_desc_chain(desc_idx);
            self.stats.errors += 1;
            return Err("Block write failed");
        }
        
        // Free descriptors
        self.queue.free_desc_chain(desc_idx);
        
        Ok(())
    }
    
    /// Flush writes to disk
    pub fn flush(&mut self) -> Result<(), &'static str> {
        // Allocate buffers
        let header_buf = DmaBuffer::new(core::mem::size_of::<BlkReq>())?;
        let status_buf = DmaBuffer::new(1)?;
        
        // Write flush request
        unsafe {
            let req = BlkReq::new_flush();
            core::ptr::write(header_buf.virt.as_u64() as *mut BlkReq, req);
        }
        
        // Allocate 2-descriptor chain
        let desc_idx = self.queue.alloc_desc_chain(2)?;
        
        unsafe {
            let desc_base = self.queue.desc.as_u64() as *mut crate::drivers::virtio::VirtqDesc;
            
            // Descriptor 0: Request header
            let desc0 = &mut *desc_base.add(desc_idx as usize);
            desc0.addr = header_buf.phys.as_u64();
            desc0.len = core::mem::size_of::<BlkReq>() as u32;
            desc0.flags = desc_flags::NEXT;
            desc0.next = desc_idx + 1;
            
            // Descriptor 1: Status byte
            let desc1 = &mut *desc_base.add((desc_idx + 1) as usize);
            desc1.addr = status_buf.phys.as_u64();
            desc1.len = 1;
            desc1.flags = desc_flags::WRITE;
            desc1.next = 0;
        }
        
        // Add to available ring
        self.queue.add_buffer(desc_idx);
        
        // Notify device
        self.device.write_u16(16, 0);
        
        // Wait for completion
        loop {
            if let Some((used_id, _len)) = self.queue.get_used() {
                if used_id == desc_idx as u32 {
                    break;
                }
            }
        }
        
        // Check status
        let status = unsafe {
            let status_ptr = status_buf.virt.as_u64() as *const u8;
            *status_ptr
        };
        
        self.queue.free_desc_chain(desc_idx);
        
        if status != BlkStatus::Ok as u8 {
            return Err("Flush failed");
        }
        
        Ok(())
    }
    
    /// Get device capacity in sectors
    pub fn capacity(&self) -> u64 {
        self.capacity
    }
    
    /// Get block size in bytes
    pub fn block_size(&self) -> u32 {
        self.block_size
    }
    
    /// Get statistics
    pub fn statistics(&self) -> BlockStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_blk_req_size() {
        assert_eq!(core::mem::size_of::<BlkReq>(), 16);
    }
    
    #[test]
    fn test_blk_req_read() {
        let req = BlkReq::new_read(100);
        assert_eq!(req.req_type, BlkReqType::In as u32);
        assert_eq!(req.sector, 100);
    }
    
    #[test]
    fn test_blk_req_write() {
        let req = BlkReq::new_write(200);
        assert_eq!(req.req_type, BlkReqType::Out as u32);
        assert_eq!(req.sector, 200);
    }
    
    #[test]
    fn test_blk_req_flush() {
        let req = BlkReq::new_flush();
        assert_eq!(req.req_type, BlkReqType::Flush as u32);
        assert_eq!(req.sector, 0);
    }
    
    #[test]
    fn test_block_stats_default() {
        let stats = BlockStats::default();
        assert_eq!(stats.read_requests, 0);
        assert_eq!(stats.write_requests, 0);
    }
    
    #[test]
    fn test_sector_size() {
        assert_eq!(VirtioBlock::SECTOR_SIZE, 512);
    }
}
