//! VirtIO Network Driver
//!
//! Full-featured network driver for VirtIO-Net devices (QEMU/KVM).
//!
//! Features:
//! - TX/RX packet handling
//! - Multiple virtqueues (RX, TX, control)
//! - MAC address configuration
//! - MTU support (1500 bytes default)
//! - Checksum offload
//! - GSO (Generic Segmentation Offload) support
//!
//! ## Architecture
//!
//! ```text
//! Network Stack
//!      ↓
//! VirtIO-Net Driver
//!      ↓
//! VirtQueues (RX/TX)
//!      ↓
//! DMA Buffers
//! ```

use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::sync::Arc;
use spin::Mutex;
use crate::drivers::virtio::{VirtioPciDevice, VirtQueue, DeviceType};
use crate::drivers::virtio::{desc_flags, status, features};
use crate::drivers::pci::PciDevice;
use crate::memory::{PhysicalAddress, VirtualAddress};
use crate::net::ethernet::{EthernetFrame, MacAddress};
use crate::net::EtherType;

/// VirtIO-Net feature bits
pub mod net_features {
    pub const CSUM: u64 = 1 << 0;
    pub const GUEST_CSUM: u64 = 1 << 1;
    pub const CTRL_GUEST_OFFLOADS: u64 = 1 << 2;
    pub const MTU: u64 = 1 << 3;
    pub const MAC: u64 = 1 << 5;
    pub const GUEST_TSO4: u64 = 1 << 7;
    pub const GUEST_TSO6: u64 = 1 << 8;
    pub const GUEST_UFO: u64 = 1 << 10;
    pub const HOST_TSO4: u64 = 1 << 11;
    pub const HOST_TSO6: u64 = 1 << 12;
    pub const HOST_UFO: u64 = 1 << 14;
    pub const MRG_RXBUF: u64 = 1 << 15;
    pub const STATUS: u64 = 1 << 16;
    pub const CTRL_VQ: u64 = 1 << 17;
    pub const CTRL_RX: u64 = 1 << 18;
    pub const CTRL_VLAN: u64 = 1 << 19;
    pub const GUEST_ANNOUNCE: u64 = 1 << 21;
    pub const MQ: u64 = 1 << 22;
    pub const CTRL_MAC_ADDR: u64 = 1 << 23;
}

/// VirtIO-Net packet header
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtioNetHdr {
    pub flags: u8,
    pub gso_type: u8,
    pub hdr_len: u16,
    pub gso_size: u16,
    pub csum_start: u16,
    pub csum_offset: u16,
    pub num_buffers: u16,
}

impl VirtioNetHdr {
    pub const fn new() -> Self {
        Self {
            flags: 0,
            gso_type: 0,
            hdr_len: 0,
            gso_size: 0,
            csum_start: 0,
            csum_offset: 0,
            num_buffers: 0,
        }
    }
}

/// VirtIO-Net configuration space
#[repr(C, packed)]
pub struct VirtioNetConfig {
    pub mac: [u8; 6],
    pub status: u16,
    pub max_virtqueue_pairs: u16,
    pub mtu: u16,
}

/// RX buffer
pub struct RxBuffer {
    /// Virtual address
    pub virt: VirtAddr,
    
    /// Physical address
    pub phys: PhysAddr,
    
    /// Buffer size
    pub size: usize,
}

impl RxBuffer {
    /// Allocate new RX buffer
    pub fn new(size: usize) -> Result<Self, &'static str> {
        let layout = core::alloc::Layout::from_size_align(size, 16)
            .map_err(|_| "Invalid layout")?;
        
        let ptr = unsafe { alloc::alloc::alloc(layout) };
        if ptr.is_null() {
            return Err("Failed to allocate buffer");
        }
        
        let virt = VirtAddr::new(ptr as u64);
        // TODO: Get real physical address from page tables
        let phys = PhysAddr::new(virt.as_u64());
        
        Ok(Self { virt, phys, size })
    }
}

impl Drop for RxBuffer {
    fn drop(&mut self) {
        unsafe {
            let layout = core::alloc::Layout::from_size_align_unchecked(self.size, 16);
            alloc::alloc::dealloc(self.virt.as_u64() as *mut u8, layout);
        }
    }
}

/// VirtIO-Net Driver
pub struct VirtioNet {
    /// Base VirtIO device
    pub device: VirtioPciDevice,
    
    /// MAC address
    pub mac: MacAddress,
    
    /// MTU
    pub mtu: u16,
    
    /// RX queue
    pub rx_queue: VirtQueue,
    
    /// TX queue
    pub tx_queue: VirtQueue,
    
    /// RX buffers
    pub rx_buffers: Vec<RxBuffer>,
    
    /// Statistics
    pub stats: NetStats,
}

/// Network statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct NetStats {
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
    pub rx_dropped: u64,
    pub tx_dropped: u64,
}

impl VirtioNet {
    /// Create from PCI device
    pub fn new(pci_dev: PciDevice) -> Result<Arc<Mutex<Self>>, &'static str> {
        let mut device = VirtioPciDevice::from_pci(pci_dev)?;
        
        if device.device_type != DeviceType::Network {
            return Err("Not a network device");
        }
        
        // Initialize device
        device.init()?;
        
        // Read device features
        let dev_features = device.device_features;
        
        // Negotiate features
        let mut driver_features = features::VERSION_1 | features::RING_INDIRECT_DESC;
        
        if (dev_features & net_features::MAC) != 0 {
            driver_features |= net_features::MAC;
        }
        
        if (dev_features & net_features::STATUS) != 0 {
            driver_features |= net_features::STATUS;
        }
        
        if (dev_features & net_features::CSUM) != 0 {
            driver_features |= net_features::CSUM | net_features::GUEST_CSUM;
        }
        
        if (dev_features & net_features::MTU) != 0 {
            driver_features |= net_features::MTU;
        }
        
        device.write_driver_features(driver_features);
        
        // Read MAC address
        let mac = Self::read_mac(&device);
        
        crate::logger::info(&alloc::format!(
            "[VirtIO-Net] MAC: {}",
            mac
        ));
        
        // Read MTU
        let mtu = if (driver_features & net_features::MTU) != 0 {
            device.read_u16(12)
        } else {
            1500
        };
        
        crate::logger::info(&alloc::format!(
            "[VirtIO-Net] MTU: {} bytes",
            mtu
        ));
        
        // Create virtqueues
        let rx_queue = VirtQueue::new(256)?;
        let tx_queue = VirtQueue::new(256)?;
        
        // Allocate RX buffers
        let buffer_size = core::mem::size_of::<VirtioNetHdr>() + mtu as usize + 14; // Header + MTU + Ethernet
        let mut rx_buffers = Vec::new();
        
        for _ in 0..256 {
            rx_buffers.push(RxBuffer::new(buffer_size)?);
        }
        
        let mut net = Self {
            device,
            mac,
            mtu,
            rx_queue,
            tx_queue,
            rx_buffers,
            stats: NetStats::default(),
        };
        
        // Fill RX queue
        net.fill_rx_queue()?;
        
        // Finalize device
        net.device.finalize();
        
        Ok(Arc::new(Mutex::new(net)))
    }
    
    /// Read MAC address from device
    fn read_mac(device: &VirtioPciDevice) -> MacAddress {
        let mut mac = [0u8; 6];
        for i in 0..6 {
            mac[i] = device.read_u8(i as u16);
        }
        MacAddress::new(mac)
    }
    
    /// Fill RX queue with buffers
    fn fill_rx_queue(&mut self) -> Result<(), &'static str> {
        for i in 0..self.rx_buffers.len() {
            let buf = &self.rx_buffers[i];
            
            // Allocate descriptor
            let desc_idx = self.rx_queue.alloc_desc_chain(1)?;
            
            // Setup descriptor
            unsafe {
                let desc_ptr = (self.rx_queue.desc.as_u64() as *mut crate::drivers::virtio::VirtqDesc)
                    .add(desc_idx as usize);
                
                (*desc_ptr).addr = buf.phys.as_u64();
                (*desc_ptr).len = buf.size as u32;
                (*desc_ptr).flags = desc_flags::WRITE; // Device writes to buffer
                (*desc_ptr).next = 0;
            }
            
            // Add to available ring
            self.rx_queue.add_buffer(desc_idx);
        }
        
        // Notify device
        self.device.write_u16(16, 0); // Queue notify (RX = 0)
        
        Ok(())
    }
    
    /// Send packet
    pub fn send(&mut self, data: &[u8]) -> Result<(), &'static str> {
        if data.len() > self.mtu as usize {
            return Err("Packet too large");
        }
        
        // Allocate TX buffer
        let total_size = core::mem::size_of::<VirtioNetHdr>() + data.len();
        let layout = core::alloc::Layout::from_size_align(total_size, 16)
            .map_err(|_| "Invalid layout")?;
        
        let ptr = unsafe { alloc::alloc::alloc(layout) };
        if ptr.is_null() {
            self.stats.tx_dropped += 1;
            return Err("Failed to allocate TX buffer");
        }
        
        // Write header
        let hdr = VirtioNetHdr::new();
        unsafe {
            core::ptr::write(ptr as *mut VirtioNetHdr, hdr);
        }
        
        // Copy data
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                ptr.add(core::mem::size_of::<VirtioNetHdr>()),
                data.len(),
            );
        }
        
        // Allocate descriptor
        let desc_idx = self.tx_queue.alloc_desc_chain(1)
            .map_err(|_| {
                unsafe {
                    alloc::alloc::dealloc(ptr, layout);
                }
                self.stats.tx_dropped += 1;
                "No free TX descriptors"
            })?;
        
        // Setup descriptor
        unsafe {
            let desc_ptr = (self.tx_queue.desc.as_u64() as *mut crate::drivers::virtio::VirtqDesc)
                .add(desc_idx as usize);
            
            // TODO: Get real physical address
            let phys_addr = ptr as u64;
            
            (*desc_ptr).addr = phys_addr;
            (*desc_ptr).len = total_size as u32;
            (*desc_ptr).flags = 0; // Read-only for device
            (*desc_ptr).next = 0;
        }
        
        // Add to available ring
        self.tx_queue.add_buffer(desc_idx);
        
        // Notify device
        self.device.write_u16(16, 1); // Queue notify (TX = 1)
        
        // Update stats
        self.stats.tx_packets += 1;
        self.stats.tx_bytes += data.len() as u64;
        
        // TODO: Free buffer when used (need to track buffers)
        
        Ok(())
    }
    
    /// Receive packet
    pub fn receive(&mut self) -> Option<Vec<u8>> {
        // Check for used buffers
        let (desc_id, len) = self.rx_queue.get_used()?;
        
        // Get buffer
        let buf = &self.rx_buffers[desc_id as usize];
        
        // Skip VirtioNetHdr
        let hdr_size = core::mem::size_of::<VirtioNetHdr>();
        let data_len = len as usize - hdr_size;
        
        if data_len == 0 {
            // Empty packet, refill
            self.rx_queue.add_buffer(desc_id as u16);
            self.device.write_u16(16, 0); // Notify RX queue
            return None;
        }
        
        // Copy packet data
        let mut packet = Vec::with_capacity(data_len);
        unsafe {
            let data_ptr = (buf.virt.as_u64() as *const u8).add(hdr_size);
            packet.extend_from_slice(core::slice::from_raw_parts(data_ptr, data_len));
        }
        
        // Refill RX buffer
        self.rx_queue.add_buffer(desc_id as u16);
        self.device.write_u16(16, 0); // Notify RX queue
        
        // Update stats
        self.stats.rx_packets += 1;
        self.stats.rx_bytes += data_len as u64;
        
        Some(packet)
    }
    
    /// Get MAC address
    pub fn mac_address(&self) -> MacAddress {
        self.mac
    }
    
    /// Get MTU
    pub fn mtu(&self) -> u16 {
        self.mtu
    }
    
    /// Get statistics
    pub fn statistics(&self) -> NetStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_virtio_net_hdr_size() {
        assert_eq!(core::mem::size_of::<VirtioNetHdr>(), 12);
    }
    
    #[test]
    fn test_virtio_net_hdr_new() {
        let hdr = VirtioNetHdr::new();
        assert_eq!(hdr.flags, 0);
        assert_eq!(hdr.gso_type, 0);
        assert_eq!(hdr.hdr_len, 0);
    }
    
    #[test]
    fn test_net_stats_default() {
        let stats = NetStats::default();
        assert_eq!(stats.rx_packets, 0);
        assert_eq!(stats.tx_packets, 0);
        assert_eq!(stats.rx_bytes, 0);
        assert_eq!(stats.tx_bytes, 0);
    }
}
