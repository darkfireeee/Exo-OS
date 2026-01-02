//! VirtIO Framework
//!
//! VirtIO is a standardized interface for virtual devices in QEMU/KVM.
//! Supports:
//! - VirtIO-Net (network)
//! - VirtIO-Block (storage)
//! - VirtIO-Console
//! - VirtIO-GPU
//!
//! ## VirtIO Architecture
//!
//! ```text
//! Driver <-> VirtQueue <-> Device
//!            (shared memory rings for DMA)
//! ```

pub mod net;    // VirtIO-Net driver
pub mod block;  // VirtIO-Block driver

use alloc::vec::Vec;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicU16, Ordering};
use spin::Mutex;
use crate::memory::{PhysAddr, VirtAddr};
use crate::drivers::pci::{PciDevice, PciAddress, BarType};
use crate::drivers::pci::{pci_read_u8, pci_write_u8, pci_read_u16, pci_write_u16};
use crate::drivers::pci::{pci_read_u32, pci_write_u32};

/// VirtIO device types
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Reserved = 0,
    Network = 1,
    Block = 2,
    Console = 3,
    Entropy = 4,
    Balloon = 5,
    IoMemory = 6,
    Rpmsg = 7,
    ScsiHost = 8,
    Transport9P = 9,
    Mac80211Wlan = 10,
    RprocSerial = 11,
    Caif = 12,
    Gpu = 16,
    Input = 18,
    Socket = 19,
    Crypto = 20,
}

/// VirtIO Status register bits
pub mod status {
    pub const ACKNOWLEDGE: u8 = 1;
    pub const DRIVER: u8 = 2;
    pub const DRIVER_OK: u8 = 4;
    pub const FEATURES_OK: u8 = 8;
    pub const DEVICE_NEEDS_RESET: u8 = 64;
    pub const FAILED: u8 = 128;
}

/// VirtIO Feature bits (common)
pub mod features {
    pub const NOTIFY_ON_EMPTY: u64 = 1 << 24;
    pub const ANY_LAYOUT: u64 = 1 << 27;
    pub const RING_INDIRECT_DESC: u64 = 1 << 28;
    pub const RING_EVENT_IDX: u64 = 1 << 29;
    pub const VERSION_1: u64 = 1 << 32;
}

/// VirtQueue descriptor flags
pub mod desc_flags {
    pub const NEXT: u16 = 1;
    pub const WRITE: u16 = 2;
    pub const INDIRECT: u16 = 4;
}

/// VirtQueue descriptor
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtqDesc {
    /// Physical address
    pub addr: u64,
    
    /// Length in bytes
    pub len: u32,
    
    /// Flags
    pub flags: u16,
    
    /// Next descriptor index (if NEXT flag set)
    pub next: u16,
}

/// VirtQueue available ring
#[repr(C)]
pub struct VirtqAvail {
    pub flags: u16,
    pub idx: AtomicU16,
    // Followed by: ring[queue_size]
    // Followed by: used_event (if EVENT_IDX feature)
}

/// VirtQueue used element
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VirtqUsedElem {
    /// Index of start of used descriptor chain
    pub id: u32,
    
    /// Total length of descriptor chain
    pub len: u32,
}

/// VirtQueue used ring
#[repr(C)]
pub struct VirtqUsed {
    pub flags: u16,
    pub idx: AtomicU16,
    // Followed by: ring[queue_size] of VirtqUsedElem
    // Followed by: avail_event (if EVENT_IDX feature)
}

/// VirtQueue (split virtqueue layout)
pub struct VirtQueue {
    /// Queue size (must be power of 2)
    pub size: u16,
    
    /// Descriptor table
    pub desc: VirtAddr,
    
    /// Available ring
    pub avail: VirtAddr,
    
    /// Used ring
    pub used: VirtAddr,
    
    /// Last seen used index
    pub last_used_idx: u16,
    
    /// Free descriptor list
    pub free_desc: Vec<u16>,
}

impl VirtQueue {
    /// Create new VirtQueue
    pub fn new(size: u16) -> Result<Self, &'static str> {
        if !size.is_power_of_two() || size == 0 || size > 32768 {
            return Err("Invalid queue size");
        }
        
        // Calculate sizes
        let desc_size = core::mem::size_of::<VirtqDesc>() * size as usize;
        let avail_size = 6 + (2 * size as usize);
        let used_size = 6 + (8 * size as usize);
        
        // Align used ring to page boundary
        let total_size = desc_size + avail_size;
        let used_offset = ((total_size + 4095) / 4096) * 4096;
        let total_alloc = used_offset + used_size;
        
        // Allocate memory
        // TODO: Use proper DMA-capable allocator
        let layout = core::alloc::Layout::from_size_align(total_alloc, 4096)
            .map_err(|_| "Failed to create layout")?;
        
        let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err("Failed to allocate queue memory");
        }
        
        let base = VirtAddr::new(ptr as u64);
        let desc = base;
        let avail = VirtAddr::new(base.as_u64() + desc_size as u64);
        let used = VirtAddr::new(base.as_u64() + used_offset as u64);
        
        // Initialize free descriptor list
        let free_desc = (0..size).collect();
        
        Ok(Self {
            size,
            desc,
            avail,
            used,
            last_used_idx: 0,
            free_desc,
        })
    }
    
    /// Allocate descriptor chain
    pub fn alloc_desc_chain(&mut self, count: u16) -> Result<u16, &'static str> {
        if count == 0 || count as usize > self.free_desc.len() {
            return Err("Not enough free descriptors");
        }
        
        let head = self.free_desc.remove(0);
        
        // Chain descriptors
        let mut current = head;
        for i in 1..count {
            let next = self.free_desc.remove(0);
            
            unsafe {
                let desc_ptr = (self.desc.as_u64() as *mut VirtqDesc).add(current as usize);
                (*desc_ptr).flags |= desc_flags::NEXT;
                (*desc_ptr).next = next;
            }
            
            current = next;
        }
        
        Ok(head)
    }
    
    /// Free descriptor chain
    pub fn free_desc_chain(&mut self, head: u16) {
        let mut current = head;
        
        loop {
            unsafe {
                let desc_ptr = (self.desc.as_u64() as *mut VirtqDesc).add(current as usize);
                let desc = &mut *desc_ptr;
                
                let has_next = (desc.flags & desc_flags::NEXT) != 0;
                let next = desc.next;
                
                // Clear descriptor
                desc.addr = 0;
                desc.len = 0;
                desc.flags = 0;
                desc.next = 0;
                
                self.free_desc.push(current);
                
                if !has_next {
                    break;
                }
                
                current = next;
            }
        }
    }
    
    /// Add buffer to available ring
    pub fn add_buffer(&mut self, desc_idx: u16) {
        unsafe {
            let avail_ptr = self.avail.as_u64() as *mut VirtqAvail;
            let avail = &mut *avail_ptr;
            
            let idx = avail.idx.load(Ordering::Acquire);
            let ring_ptr = (avail_ptr as *mut u16).add(2); // Skip flags and idx
            
            let ring_idx = (idx % self.size) as isize;
            *ring_ptr.offset(ring_idx) = desc_idx;
            
            // Memory barrier
            core::sync::atomic::fence(Ordering::Release);
            
            avail.idx.store(idx.wrapping_add(1), Ordering::Release);
        }
    }
    
    /// Check for used buffers
    pub fn get_used(&mut self) -> Option<(u32, u32)> {
        unsafe {
            let used_ptr = self.used.as_u64() as *const VirtqUsed;
            let used = &*used_ptr;
            
            let idx = used.idx.load(Ordering::Acquire);
            
            if self.last_used_idx == idx {
                return None; // No new used buffers
            }
            
            let ring_ptr = (used_ptr as *const VirtqUsedElem).add(1); // Skip header
            let ring_idx = (self.last_used_idx % self.size) as isize;
            
            let elem = *ring_ptr.offset(ring_idx);
            
            self.last_used_idx = self.last_used_idx.wrapping_add(1);
            
            Some((elem.id, elem.len))
        }
    }
}

/// VirtIO PCI Device (legacy interface)
pub struct VirtioPciDevice {
    /// PCI device
    pub pci_dev: PciDevice,
    
    /// I/O port base (BAR0)
    pub io_base: u16,
    
    /// Device type
    pub device_type: DeviceType,
    
    /// Device features
    pub device_features: u64,
    
    /// Driver features
    pub driver_features: u64,
    
    /// VirtQueues
    pub queues: Vec<Option<VirtQueue>>,
}

impl VirtioPciDevice {
    /// Create from PCI device
    pub fn from_pci(pci_dev: PciDevice) -> Result<Self, &'static str> {
        // VirtIO devices: vendor 0x1AF4, device 0x1000-0x103F
        if pci_dev.vendor_id != 0x1AF4 {
            return Err("Not a VirtIO device");
        }
        
        // Get I/O port base from BAR0
        let io_base = match &pci_dev.bars[0] {
            Some(BarType::Io { address, .. }) => *address,
            _ => return Err("BAR0 is not I/O port"),
        };
        
        let device_id = pci_dev.device_id;
        let device_type = match device_id {
            0x1000 => DeviceType::Network,
            0x1001 => DeviceType::Block,
            0x1002 => DeviceType::Balloon,
            0x1003 => DeviceType::Console,
            0x1004 => DeviceType::Entropy,
            0x1005 => DeviceType::ScsiHost,
            0x1009 => DeviceType::Transport9P,
            0x1010 => DeviceType::Gpu,
            0x1012 => DeviceType::Input,
            0x1013 => DeviceType::Socket,
            0x1014 => DeviceType::Crypto,
            _ => DeviceType::Reserved,
        };
        
        Ok(Self {
            pci_dev,
            io_base,
            device_type,
            device_features: 0,
            driver_features: 0,
            queues: Vec::new(),
        })
    }
    
    /// Reset device
    pub fn reset(&mut self) {
        self.write_status(0);
    }
    
    /// Read device features
    pub fn read_device_features(&mut self) -> u64 {
        let low = self.read_u32(8) as u64;
        let high = self.read_u32(12) as u64;
        self.device_features = low | (high << 32);
        self.device_features
    }
    
    /// Write driver features
    pub fn write_driver_features(&mut self, features: u64) {
        self.driver_features = features;
        self.write_u32(4, features as u32);
        self.write_u32(8, (features >> 32) as u32);
    }
    
    /// Read status register
    pub fn read_status(&self) -> u8 {
        self.read_u8(18)
    }
    
    /// Write status register
    pub fn write_status(&self, status: u8) {
        self.write_u8(18, status);
    }
    
    /// Initialize device
    pub fn init(&mut self) -> Result<(), &'static str> {
        // 1. Reset device
        self.reset();
        
        // 2. Set ACKNOWLEDGE status bit
        self.write_status(status::ACKNOWLEDGE);
        
        // 3. Set DRIVER status bit
        self.write_status(status::ACKNOWLEDGE | status::DRIVER);
        
        // 4. Read device features
        let device_features = self.read_device_features();
        
        crate::logger::info(&alloc::format!(
            "[VirtIO] Device features: {:#x}",
            device_features
        ));
        
        // 5. Negotiate features (select subset)
        let driver_features = device_features & (
            features::VERSION_1 |
            features::RING_INDIRECT_DESC |
            features::RING_EVENT_IDX
        );
        
        self.write_driver_features(driver_features);
        
        // 6. Set FEATURES_OK status bit
        self.write_status(status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK);
        
        // 7. Re-read status to confirm features accepted
        let status = self.read_status();
        if (status & status::FEATURES_OK) == 0 {
            self.write_status(status::FAILED);
            return Err("Device did not accept features");
        }
        
        Ok(())
    }
    
    /// Finalize device initialization
    pub fn finalize(&mut self) {
        let mut status = self.read_status();
        status |= status::DRIVER_OK;
        self.write_status(status);
    }
    
    /// Read 8-bit register
    fn read_u8(&self, offset: u16) -> u8 {
        unsafe {
            use core::arch::asm;
            
            let port = self.io_base + offset;
            let mut value: u8;
            
            asm!(
                "in al, dx",
                in("dx") port,
                out("al") value,
                options(nostack, preserves_flags)
            );
            
            value
        }
    }
    
    /// Write 8-bit register
    fn write_u8(&self, offset: u16, value: u8) {
        unsafe {
            use core::arch::asm;
            
            let port = self.io_base + offset;
            
            asm!(
                "out dx, al",
                in("dx") port,
                in("al") value,
                options(nostack, preserves_flags)
            );
        }
    }
    
    /// Read 16-bit register
    fn read_u16(&self, offset: u16) -> u16 {
        unsafe {
            use core::arch::asm;
            
            let port = self.io_base + offset;
            let mut value: u16;
            
            asm!(
                "in ax, dx",
                in("dx") port,
                out("ax") value,
                options(nostack, preserves_flags)
            );
            
            value
        }
    }
    
    /// Write 16-bit register
    fn write_u16(&self, offset: u16, value: u16) {
        unsafe {
            use core::arch::asm;
            
            let port = self.io_base + offset;
            
            asm!(
                "out dx, ax",
                in("dx") port,
                in("ax") value,
                options(nostack, preserves_flags)
            );
        }
    }
    
    /// Read 32-bit register
    fn read_u32(&self, offset: u16) -> u32 {
        unsafe {
            use core::arch::asm;
            
            let port = self.io_base + offset;
            let mut value: u32;
            
            asm!(
                "in eax, dx",
                in("dx") port,
                out("eax") value,
                options(nostack, preserves_flags)
            );
            
            value
        }
    }
    
    /// Write 32-bit register
    fn write_u32(&self, offset: u16, value: u32) {
        unsafe {
            use core::arch::asm;
            
            let port = self.io_base + offset;
            
            asm!(
                "out dx, eax",
                in("dx") port,
                in("eax") value,
                options(nostack, preserves_flags)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_virtq_desc_size() {
        assert_eq!(core::mem::size_of::<VirtqDesc>(), 16);
    }
    
    #[test]
    fn test_virtq_used_elem_size() {
        assert_eq!(core::mem::size_of::<VirtqUsedElem>(), 8);
    }
}
