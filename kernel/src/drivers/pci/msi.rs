//! PCI MSI (Message Signaled Interrupts) Support
//!
//! Provides MSI and MSI-X configuration for PCI devices.
//! MSI allows devices to trigger interrupts by writing to memory,
//! eliminating the need for shared IRQ lines and improving performance.

use super::{PciDevice, PciBus, PCI_BUS};
use crate::drivers::{DriverError, DriverResult};
use core::sync::atomic::{AtomicU8, Ordering};

/// MSI Capability ID in PCI configuration space
pub const MSI_CAP_ID: u8 = 0x05;

/// MSI-X Capability ID in PCI configuration space
pub const MSIX_CAP_ID: u8 = 0x11;

/// MSI Message Address Register (x86_64)
/// Format: 0xFEE00000 | (destination_id << 12) | (redirection_hint << 3) | (destination_mode << 2)
pub const MSI_ADDRESS_BASE: u32 = 0xFEE00000;

/// Next available MSI vector (starting from 0x50 to avoid conflicts)
static NEXT_MSI_VECTOR: AtomicU8 = AtomicU8::new(0x50);

/// MSI Capability Structure
#[derive(Debug, Clone, Copy)]
pub struct MsiCapability {
    /// Offset in configuration space
    pub offset: u8,
    /// Message Control register
    pub control: u16,
    /// Message Address (lower 32 bits)
    pub address_lo: u32,
    /// Message Address (upper 32 bits, for 64-bit capable)
    pub address_hi: u32,
    /// Message Data
    pub data: u16,
    /// Mask bits (if per-vector masking supported)
    pub mask: u32,
    /// Pending bits
    pub pending: u32,
}

impl MsiCapability {
    /// Check if this MSI capability supports 64-bit addresses
    pub fn is_64bit(&self) -> bool {
        (self.control & (1 << 7)) != 0
    }

    /// Check if per-vector masking is supported
    pub fn has_masking(&self) -> bool {
        (self.control & (1 << 8)) != 0
    }

    /// Get number of vectors requested (Multiple Message Capable)
    pub fn vectors_capable(&self) -> u8 {
        let mmc = (self.control >> 1) & 0x07;
        1 << mmc // 2^mmc vectors
    }

    /// Get number of vectors enabled (Multiple Message Enable)
    pub fn vectors_enabled(&self) -> u8 {
        let mme = (self.control >> 4) & 0x07;
        1 << mme
    }
}

/// MSI-X Capability Structure
#[derive(Debug, Clone, Copy)]
pub struct MsixCapability {
    /// Offset in configuration space
    pub offset: u8,
    /// Message Control register
    pub control: u16,
    /// Table Offset and BAR Indicator
    pub table_offset_bir: u32,
    /// PBA (Pending Bit Array) Offset and BAR Indicator
    pub pba_offset_bir: u32,
}

impl MsixCapability {
    /// Get table size (number of entries)
    pub fn table_size(&self) -> u16 {
        (self.control & 0x7FF) + 1
    }

    /// Check if MSI-X is enabled
    pub fn is_enabled(&self) -> bool {
        (self.control & (1 << 15)) != 0
    }

    /// Check if function is masked
    pub fn is_masked(&self) -> bool {
        (self.control & (1 << 14)) != 0
    }

    /// Get BAR index for table
    pub fn table_bar(&self) -> u8 {
        (self.table_offset_bir & 0x07) as u8
    }

    /// Get table offset within BAR
    pub fn table_offset(&self) -> u32 {
        self.table_offset_bir & !0x07
    }

    /// Get BAR index for PBA
    pub fn pba_bar(&self) -> u8 {
        (self.pba_offset_bir & 0x07) as u8
    }

    /// Get PBA offset within BAR
    pub fn pba_offset(&self) -> u32 {
        self.pba_offset_bir & !0x07
    }
}

impl PciDevice {
    /// Find a specific capability by ID
    pub fn find_capability(&self, cap_id: u8) -> Option<u8> {
        let bus = PCI_BUS.lock();
        
        // Check if device has capabilities (Status register bit 4)
        let status = bus.read_config_word(self.bus, self.device, self.function, 0x06);
        if (status & (1 << 4)) == 0 {
            return None;
        }
        
        // Capabilities pointer is at offset 0x34
        let mut cap_ptr = bus.read_config_byte(self.bus, self.device, self.function, 0x34);
        
        // Traverse capability list
        while cap_ptr != 0 && cap_ptr != 0xFF {
            let cap_id_found = bus.read_config_byte(self.bus, self.device, self.function, cap_ptr);
            
            if cap_id_found == cap_id {
                return Some(cap_ptr);
            }
            
            // Next capability pointer is at offset+1
            cap_ptr = bus.read_config_byte(self.bus, self.device, self.function, cap_ptr + 1);
        }
        
        None
    }
    
    /// Read MSI capability structure
    pub fn read_msi_capability(&self) -> Option<MsiCapability> {
        let cap_offset = self.find_capability(MSI_CAP_ID)?;
        let bus = PCI_BUS.lock();
        
        let control = bus.read_config_word(self.bus, self.device, self.function, cap_offset + 2);
        let address_lo = bus.read_config_dword(self.bus, self.device, self.function, cap_offset + 4);
        
        let is_64bit = (control & (1 << 7)) != 0;
        let has_masking = (control & (1 << 8)) != 0;
        
        let (address_hi, data_offset) = if is_64bit {
            let hi = bus.read_config_dword(self.bus, self.device, self.function, cap_offset + 8);
            (hi, 12)
        } else {
            (0, 8)
        };
        
        let data = bus.read_config_word(self.bus, self.device, self.function, cap_offset + data_offset);
        
        let (mask, pending) = if has_masking {
            let mask_offset = data_offset + 4;
            let m = bus.read_config_dword(self.bus, self.device, self.function, cap_offset + mask_offset);
            let p = bus.read_config_dword(self.bus, self.device, self.function, cap_offset + mask_offset + 4);
            (m, p)
        } else {
            (0, 0)
        };
        
        Some(MsiCapability {
            offset: cap_offset,
            control,
            address_lo,
            address_hi,
            data,
            mask,
            pending,
        })
    }
    
    /// Read MSI-X capability structure
    pub fn read_msix_capability(&self) -> Option<MsixCapability> {
        let cap_offset = self.find_capability(MSIX_CAP_ID)?;
        let bus = PCI_BUS.lock();
        
        let control = bus.read_config_word(self.bus, self.device, self.function, cap_offset + 2);
        let table_offset_bir = bus.read_config_dword(self.bus, self.device, self.function, cap_offset + 4);
        let pba_offset_bir = bus.read_config_dword(self.bus, self.device, self.function, cap_offset + 8);
        
        Some(MsixCapability {
            offset: cap_offset,
            control,
            table_offset_bir,
            pba_offset_bir,
        })
    }
    
    /// Enable MSI for this device with a single vector
    pub fn enable_msi(&mut self) -> DriverResult<u8> {
        let cap = self.read_msi_capability()
            .ok_or(DriverError::NotSupported)?;
        
        // Allocate vector
        let vector = NEXT_MSI_VECTOR.fetch_add(1, Ordering::SeqCst);
        if vector >= 0xF0 {
            // Vectors 0xF0-0xFF are reserved for IPIs
            return Err(DriverError::ResourceBusy);
        }
        
        let bus = PCI_BUS.lock();
        
        // Get current CPU APIC ID for targeting
        let apic_id = unsafe {
            use x86_64::registers::model_specific::Msr;
            let apic_base_msr = Msr::new(0x1B);
            let apic_base = apic_base_msr.read();
            // In x2APIC mode, read APIC ID from MSR 0x802
            if apic_base & (1 << 10) != 0 {
                let apic_id_msr = Msr::new(0x802);
                apic_id_msr.read() as u32
            } else {
                0 // Use BSP for now
            }
        };
        
        // Configure MSI Message Address
        // Format: 0xFEE[destination_id]XXX
        let address = MSI_ADDRESS_BASE | ((apic_id & 0xFF) << 12);
        
        bus.write_config_dword(self.bus, self.device, self.function, cap.offset + 4, address);
        
        // If 64-bit capable, write upper address (always 0 for x86_64)
        if cap.is_64bit() {
            bus.write_config_dword(self.bus, self.device, self.function, cap.offset + 8, 0);
        }
        
        // Configure MSI Message Data
        // Format: [trigger_mode:1][level:1][reserved:3][delivery_mode:3][vector:8]
        // delivery_mode = 000 (Fixed), trigger_mode = 0 (Edge), level = 0 (Deassert)
        let data = vector as u16;
        let data_offset = if cap.is_64bit() { 12 } else { 8 };
        
        bus.write_config_word(self.bus, self.device, self.function, cap.offset + data_offset, data);
        
        // Enable MSI (set bit 0 of control register)
        let mut control = cap.control;
        control |= 1; // MSI Enable
        control &= !(0x07 << 4); // Clear Multiple Message Enable (use only 1 vector)
        
        bus.write_config_word(self.bus, self.device, self.function, cap.offset + 2, control);
        
        log::info!("PCI MSI: Enabled MSI for device {:02x}:{:02x}.{} with vector {:#x}",
                   self.bus, self.device, self.function, vector);
        
        Ok(vector)
    }
    
    /// Enable MSI-X for this device
    pub fn enable_msix(&mut self, vectors: &[(u8, u64, u32)]) -> DriverResult<()> {
        let cap = self.read_msix_capability()
            .ok_or(DriverError::NotSupported)?;
        
        if vectors.len() > cap.table_size() as usize {
            return Err(DriverError::InvalidParameter);
        }
        
        // TODO: Map MSI-X table BAR and configure entries
        // This requires memory mapping which is not yet fully implemented
        
        log::warn!("MSI-X support not yet implemented");
        Err(DriverError::NotSupported)
    }
    
    /// Disable MSI for this device
    pub fn disable_msi(&mut self) -> DriverResult<()> {
        let cap = self.read_msi_capability()
            .ok_or(DriverError::NotSupported)?;
        
        let bus = PCI_BUS.lock();
        
        // Clear MSI Enable bit
        let mut control = bus.read_config_word(self.bus, self.device, self.function, cap.offset + 2);
        control &= !1;
        
        bus.write_config_word(self.bus, self.device, self.function, cap.offset + 2, control);
        
        log::info!("PCI MSI: Disabled MSI for device {:02x}:{:02x}.{}",
                   self.bus, self.device, self.function);
        
        Ok(())
    }
}

/// Allocate a new MSI vector
pub fn allocate_msi_vector() -> Option<u8> {
    let vector = NEXT_MSI_VECTOR.fetch_add(1, Ordering::SeqCst);
    if vector < 0xF0 {
        Some(vector)
    } else {
        None
    }
}

/// Check if a device supports MSI
pub fn supports_msi(device: &PciDevice) -> bool {
    device.find_capability(MSI_CAP_ID).is_some()
}

/// Check if a device supports MSI-X
pub fn supports_msix(device: &PciDevice) -> bool {
    device.find_capability(MSIX_CAP_ID).is_some()
}
