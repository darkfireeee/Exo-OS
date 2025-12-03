//! VirtIO Network Driver
//!
//! Driver for VirtIO network devices (commonly used in QEMU/KVM)
//! VirtIO device ID: 0x1AF4:0x1000

use alloc::vec::Vec;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use crate::drivers::pci::{PciDevice, PciBar, ids, PCI_BUS};

/// VirtIO vendor ID
const VIRTIO_VENDOR_ID: u16 = 0x1AF4;
/// VirtIO network device ID (legacy)
const VIRTIO_NET_DEVICE_ID: u16 = 0x1000;

/// VirtIO device status bits
#[allow(dead_code)]
mod status {
    pub const ACKNOWLEDGE: u8 = 1;
    pub const DRIVER: u8 = 2;
    pub const DRIVER_OK: u8 = 4;
    pub const FEATURES_OK: u8 = 8;
    pub const DEVICE_NEEDS_RESET: u8 = 64;
    pub const FAILED: u8 = 128;
}

/// VirtIO network feature bits
#[allow(dead_code)]
mod features {
    pub const VIRTIO_NET_F_CSUM: u32 = 1 << 0;
    pub const VIRTIO_NET_F_GUEST_CSUM: u32 = 1 << 1;
    pub const VIRTIO_NET_F_MAC: u32 = 1 << 5;
    pub const VIRTIO_NET_F_GSO: u32 = 1 << 6;
    pub const VIRTIO_NET_F_GUEST_TSO4: u32 = 1 << 7;
    pub const VIRTIO_NET_F_GUEST_TSO6: u32 = 1 << 8;
    pub const VIRTIO_NET_F_HOST_TSO4: u32 = 1 << 11;
    pub const VIRTIO_NET_F_HOST_TSO6: u32 = 1 << 12;
    pub const VIRTIO_NET_F_MRG_RXBUF: u32 = 1 << 15;
    pub const VIRTIO_NET_F_STATUS: u32 = 1 << 16;
    pub const VIRTIO_NET_F_CTRL_VQ: u32 = 1 << 17;
}

/// VirtIO register offsets (legacy PCI interface)
#[allow(dead_code)]
mod regs {
    pub const DEVICE_FEATURES: u16 = 0x00;
    pub const GUEST_FEATURES: u16 = 0x04;
    pub const QUEUE_ADDRESS: u16 = 0x08;
    pub const QUEUE_SIZE: u16 = 0x0C;
    pub const QUEUE_SELECT: u16 = 0x0E;
    pub const QUEUE_NOTIFY: u16 = 0x10;
    pub const DEVICE_STATUS: u16 = 0x12;
    pub const ISR_STATUS: u16 = 0x13;
    pub const MAC_ADDRESS: u16 = 0x14; // Network specific (offset in device config)
}

/// VirtIO queue descriptor
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtqDesc {
    /// Physical address of buffer
    pub addr: u64,
    /// Length of buffer
    pub len: u32,
    /// Flags
    pub flags: u16,
    /// Next descriptor in chain
    pub next: u16,
}

/// Descriptor flags
#[allow(dead_code)]
mod desc_flags {
    pub const NEXT: u16 = 1;
    pub const WRITE: u16 = 2;
    pub const INDIRECT: u16 = 4;
}

/// VirtIO available ring
#[repr(C)]
pub struct VirtqAvail {
    pub flags: u16,
    pub idx: u16,
    // ring: [u16; QUEUE_SIZE]
    // used_event: u16 (if VIRTIO_F_EVENT_IDX)
}

/// VirtIO used ring element
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtqUsedElem {
    pub id: u32,
    pub len: u32,
}

/// VirtIO used ring
#[repr(C)]
pub struct VirtqUsed {
    pub flags: u16,
    pub idx: u16,
    // ring: [VirtqUsedElem; QUEUE_SIZE]
    // avail_event: u16 (if VIRTIO_F_EVENT_IDX)
}

/// VirtIO network header
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtioNetHeader {
    pub flags: u8,
    pub gso_type: u8,
    pub hdr_len: u16,
    pub gso_size: u16,
    pub csum_start: u16,
    pub csum_offset: u16,
    pub num_buffers: u16,
}

/// Queue size (must be power of 2)
const QUEUE_SIZE: usize = 256;

/// VirtIO network driver
pub struct VirtioNetDriver {
    /// PCI device info
    pci_device: Option<PciDevice>,
    /// Base I/O port
    io_base: u16,
    /// MAC address
    mac_address: [u8; 6],
    /// TX queue descriptors
    tx_descriptors: Vec<VirtqDesc>,
    /// RX queue descriptors
    rx_descriptors: Vec<VirtqDesc>,
    /// RX buffers
    rx_buffers: Vec<Box<[u8; 2048]>>,
    /// Is driver initialized
    initialized: AtomicBool,
    /// Last used RX index
    rx_used_idx: u16,
    /// Last used TX index
    tx_used_idx: u16,
    /// Next available RX index
    rx_avail_idx: u16,
    /// Next available TX index
    tx_avail_idx: u16,
}

impl VirtioNetDriver {
    /// Create new VirtIO network driver instance
    pub const fn new() -> Self {
        Self {
            pci_device: None,
            io_base: 0,
            mac_address: [0u8; 6],
            tx_descriptors: Vec::new(),
            rx_descriptors: Vec::new(),
            rx_buffers: Vec::new(),
            initialized: AtomicBool::new(false),
            rx_used_idx: 0,
            tx_used_idx: 0,
            rx_avail_idx: 0,
            tx_avail_idx: 0,
        }
    }
    
    /// Detect VirtIO network device on PCI bus
    pub fn detect() -> Option<Self> {
        let pci_bus = PCI_BUS.lock();
        
        // Search for VirtIO network device
        if let Some(device) = pci_bus.find_device(ids::VIRTIO_NET) {
            log::info!("Found VirtIO-Net at {}", device.bdf());
            
            // Get I/O base from BAR0
            if let Some(io_base) = device.io_base() {
                log::info!("  I/O base: 0x{:04X}", io_base);
                
                let mut driver = Self::new();
                driver.pci_device = Some(device.clone());
                driver.io_base = io_base as u16;
                
                return Some(driver);
            }
        }
        
        None
    }
    
    /// Initialize the driver
    pub fn init(&mut self) -> bool {
        if self.initialized.load(Ordering::SeqCst) {
            return true;
        }
        
        if self.io_base == 0 {
            log::error!("VirtIO-Net: No I/O base configured");
            return false;
        }
        
        // Enable PCI bus mastering and I/O space
        if let Some(ref pci_dev) = self.pci_device {
            pci_dev.enable_bus_master();
            pci_dev.enable_io();
        }
        
        unsafe {
            // 1. Reset device
            self.write_status(0);
            
            // 2. Set ACKNOWLEDGE status bit
            self.write_status(status::ACKNOWLEDGE);
            
            // 3. Set DRIVER status bit
            self.write_status(status::ACKNOWLEDGE | status::DRIVER);
            
            // 4. Read device features
            let device_features = self.read_features();
            log::debug!("VirtIO-Net features: 0x{:08X}", device_features);
            
            // 5. Negotiate features (accept basic features)
            let mut guest_features = 0u32;
            if device_features & features::VIRTIO_NET_F_MAC != 0 {
                guest_features |= features::VIRTIO_NET_F_MAC;
            }
            self.write_features(guest_features);
            
            // 6. Set FEATURES_OK status bit
            self.write_status(status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK);
            
            // 7. Re-read status to ensure FEATURES_OK is still set
            let status = self.read_status();
            if status & status::FEATURES_OK == 0 {
                log::error!("VirtIO-Net: Feature negotiation failed");
                return false;
            }
            
            // 8. Read MAC address
            if guest_features & features::VIRTIO_NET_F_MAC != 0 {
                self.read_mac_address();
                log::info!(
                    "VirtIO-Net MAC: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                    self.mac_address[0], self.mac_address[1], self.mac_address[2],
                    self.mac_address[3], self.mac_address[4], self.mac_address[5]
                );
            }
            
            // 9. Setup virtqueues
            if !self.setup_queues() {
                log::error!("VirtIO-Net: Failed to setup queues");
                return false;
            }
            
            // 10. Set DRIVER_OK status bit - device is ready
            self.write_status(status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK);
            
            log::info!("VirtIO-Net: Driver initialized successfully");
        }
        
        self.initialized.store(true, Ordering::SeqCst);
        true
    }
    
    /// Setup virtqueues for TX and RX
    fn setup_queues(&mut self) -> bool {
        // TODO: Implement proper virtqueue setup
        // This requires:
        // 1. Allocate physically contiguous memory for queues
        // 2. Calculate queue addresses
        // 3. Set queue size
        // 4. Set queue address in device
        // 5. Populate RX queue with buffers
        
        log::debug!("VirtIO-Net: Queue setup (stub)");
        true
    }
    
    /// Read device status
    unsafe fn read_status(&self) -> u8 {
        use x86_64::instructions::port::Port;
        let mut port: Port<u8> = Port::new(self.io_base + regs::DEVICE_STATUS);
        port.read()
    }
    
    /// Write device status
    unsafe fn write_status(&self, status: u8) {
        use x86_64::instructions::port::Port;
        let mut port: Port<u8> = Port::new(self.io_base + regs::DEVICE_STATUS);
        port.write(status);
    }
    
    /// Read device features
    unsafe fn read_features(&self) -> u32 {
        use x86_64::instructions::port::Port;
        let mut port: Port<u32> = Port::new(self.io_base + regs::DEVICE_FEATURES);
        port.read()
    }
    
    /// Write guest features
    unsafe fn write_features(&self, features: u32) {
        use x86_64::instructions::port::Port;
        let mut port: Port<u32> = Port::new(self.io_base + regs::GUEST_FEATURES);
        port.write(features);
    }
    
    /// Read MAC address from device config
    unsafe fn read_mac_address(&mut self) {
        use x86_64::instructions::port::Port;
        // MAC address is in device-specific config area (starts at offset 0x14 for legacy)
        let config_offset = self.io_base + regs::MAC_ADDRESS;
        for i in 0..6 {
            let mut port: Port<u8> = Port::new(config_offset + i);
            self.mac_address[i as usize] = port.read();
        }
    }
    
    /// Send a packet
    pub fn send(&mut self, data: &[u8]) -> bool {
        if !self.initialized.load(Ordering::SeqCst) {
            return false;
        }
        
        if data.len() > 1500 {
            log::error!("VirtIO-Net: Packet too large ({} bytes)", data.len());
            return false;
        }
        
        // TODO: Implement packet sending via TX queue
        // 1. Allocate TX descriptor
        // 2. Copy data to buffer (with virtio_net_hdr)
        // 3. Add to available ring
        // 4. Notify device
        
        log::debug!("VirtIO-Net: send {} bytes (stub)", data.len());
        true
    }
    
    /// Receive a packet (returns None if no packet available)
    pub fn receive(&mut self) -> Option<Vec<u8>> {
        if !self.initialized.load(Ordering::SeqCst) {
            return None;
        }
        
        // TODO: Implement packet receiving from RX queue
        // 1. Check used ring for completed buffers
        // 2. Copy data from buffer (strip virtio_net_hdr)
        // 3. Return buffer to available ring
        // 4. Return packet data
        
        None
    }
    
    /// Get MAC address
    pub fn mac_address(&self) -> [u8; 6] {
        self.mac_address
    }
    
    /// Check if driver is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::SeqCst)
    }
}

/// Global VirtIO network driver instance
pub static VIRTIO_NET: Mutex<Option<VirtioNetDriver>> = Mutex::new(None);

/// Initialize VirtIO network driver
pub fn init() -> bool {
    if let Some(mut driver) = VirtioNetDriver::detect() {
        if driver.init() {
            *VIRTIO_NET.lock() = Some(driver);
            true
        } else {
            false
        }
    } else {
        log::debug!("VirtIO-Net: No device found");
        false
    }
}
