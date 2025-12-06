//! VirtIO Block Device Driver
//!
//! Provides access to virtual block devices in QEMU/KVM.

use super::BlockDevice;
use crate::drivers::{Driver, DeviceInfo, DriverError, DriverResult};
use crate::drivers::pci::{PciDevice, PCI_BUS, ids};
use crate::drivers::virtio::{Virtqueue, VIRTIO_STATUS_ACKNOWLEDGE, VIRTIO_STATUS_DRIVER, VIRTIO_STATUS_FEATURES_OK, VIRTIO_STATUS_DRIVER_OK};
use crate::memory::dma_simple::{dma_alloc_coherent, dma_free_coherent};
use spin::Mutex;
use lazy_static::lazy_static;
use alloc::vec::Vec;

/// VirtIO-Blk device registers (legacy)
const VIRTIO_BLK_REG_DEVICE_FEATURES: usize = 0x00;
const VIRTIO_BLK_REG_DRIVER_FEATURES: usize = 0x04;
const VIRTIO_BLK_REG_QUEUE_ADDR: usize = 0x08;
const VIRTIO_BLK_REG_QUEUE_SIZE: usize = 0x0C;
const VIRTIO_BLK_REG_QUEUE_SELECT: usize = 0x0E;
const VIRTIO_BLK_REG_QUEUE_NOTIFY: usize = 0x10;
const VIRTIO_BLK_REG_DEVICE_STATUS: usize = 0x12;
const VIRTIO_BLK_REG_ISR_STATUS: usize = 0x13;

/// VirtIO-Blk request header
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct VirtioBlkReqHeader {
    req_type: u32,      // 0=in, 1=out, 4=flush
    reserved: u32,
    sector: u64,
}

/// VirtIO-Blk request status
const VIRTIO_BLK_S_OK: u8 = 0;
const VIRTIO_BLK_S_IOERR: u8 = 1;
const VIRTIO_BLK_S_UNSUPP: u8 = 2;

/// VirtIO Block Driver
pub struct VirtioBlkDriver {
    pci_device: Option<PciDevice>,
    base_addr: u64,
    capacity: u64,
    virtqueue: Option<Virtqueue>,
    initialized: bool,
}

impl VirtioBlkDriver {
    pub const fn new() -> Self {
        Self {
            pci_device: None,
            base_addr: 0,
            capacity: 0,
            virtqueue: None,
            initialized: false,
        }
    }
    
    /// Initialize the VirtIO-Blk device
    pub fn init(&mut self) -> DriverResult<()> {
        // Find VirtIO-Blk device
        let bus = PCI_BUS.lock();
        let device = bus.find_device(ids::VIRTIO_BLK)
            .ok_or(DriverError::DeviceNotFound)?;
        
        self.pci_device = Some(device.clone());
        
        // Get BAR0 (I/O port base)
        self.base_addr = device.memory_base()
            .ok_or(DriverError::InitFailed)?;
        
        drop(bus);
        
        log::info!("VirtIO-Blk found at BAR0={:#x}", self.base_addr);
        
        // Reset device
        self.write_reg8(VIRTIO_BLK_REG_DEVICE_STATUS, 0);
        
        // Set ACKNOWLEDGE bit
        self.write_reg8(VIRTIO_BLK_REG_DEVICE_STATUS, VIRTIO_STATUS_ACKNOWLEDGE);
        
        // Set DRIVER bit
        self.write_reg8(VIRTIO_BLK_REG_DEVICE_STATUS, 
                        VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER);
        
        // Read device features
        let device_features = self.read_reg32(VIRTIO_BLK_REG_DEVICE_FEATURES);
        log::debug!("VirtIO-Blk features: {:#x}", device_features);
        
        // Acknowledge all features (for now)
        self.write_reg32(VIRTIO_BLK_REG_DRIVER_FEATURES, device_features);
        
        // Set FEATURES_OK
        self.write_reg8(VIRTIO_BLK_REG_DEVICE_STATUS,
                        VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK);
        
        // Verify FEATURES_OK
        let status = self.read_reg8(VIRTIO_BLK_REG_DEVICE_STATUS);
        if (status & VIRTIO_STATUS_FEATURES_OK) == 0 {
            return Err(DriverError::InitFailed);
        }
        
        // Setup virtqueue
        self.write_reg16(VIRTIO_BLK_REG_QUEUE_SELECT, 0);
        let queue_size = self.read_reg16(VIRTIO_BLK_REG_QUEUE_SIZE);
        
        log::info!("VirtIO-Blk queue size: {}", queue_size);
        
        let mut vq = Virtqueue::new(queue_size)
            .map_err(|_| DriverError::NoMemory)?;
        
        let (desc_phys, avail_phys, used_phys) = vq.addresses();
        
        // Configure queue address (legacy: physical address / 4096)
        self.write_reg32(VIRTIO_BLK_REG_QUEUE_ADDR, (desc_phys / 4096) as u32);
        
        self.virtqueue = Some(vq);
        
        // Set DRIVER_OK
        self.write_reg8(VIRTIO_BLK_REG_DEVICE_STATUS,
                        VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | 
                        VIRTIO_STATUS_FEATURES_OK | VIRTIO_STATUS_DRIVER_OK);
        
        // Read capacity (at offset 0x14 in device config)
        self.capacity = self.read_reg64(0x14);
        
        log::info!("VirtIO-Blk: {} sectors ({}MB)",
                   self.capacity, self.capacity * 512 / 1024 / 1024);
        
        self.initialized = true;
        Ok(())
    }
    
    fn read_reg8(&self, offset: usize) -> u8 {
        unsafe { core::ptr::read_volatile((self.base_addr as usize + offset) as *const u8) }
    }
    
    fn write_reg8(&mut self, offset: usize, value: u8) {
        unsafe { core::ptr::write_volatile((self.base_addr as usize + offset) as *mut u8, value) }
    }
    
    fn read_reg16(&self, offset: usize) -> u16 {
        unsafe { core::ptr::read_volatile((self.base_addr as usize + offset) as *const u16) }
    }
    
    fn write_reg16(&mut self, offset: usize, value: u16) {
        unsafe { core::ptr::write_volatile((self.base_addr as usize + offset) as *mut u16, value) }
    }
    
    fn read_reg32(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.base_addr as usize + offset) as *const u32) }
    }
    
    fn write_reg32(&mut self, offset: usize, value: u32) {
        unsafe { core::ptr::write_volatile((self.base_addr as usize + offset) as *mut u32, value) }
    }
    
    fn read_reg64(&self, offset: usize) -> u64 {
        let lo = self.read_reg32(offset) as u64;
        let hi = self.read_reg32(offset + 4) as u64;
        (hi << 32) | lo
    }
}

impl BlockDevice for VirtioBlkDriver {
    fn read(&mut self, sector: u64, buffer: &mut [u8]) -> DriverResult<usize> {
        if !self.initialized {
            return Err(DriverError::NotSupported);
        }
        
        let vq = self.virtqueue.as_mut().ok_or(DriverError::InitFailed)?;
        
        // Allocate DMA buffers
        let (hdr_virt, hdr_phys) = dma_alloc_coherent(
            core::mem::size_of::<VirtioBlkReqHeader>(), true
        ).map_err(|_| DriverError::NoMemory)?;
        
        let (data_virt, data_phys) = dma_alloc_coherent(buffer.len(), true)
            .map_err(|_| DriverError::NoMemory)?;
        
        let (status_virt, status_phys) = dma_alloc_coherent(1, true)
            .map_err(|_| DriverError::NoMemory)?;
        
        // Fill request header
        unsafe {
            let hdr = &mut *(hdr_virt as *mut VirtioBlkReqHeader);
            hdr.req_type = 0; // VIRTIO_BLK_T_IN (read)
            hdr.reserved = 0;
            hdr.sector = sector;
        }
        
        // Add to virtqueue
        let buffers = [
            (hdr_phys, core::mem::size_of::<VirtioBlkReqHeader>() as u32, false),
            (data_phys, buffer.len() as u32, true),
            (status_phys, 1, true),
        ];
        
        vq.add_buffer(&buffers).map_err(|_| DriverError::ResourceBusy)?;
        
        // Notify device
        vq.kick(self.base_addr + VIRTIO_BLK_REG_QUEUE_NOTIFY as u64);
        
        // Wait for completion (busy wait for now)
        while !vq.has_used() {
            core::hint::spin_loop();
        }
        
        // Get used buffer
        if let Some((_, len)) = vq.get_used() {
            // Copy data to buffer
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data_virt as *const u8,
                    buffer.as_mut_ptr(),
                    buffer.len()
                );
            }
            
            // Check status
            let status = unsafe { *(status_virt as *const u8) };
            
            // Free DMA buffers
            let _ = dma_free_coherent(hdr_virt);
            let _ = dma_free_coherent(data_virt);
            let _ = dma_free_coherent(status_virt);
            
            if status == VIRTIO_BLK_S_OK {
                Ok(buffer.len())
            } else {
                Err(DriverError::IoError)
            }
        } else {
            Err(DriverError::IoError)
        }
    }
    
    fn write(&mut self, sector: u64, data: &[u8]) -> DriverResult<usize> {
        if !self.initialized {
            return Err(DriverError::NotSupported);
        }
        
        // Similar to read but with req_type = 1 (VIRTIO_BLK_T_OUT)
        Err(DriverError::NotSupported) // TODO: Implement write
    }
    
    fn sector_size(&self) -> usize {
        512
    }
    
    fn total_sectors(&self) -> u64 {
        self.capacity
    }
}

impl Driver for VirtioBlkDriver {
    fn name(&self) -> &str {
        "VirtIO Block Driver"
    }
    
    fn init(&mut self) -> DriverResult<()> {
        VirtioBlkDriver::init(self)
    }
    
    fn probe(&self) -> DriverResult<DeviceInfo> {
        Ok(DeviceInfo {
            name: "VirtIO Block Device",
            vendor_id: 0x1AF4,
            device_id: 0x1001,
        })
    }
}

lazy_static! {
    pub static ref VIRTIO_BLK_DEVICE: Mutex<VirtioBlkDriver> = Mutex::new(VirtioBlkDriver::new());
}

/// Initialize VirtIO-Blk driver
pub fn init() -> bool {
    match VIRTIO_BLK_DEVICE.lock().init() {
        Ok(_) => {
            log::info!("VirtIO-Blk driver initialized successfully");
            true
        }
        Err(e) => {
            log::warn!("VirtIO-Blk initialization failed: {:?}", e);
            false
        }
    }
}
