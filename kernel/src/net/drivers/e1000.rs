//! Intel e1000/e1000e Gigabit Ethernet Driver
//!
//! Driver for Intel PRO/1000 family of network adapters.
//! Widely used in virtual machines and physical hardware.
//!
//! ## Supported Devices
//! - 82540EM/EP (e1000)
//! - 82541/82547 (e1000)
//! - 82571/82572 (e1000e)
//! - 82573/82574 (e1000e)
//!
//! ## Features
//! - Hardware checksum offload
//! - TCP segmentation offload (TSO)
//! - Receive-side scaling (RSS)
//! - Interrupt mitigation

use super::{NetworkDevice, DeviceCapabilities, DeviceStats, DriverError, DriverResult};
use alloc::vec::Vec;
use alloc::sync::Arc;
use crate::sync::SpinLock;

/// Intel e1000 PCI IDs
pub mod pci_ids {
    pub const VENDOR_INTEL: u16 = 0x8086;
    pub const DEVICE_82540EM: u16 = 0x100E;
    pub const DEVICE_82545EM: u16 = 0x100F;
    pub const DEVICE_82574L: u16 = 0x10D3;
}

/// e1000 Register offsets
pub mod regs {
    pub const CTRL: u32 = 0x0000;      // Device Control
    pub const STATUS: u32 = 0x0008;    // Device Status
    pub const EECD: u32 = 0x0010;      // EEPROM Control
    pub const CTRL_EXT: u32 = 0x0018;  // Extended Control
    pub const MDIC: u32 = 0x0020;      // MDI Control
    pub const ICR: u32 = 0x00C0;       // Interrupt Cause Read
    pub const IMS: u32 = 0x00D0;       // Interrupt Mask Set
    pub const IMC: u32 = 0x00D8;       // Interrupt Mask Clear
    pub const RCTL: u32 = 0x0100;      // Receive Control
    pub const TCTL: u32 = 0x0400;      // Transmit Control
    pub const RDBAL: u32 = 0x2800;     // RX Descriptor Base Low
    pub const RDBAH: u32 = 0x2804;     // RX Descriptor Base High
    pub const RDLEN: u32 = 0x2808;     // RX Descriptor Length
    pub const RDH: u32 = 0x2810;       // RX Descriptor Head
    pub const RDT: u32 = 0x2818;       // RX Descriptor Tail
    pub const TDBAL: u32 = 0x3800;     // TX Descriptor Base Low
    pub const TDBAH: u32 = 0x3804;     // TX Descriptor Base High
    pub const TDLEN: u32 = 0x3808;     // TX Descriptor Length
    pub const TDH: u32 = 0x3810;       // TX Descriptor Head
    pub const TDT: u32 = 0x3818;       // TX Descriptor Tail
    pub const RAL: u32 = 0x5400;       // Receive Address Low
    pub const RAH: u32 = 0x5404;       // Receive Address High
}

/// Control register bits
pub mod ctrl {
    pub const FD: u32 = 1 << 0;        // Full Duplex
    pub const ASDE: u32 = 1 << 5;      // Auto Speed Detection
    pub const SLU: u32 = 1 << 6;       // Set Link Up
    pub const RST: u32 = 1 << 26;      // Device Reset
    pub const PHY_RST: u32 = 1 << 31;  // PHY Reset
}

/// Receive Control register bits
pub mod rctl {
    pub const EN: u32 = 1 << 1;        // Enable
    pub const UPE: u32 = 1 << 3;       // Unicast Promiscuous
    pub const MPE: u32 = 1 << 4;       // Multicast Promiscuous
    pub const BAM: u32 = 1 << 15;      // Broadcast Accept Mode
    pub const BSIZE_2048: u32 = 0 << 16;  // Buffer size 2048
    pub const SECRC: u32 = 1 << 26;    // Strip Ethernet CRC
}

/// Transmit Control register bits
pub mod tctl {
    pub const EN: u32 = 1 << 1;        // Enable
    pub const PSP: u32 = 1 << 3;       // Pad Short Packets
}

/// RX Descriptor
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct RxDescriptor {
    pub addr: u64,
    pub length: u16,
    pub checksum: u16,
    pub status: u8,
    pub errors: u8,
    pub special: u16,
}

/// TX Descriptor
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct TxDescriptor {
    pub addr: u64,
    pub length: u16,
    pub cso: u8,
    pub cmd: u8,
    pub status: u8,
    pub css: u8,
    pub special: u16,
}

/// e1000 driver
pub struct E1000Driver {
    capabilities: DeviceCapabilities,
    stats: SpinLock<DeviceStats>,
    // TODO: Add MMIO base address, descriptor rings, etc.
}

impl E1000Driver {
    /// Create a new e1000 driver
    pub fn new(mac_address: [u8; 6]) -> Self {
        let mut caps = DeviceCapabilities::default();
        caps.mac_address = mac_address;
        caps.max_mtu = 1500;
        caps.checksum_offload = true;
        caps.tso_offload = true;
        caps.scatter_gather = true;
        
        Self {
            capabilities: caps,
            stats: SpinLock::new(DeviceStats::default()),
        }
    }
    
    /// Probe for e1000 devices
    pub fn probe() -> Option<Self> {
        // TODO: Implement PCI device scanning for e1000
        // For now, return None (device not found)
        None
    }
    
    /// Read MAC address from EEPROM
    fn read_mac_address(&self) -> [u8; 6] {
        // TODO: Implement EEPROM reading
        [0x52, 0x54, 0x00, 0x12, 0x34, 0x56] // QEMU default
    }
    
    /// Reset device
    fn reset(&self) -> DriverResult<()> {
        // TODO: Write to CTRL register to reset
        Ok(())
    }
    
    /// Initialize RX ring
    fn init_rx(&self) -> DriverResult<()> {
        // TODO: Allocate descriptor ring, set RDBAL/RDBAH/RDLEN
        Ok(())
    }
    
    /// Initialize TX ring
    fn init_tx(&self) -> DriverResult<()> {
        // TODO: Allocate descriptor ring, set TDBAL/TDBAH/TDLEN
        Ok(())
    }
}

impl NetworkDevice for E1000Driver {
    fn name(&self) -> &str {
        "eth0"
    }
    
    fn capabilities(&self) -> &DeviceCapabilities {
        &self.capabilities
    }
    
    fn stats(&self) -> DeviceStats {
        *self.stats.lock()
    }
    
    fn send(&self, packet: &[u8]) -> Result<(), DriverError> {
        // TODO: Add packet to TX ring, update TDT register
        let mut stats = self.stats.lock();
        stats.tx_packets += 1;
        stats.tx_bytes += packet.len() as u64;
        Ok(())
    }
    
    fn receive(&self) -> Result<Vec<Vec<u8>>, DriverError> {
        // TODO: Check RX ring for received packets
        Ok(Vec::new())
    }
    
    fn set_promiscuous(&self, enabled: bool) -> Result<(), DriverError> {
        // TODO: Set/clear RCTL.UPE and RCTL.MPE bits
        Ok(())
    }
    
    fn set_mac_address(&self, mac: [u8; 6]) -> Result<(), DriverError> {
        // TODO: Write to RAL/RAH registers
        Ok(())
    }
}

/// Initialize e1000 driver
pub fn init() -> DriverResult<()> {
    log::debug!("Probing for Intel e1000 devices...");
    
    if let Some(driver) = E1000Driver::probe() {
        let driver = Arc::new(driver);
        super::DEVICE_REGISTRY.register(driver);
        log::info!("Intel e1000 device initialized");
        Ok(())
    } else {
        log::debug!("No Intel e1000 devices found");
        Err(DriverError::DeviceNotReady)
    }
}
