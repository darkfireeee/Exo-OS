//! Intel E1000 Network Driver
//!
//! Driver for Intel 82540EM Gigabit Ethernet Controller (E1000)
//! Common in QEMU/VirtualBox emulation
//!
//! PCI Device IDs:
//! - 0x8086:0x100E - E1000 (82540EM)
//! - 0x8086:0x100F - E1000 (82545EM)
//! - 0x8086:0x1004 - Intel Pro/1000 MT Desktop

use super::NetworkDriver;
use crate::drivers::pci::{PciDevice, PciBar, ids, PCI_BUS};
use alloc::vec::Vec;
use alloc::boxed::Box;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

/// E1000 Register offsets
#[allow(dead_code)]
mod regs {
    pub const CTRL: u32 = 0x0000;      // Device Control
    pub const STATUS: u32 = 0x0008;    // Device Status
    pub const EECD: u32 = 0x0010;      // EEPROM/Flash Control
    pub const EERD: u32 = 0x0014;      // EEPROM Read
    pub const CTRL_EXT: u32 = 0x0018;  // Extended Device Control
    pub const ICR: u32 = 0x00C0;       // Interrupt Cause Read
    pub const ITR: u32 = 0x00C4;       // Interrupt Throttling Rate
    pub const ICS: u32 = 0x00C8;       // Interrupt Cause Set
    pub const IMS: u32 = 0x00D0;       // Interrupt Mask Set
    pub const IMC: u32 = 0x00D8;       // Interrupt Mask Clear
    pub const RCTL: u32 = 0x0100;      // RX Control
    pub const RDBAL: u32 = 0x2800;     // RX Descriptor Base Address Low
    pub const RDBAH: u32 = 0x2804;     // RX Descriptor Base Address High
    pub const RDLEN: u32 = 0x2808;     // RX Descriptor Length
    pub const RDH: u32 = 0x2810;       // RX Descriptor Head
    pub const RDT: u32 = 0x2818;       // RX Descriptor Tail
    pub const TCTL: u32 = 0x0400;      // TX Control
    pub const TDBAL: u32 = 0x3800;     // TX Descriptor Base Address Low
    pub const TDBAH: u32 = 0x3804;     // TX Descriptor Base Address High
    pub const TDLEN: u32 = 0x3808;     // TX Descriptor Length
    pub const TDH: u32 = 0x3810;       // TX Descriptor Head
    pub const TDT: u32 = 0x3818;       // TX Descriptor Tail
    pub const MTA: u32 = 0x5200;       // Multicast Table Array
    pub const RAL0: u32 = 0x5400;      // Receive Address Low
    pub const RAH0: u32 = 0x5404;      // Receive Address High
}

/// E1000 Control Register bits
#[allow(dead_code)]
mod ctrl {
    pub const FD: u32 = 1 << 0;        // Full Duplex
    pub const LRST: u32 = 1 << 3;      // Link Reset
    pub const ASDE: u32 = 1 << 5;      // Auto-Speed Detection Enable
    pub const SLU: u32 = 1 << 6;       // Set Link Up
    pub const ILOS: u32 = 1 << 7;      // Invert Loss of Signal
    pub const RST: u32 = 1 << 26;      // Device Reset
    pub const VME: u32 = 1 << 30;      // VLAN Mode Enable
    pub const PHY_RST: u32 = 1 << 31;  // PHY Reset
}

/// E1000 RCTL (Receive Control) bits
#[allow(dead_code)]
mod rctl {
    pub const EN: u32 = 1 << 1;        // Receiver Enable
    pub const SBP: u32 = 1 << 2;       // Store Bad Packets
    pub const UPE: u32 = 1 << 3;       // Unicast Promiscuous Enable
    pub const MPE: u32 = 1 << 4;       // Multicast Promiscuous Enable
    pub const LPE: u32 = 1 << 5;       // Long Packet Enable
    pub const LBM_NONE: u32 = 0 << 6;  // No Loopback
    pub const RDMTS_HALF: u32 = 0 << 8; // RX Descriptor Minimum Threshold Size
    pub const BAM: u32 = 1 << 15;      // Broadcast Accept Mode
    pub const BSIZE_2048: u32 = 0 << 16; // Buffer Size 2048
    pub const BSIZE_4096: u32 = 3 << 16; // Buffer Size 4096
    pub const SECRC: u32 = 1 << 26;    // Strip Ethernet CRC
}

/// E1000 TCTL (Transmit Control) bits
#[allow(dead_code)]
mod tctl {
    pub const EN: u32 = 1 << 1;        // Transmit Enable
    pub const PSP: u32 = 1 << 3;       // Pad Short Packets
    pub const CT_SHIFT: u32 = 4;       // Collision Threshold
    pub const COLD_SHIFT: u32 = 12;    // Collision Distance
    pub const SWXOFF: u32 = 1 << 22;   // Software XOFF Transmission
    pub const RTLC: u32 = 1 << 24;     // Re-transmit on Late Collision
}

/// E1000 Interrupt bits
#[allow(dead_code)]
mod intr {
    pub const TXDW: u32 = 1 << 0;      // TX Descriptor Written Back
    pub const TXQE: u32 = 1 << 1;      // TX Queue Empty
    pub const LSC: u32 = 1 << 2;       // Link Status Change
    pub const RXSEQ: u32 = 1 << 3;     // RX Sequence Error
    pub const RXDMT0: u32 = 1 << 4;    // RX Descriptor Minimum Threshold
    pub const RXO: u32 = 1 << 6;       // RX Overrun
    pub const RXT0: u32 = 1 << 7;      // RX Timer Interrupt
}

/// TX Descriptor
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy)]
pub struct TxDesc {
    pub addr: u64,
    pub length: u16,
    pub cso: u8,
    pub cmd: u8,
    pub status: u8,
    pub css: u8,
    pub special: u16,
}

impl Default for TxDesc {
    fn default() -> Self {
        Self {
            addr: 0,
            length: 0,
            cso: 0,
            cmd: 0,
            status: 0,
            css: 0,
            special: 0,
        }
    }
}

/// TX command bits
#[allow(dead_code)]
mod tx_cmd {
    pub const EOP: u8 = 1 << 0;       // End of Packet
    pub const IFCS: u8 = 1 << 1;      // Insert FCS
    pub const IC: u8 = 1 << 2;        // Insert Checksum
    pub const RS: u8 = 1 << 3;        // Report Status
    pub const RPS: u8 = 1 << 4;       // Report Packet Sent
    pub const DEXT: u8 = 1 << 5;      // Descriptor Extension
    pub const VLE: u8 = 1 << 6;       // VLAN Packet Enable
    pub const IDE: u8 = 1 << 7;       // Interrupt Delay Enable
}

/// TX status bits
#[allow(dead_code)]
mod tx_status {
    pub const DD: u8 = 1 << 0;        // Descriptor Done
    pub const EC: u8 = 1 << 1;        // Excess Collisions
    pub const LC: u8 = 1 << 2;        // Late Collision
    pub const TU: u8 = 1 << 3;        // Transmit Underrun
}

/// RX Descriptor
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy)]
pub struct RxDesc {
    pub addr: u64,
    pub length: u16,
    pub checksum: u16,
    pub status: u8,
    pub errors: u8,
    pub special: u16,
}

impl Default for RxDesc {
    fn default() -> Self {
        Self {
            addr: 0,
            length: 0,
            checksum: 0,
            status: 0,
            errors: 0,
            special: 0,
        }
    }
}

/// RX status bits
#[allow(dead_code)]
mod rx_status {
    pub const DD: u8 = 1 << 0;        // Descriptor Done
    pub const EOP: u8 = 1 << 1;       // End of Packet
    pub const IXSM: u8 = 1 << 2;      // Ignore Checksum Indication
    pub const VP: u8 = 1 << 3;        // Packet is 802.1Q
    pub const TCPCS: u8 = 1 << 5;     // TCP Checksum Calculated
    pub const IPCS: u8 = 1 << 6;      // IP Checksum Calculated
    pub const PIF: u8 = 1 << 7;       // Passed In-exact Filter
}

/// Number of descriptors
const NUM_RX_DESC: usize = 32;
const NUM_TX_DESC: usize = 32;
const RX_BUFFER_SIZE: usize = 2048;

/// E1000 driver structure
pub struct E1000Driver {
    /// MMIO base address
    mmio_base: usize,
    /// MAC address
    mac: [u8; 6],
    /// PCI device info
    pci_device: Option<PciDevice>,
    /// RX descriptors
    rx_descs: Vec<RxDesc>,
    /// TX descriptors  
    tx_descs: Vec<TxDesc>,
    /// RX buffers
    rx_buffers: Vec<Box<[u8; RX_BUFFER_SIZE]>>,
    /// TX buffers
    tx_buffers: Vec<Box<[u8; RX_BUFFER_SIZE]>>,
    /// Current RX index
    rx_cur: usize,
    /// Current TX index
    tx_cur: usize,
    /// Is driver initialized
    initialized: AtomicBool,
}

impl E1000Driver {
    pub const fn new() -> Self {
        Self {
            mmio_base: 0,
            mac: [0; 6],
            pci_device: None,
            rx_descs: Vec::new(),
            tx_descs: Vec::new(),
            rx_buffers: Vec::new(),
            tx_buffers: Vec::new(),
            rx_cur: 0,
            tx_cur: 0,
            initialized: AtomicBool::new(false),
        }
    }

    pub fn with_base(base_addr: usize) -> Self {
        Self {
            mmio_base: base_addr,
            mac: [0; 6],
            pci_device: None,
            rx_descs: Vec::new(),
            tx_descs: Vec::new(),
            rx_buffers: Vec::new(),
            tx_buffers: Vec::new(),
            rx_cur: 0,
            tx_cur: 0,
            initialized: AtomicBool::new(false),
        }
    }
    
    /// Detect E1000 cards via PCI
    pub fn detect() -> Option<Self> {
        let pci_bus = PCI_BUS.lock();
        
        // Search for E1000 variants
        let device_ids = [ids::INTEL_E1000, ids::INTEL_E1000_82545EM, ids::INTEL_PRO1000_MT];
        
        for id in device_ids {
            if let Some(device) = pci_bus.find_device(id) {
                log::info!("Found E1000 at {} (vendor={:04X}, device={:04X})",
                    device.bdf(), device.vendor_id, device.device_id);
                
                // Get MMIO base from BAR0
                if let Some(mmio_base) = device.memory_base() {
                    log::info!("  MMIO base: 0x{:016X}", mmio_base);
                    
                    let mut driver = Self::new();
                    driver.mmio_base = mmio_base as usize;
                    driver.pci_device = Some(device.clone());
                    
                    return Some(driver);
                }
            }
        }
        
        None
    }
    
    /// Read MMIO register
    #[inline]
    fn read_reg(&self, reg: u32) -> u32 {
        unsafe {
            read_volatile((self.mmio_base + reg as usize) as *const u32)
        }
    }
    
    /// Write MMIO register
    #[inline]
    fn write_reg(&self, reg: u32, value: u32) {
        unsafe {
            write_volatile((self.mmio_base + reg as usize) as *mut u32, value);
        }
    }
    
    /// Read MAC address from EEPROM
    fn read_mac_from_eeprom(&mut self) {
        // Try reading from RAL0/RAH0 first (set by BIOS/firmware)
        let ral = self.read_reg(regs::RAL0);
        let rah = self.read_reg(regs::RAH0);
        
        if ral != 0 || (rah & 0xFFFF) != 0 {
            self.mac[0] = (ral >> 0) as u8;
            self.mac[1] = (ral >> 8) as u8;
            self.mac[2] = (ral >> 16) as u8;
            self.mac[3] = (ral >> 24) as u8;
            self.mac[4] = (rah >> 0) as u8;
            self.mac[5] = (rah >> 8) as u8;
            return;
        }
        
        // Read from EEPROM
        for i in 0..3 {
            let word = self.eeprom_read(i);
            self.mac[i as usize * 2] = (word & 0xFF) as u8;
            self.mac[i as usize * 2 + 1] = (word >> 8) as u8;
        }
    }
    
    /// Read word from EEPROM
    fn eeprom_read(&self, addr: u8) -> u16 {
        // Set read address and start bit
        self.write_reg(regs::EERD, ((addr as u32) << 8) | 1);
        
        // Wait for read to complete
        loop {
            let val = self.read_reg(regs::EERD);
            if val & (1 << 4) != 0 {
                return ((val >> 16) & 0xFFFF) as u16;
            }
        }
    }
    
    /// Reset the device
    fn reset(&mut self) {
        // Disable interrupts
        self.write_reg(regs::IMC, 0xFFFFFFFF);
        
        // Reset device
        let ctrl = self.read_reg(regs::CTRL);
        self.write_reg(regs::CTRL, ctrl | ctrl::RST);
        
        // Wait for reset to complete
        for _ in 0..1000 {
            if self.read_reg(regs::CTRL) & ctrl::RST == 0 {
                break;
            }
        }
        
        // Disable interrupts again after reset
        self.write_reg(regs::IMC, 0xFFFFFFFF);
        
        // Clear pending interrupts
        let _ = self.read_reg(regs::ICR);
    }
    
    /// Initialize RX descriptors and buffers
    fn init_rx(&mut self) {
        // Allocate RX descriptors
        self.rx_descs = (0..NUM_RX_DESC).map(|_| RxDesc::default()).collect();
        
        // Allocate RX buffers and set descriptor addresses
        for i in 0..NUM_RX_DESC {
            let buffer = Box::new([0u8; RX_BUFFER_SIZE]);
            let addr = buffer.as_ptr() as u64;
            self.rx_descs[i].addr = addr;
            self.rx_buffers.push(buffer);
        }
        
        // Set RX descriptor base address
        let rx_desc_addr = self.rx_descs.as_ptr() as u64;
        self.write_reg(regs::RDBAL, rx_desc_addr as u32);
        self.write_reg(regs::RDBAH, (rx_desc_addr >> 32) as u32);
        
        // Set RX descriptor length
        self.write_reg(regs::RDLEN, (NUM_RX_DESC * core::mem::size_of::<RxDesc>()) as u32);
        
        // Set head and tail
        self.write_reg(regs::RDH, 0);
        self.write_reg(regs::RDT, NUM_RX_DESC as u32 - 1);
        
        // Enable receiver
        self.write_reg(regs::RCTL,
            rctl::EN |
            rctl::BAM |           // Accept broadcast
            rctl::SECRC |         // Strip CRC
            rctl::BSIZE_2048 |
            rctl::LPE             // Accept long packets
        );
        
        self.rx_cur = 0;
    }
    
    /// Initialize TX descriptors and buffers
    fn init_tx(&mut self) {
        // Allocate TX descriptors
        self.tx_descs = (0..NUM_TX_DESC).map(|_| TxDesc::default()).collect();
        
        // Allocate TX buffers
        for i in 0..NUM_TX_DESC {
            let buffer = Box::new([0u8; RX_BUFFER_SIZE]);
            let addr = buffer.as_ptr() as u64;
            self.tx_descs[i].addr = addr;
            self.tx_descs[i].status = tx_status::DD; // Mark as done (available)
            self.tx_buffers.push(buffer);
        }
        
        // Set TX descriptor base address
        let tx_desc_addr = self.tx_descs.as_ptr() as u64;
        self.write_reg(regs::TDBAL, tx_desc_addr as u32);
        self.write_reg(regs::TDBAH, (tx_desc_addr >> 32) as u32);
        
        // Set TX descriptor length
        self.write_reg(regs::TDLEN, (NUM_TX_DESC * core::mem::size_of::<TxDesc>()) as u32);
        
        // Set head and tail
        self.write_reg(regs::TDH, 0);
        self.write_reg(regs::TDT, 0);
        
        // Enable transmitter
        self.write_reg(regs::TCTL,
            tctl::EN |
            tctl::PSP |                    // Pad short packets
            (15 << tctl::CT_SHIFT) |       // Collision threshold
            (64 << tctl::COLD_SHIFT) |     // Collision distance
            tctl::RTLC                     // Retransmit on late collision
        );
        
        self.tx_cur = 0;
    }
    
    /// Setup link
    fn setup_link(&self) {
        let ctrl = self.read_reg(regs::CTRL);
        self.write_reg(regs::CTRL,
            ctrl | ctrl::SLU | ctrl::ASDE | ctrl::FD
        );
    }
    
    /// Enable interrupts
    fn enable_interrupts(&self) {
        // Enable TX and RX interrupts
        self.write_reg(regs::IMS,
            intr::RXT0 |    // RX timer
            intr::RXDMT0 |  // RX descriptor min threshold
            intr::TXDW |    // TX descriptor written back
            intr::LSC       // Link status change
        );
    }
    
    /// Check if link is up
    pub fn link_up(&self) -> bool {
        self.read_reg(regs::STATUS) & (1 << 1) != 0
    }
}

impl NetworkDriver for E1000Driver {
    fn init(&mut self) -> Result<(), &'static str> {
        if self.initialized.load(Ordering::SeqCst) {
            return Ok(());
        }
        
        if self.mmio_base == 0 {
            return Err("E1000: No MMIO base configured");
        }
        
        // Enable PCI bus mastering and memory space
        if let Some(ref pci_dev) = self.pci_device {
            pci_dev.enable_bus_master();
            pci_dev.enable_memory();
        }
        
        // Reset device
        self.reset();
        
        // Read MAC address
        self.read_mac_from_eeprom();
        log::info!(
            "E1000 MAC: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            self.mac[0], self.mac[1], self.mac[2],
            self.mac[3], self.mac[4], self.mac[5]
        );
        
        // Initialize descriptor rings
        self.init_rx();
        self.init_tx();
        
        // Setup link
        self.setup_link();
        
        // Enable interrupts
        self.enable_interrupts();
        
        // Clear multicast table
        for i in 0..128 {
            self.write_reg(regs::MTA + i * 4, 0);
        }
        
        self.initialized.store(true, Ordering::SeqCst);
        
        log::info!("E1000: Initialized (link {})", 
            if self.link_up() { "UP" } else { "DOWN" });
        
        Ok(())
    }
    
    fn send(&mut self, data: &[u8]) -> Result<(), &'static str> {
        if !self.initialized.load(Ordering::SeqCst) {
            return Err("E1000: Not initialized");
        }
        
        if data.len() > 1518 {
            return Err("E1000: Packet too large");
        }
        
        // Check if descriptor is available
        if self.tx_descs[self.tx_cur].status & tx_status::DD == 0 {
            return Err("E1000: TX queue full");
        }
        
        // Copy data to TX buffer
        let buf = &mut self.tx_buffers[self.tx_cur];
        buf[..data.len()].copy_from_slice(data);
        
        // Setup descriptor
        self.tx_descs[self.tx_cur].length = data.len() as u16;
        self.tx_descs[self.tx_cur].cmd = tx_cmd::EOP | tx_cmd::IFCS | tx_cmd::RS;
        self.tx_descs[self.tx_cur].status = 0;
        
        // Update tail
        let old_cur = self.tx_cur;
        self.tx_cur = (self.tx_cur + 1) % NUM_TX_DESC;
        self.write_reg(regs::TDT, self.tx_cur as u32);
        
        log::trace!("E1000: Sent {} bytes (desc {})", data.len(), old_cur);
        
        Ok(())
    }
    
    fn receive(&mut self) -> Option<&[u8]> {
        if !self.initialized.load(Ordering::SeqCst) {
            return None;
        }
        
        // Check if packet received
        if self.rx_descs[self.rx_cur].status & rx_status::DD == 0 {
            return None;
        }
        
        // Check for errors
        if self.rx_descs[self.rx_cur].errors != 0 {
            log::warn!("E1000: RX error 0x{:02X}", self.rx_descs[self.rx_cur].errors);
            // Reset descriptor and continue
            self.rx_descs[self.rx_cur].status = 0;
            let old_cur = self.rx_cur;
            self.rx_cur = (self.rx_cur + 1) % NUM_RX_DESC;
            self.write_reg(regs::RDT, old_cur as u32);
            return None;
        }
        
        let len = self.rx_descs[self.rx_cur].length as usize;
        let data = &self.rx_buffers[self.rx_cur][..len];
        
        log::trace!("E1000: Received {} bytes (desc {})", len, self.rx_cur);
        
        // Note: Caller should call receive_done() after processing
        Some(data)
    }
    
    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }
}

impl E1000Driver {
    /// Signal that receive buffer has been processed
    pub fn receive_done(&mut self) {
        if !self.initialized.load(Ordering::SeqCst) {
            return;
        }
        
        // Reset descriptor
        self.rx_descs[self.rx_cur].status = 0;
        
        // Update tail to return buffer to hardware
        let old_cur = self.rx_cur;
        self.rx_cur = (self.rx_cur + 1) % NUM_RX_DESC;
        self.write_reg(regs::RDT, old_cur as u32);
    }
    
    /// Check if initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::SeqCst)
    }
}

/// Global E1000 driver instance
pub static E1000: Mutex<Option<E1000Driver>> = Mutex::new(None);

/// Initialize E1000 driver
pub fn init() -> bool {
    if let Some(mut driver) = E1000Driver::detect() {
        match driver.init() {
            Ok(_) => {
                *E1000.lock() = Some(driver);
                true
            }
            Err(e) => {
                log::error!("{}", e);
                false
            }
        }
    } else {
        log::debug!("E1000: No device found");
        false
    }
}
