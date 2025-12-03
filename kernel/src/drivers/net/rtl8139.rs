//! Realtek RTL8139 Network Driver
//!
//! Driver for Realtek RTL8139 Fast Ethernet Controller
//! Common in older hardware and emulation (QEMU -net nic,model=rtl8139)
//!
//! PCI Device ID: 0x10EC:0x8139

use super::NetworkDriver;
use crate::drivers::pci::{PciDevice, ids, PCI_BUS};
use alloc::vec::Vec;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use x86_64::instructions::port::{Port, PortWriteOnly, PortReadOnly};

/// RTL8139 Register offsets (I/O space)
#[allow(dead_code)]
mod regs {
    pub const MAC0: u16 = 0x00;         // MAC address (6 bytes)
    pub const MAR0: u16 = 0x08;         // Multicast filter (8 bytes)
    pub const TXSTATUS0: u16 = 0x10;    // TX status (4 descriptors * 4 bytes)
    pub const TXADDR0: u16 = 0x20;      // TX address (4 descriptors * 4 bytes)
    pub const RXBUF: u16 = 0x30;        // RX buffer start address
    pub const CMD: u16 = 0x37;          // Command register
    pub const CAPR: u16 = 0x38;         // Current Address of Packet Read
    pub const CBR: u16 = 0x3A;          // Current Buffer Address
    pub const IMR: u16 = 0x3C;          // Interrupt Mask Register
    pub const ISR: u16 = 0x3E;          // Interrupt Status Register
    pub const TCR: u16 = 0x40;          // Transmit Configuration
    pub const RCR: u16 = 0x44;          // Receive Configuration
    pub const CONFIG1: u16 = 0x52;      // Configuration 1
}

/// Command register bits
#[allow(dead_code)]
mod cmd {
    pub const BUFE: u8 = 1 << 0;        // Buffer Empty
    pub const TE: u8 = 1 << 2;          // Transmitter Enable
    pub const RE: u8 = 1 << 3;          // Receiver Enable
    pub const RST: u8 = 1 << 4;         // Reset
}

/// Interrupt bits
#[allow(dead_code)]
mod intr {
    pub const ROK: u16 = 1 << 0;        // Receive OK
    pub const RER: u16 = 1 << 1;        // Receive Error
    pub const TOK: u16 = 1 << 2;        // Transmit OK
    pub const TER: u16 = 1 << 3;        // Transmit Error
    pub const RXOVW: u16 = 1 << 4;      // RX Buffer Overflow
    pub const PUN: u16 = 1 << 5;        // Packet Underrun/Link Change
    pub const FOVW: u16 = 1 << 6;       // RX FIFO Overflow
    pub const LENCHG: u16 = 1 << 13;    // Cable Length Change
    pub const TIMEOUT: u16 = 1 << 14;   // Time Out
    pub const SERR: u16 = 1 << 15;      // System Error
}

/// Receive Configuration bits
#[allow(dead_code)]
mod rcr {
    pub const AAP: u32 = 1 << 0;        // Accept All Packets
    pub const APM: u32 = 1 << 1;        // Accept Physical Match
    pub const AM: u32 = 1 << 2;         // Accept Multicast
    pub const AB: u32 = 1 << 3;         // Accept Broadcast
    pub const AR: u32 = 1 << 4;         // Accept Runt (< 64 bytes)
    pub const AER: u32 = 1 << 5;        // Accept Error Packets
    pub const WRAP: u32 = 1 << 7;       // Wrap (ring buffer)
    pub const RBLEN_8K: u32 = 0 << 11;  // 8K + 16 byte buffer
    pub const RBLEN_16K: u32 = 1 << 11; // 16K + 16 byte buffer
    pub const RBLEN_32K: u32 = 2 << 11; // 32K + 16 byte buffer
    pub const RBLEN_64K: u32 = 3 << 11; // 64K + 16 byte buffer
}

/// Transmit Configuration bits
#[allow(dead_code)]
mod tcr {
    pub const CLRABT: u32 = 1 << 0;     // Clear Abort
    pub const TXRR: u32 = 0 << 4;       // TX Retry Count (16 + TXRR*16)
    pub const MXDMA_256: u32 = 4 << 8;  // Max DMA Burst 256 bytes
    pub const MXDMA_512: u32 = 5 << 8;  // Max DMA Burst 512 bytes
    pub const MXDMA_1K: u32 = 6 << 8;   // Max DMA Burst 1K bytes
    pub const MXDMA_2K: u32 = 7 << 8;   // Max DMA Burst 2K bytes
    pub const IFG_NORMAL: u32 = 3 << 24; // Normal Inter-Frame Gap
}

/// TX Status bits
#[allow(dead_code)]
mod tx_status {
    pub const OWN: u32 = 1 << 13;       // DMA operation completed
    pub const TUN: u32 = 1 << 14;       // TX FIFO Underrun
    pub const TOK: u32 = 1 << 15;       // Transmit OK
    pub const OWC: u32 = 1 << 29;       // Out of Window Collision
    pub const TABT: u32 = 1 << 30;      // Transmit Abort
    pub const CRS: u32 = 1 << 31;       // Carrier Sense Lost
}

/// RX buffer size (8K + 16 bytes + 1500 bytes for wrap)
const RX_BUF_SIZE: usize = 8192 + 16 + 1500;
/// TX buffer size
const TX_BUF_SIZE: usize = 1792;
/// Number of TX descriptors
const NUM_TX_DESC: usize = 4;

/// RTL8139 driver structure
pub struct Rtl8139Driver {
    /// I/O base port
    io_base: u16,
    /// MAC address
    mac: [u8; 6],
    /// PCI device info
    pci_device: Option<PciDevice>,
    /// RX buffer (ring buffer)
    rx_buffer: Box<[u8; RX_BUF_SIZE]>,
    /// TX buffers (4 descriptors)
    tx_buffers: [Box<[u8; TX_BUF_SIZE]>; NUM_TX_DESC],
    /// Current RX buffer position
    rx_cur: usize,
    /// Current TX descriptor
    tx_cur: usize,
    /// Is driver initialized
    initialized: AtomicBool,
}

impl Rtl8139Driver {
    /// Create uninitialized driver
    pub fn new() -> Self {
        Self {
            io_base: 0,
            mac: [0; 6],
            pci_device: None,
            rx_buffer: Box::new([0u8; RX_BUF_SIZE]),
            tx_buffers: [
                Box::new([0u8; TX_BUF_SIZE]),
                Box::new([0u8; TX_BUF_SIZE]),
                Box::new([0u8; TX_BUF_SIZE]),
                Box::new([0u8; TX_BUF_SIZE]),
            ],
            rx_cur: 0,
            tx_cur: 0,
            initialized: AtomicBool::new(false),
        }
    }

    pub fn with_base(base_addr: u16) -> Self {
        let mut driver = Self::new();
        driver.io_base = base_addr;
        driver
    }
    
    /// Detect RTL8139 cards via PCI
    pub fn detect() -> Option<Self> {
        let pci_bus = PCI_BUS.lock();
        
        if let Some(device) = pci_bus.find_device(ids::REALTEK_RTL8139) {
            log::info!("Found RTL8139 at {}", device.bdf());
            
            // RTL8139 uses I/O space (BAR0)
            if let Some(io_base) = device.io_base() {
                log::info!("  I/O base: 0x{:04X}", io_base);
                
                let mut driver = Self::new();
                driver.io_base = io_base as u16;
                driver.pci_device = Some(device.clone());
                
                return Some(driver);
            }
        }
        
        None
    }
    
    /// Read byte from I/O port
    #[inline]
    fn inb(&self, reg: u16) -> u8 {
        unsafe {
            let mut port: Port<u8> = Port::new(self.io_base + reg);
            port.read()
        }
    }
    
    /// Write byte to I/O port
    #[inline]
    fn outb(&self, reg: u16, value: u8) {
        unsafe {
            let mut port: Port<u8> = Port::new(self.io_base + reg);
            port.write(value);
        }
    }
    
    /// Read word from I/O port
    #[inline]
    fn inw(&self, reg: u16) -> u16 {
        unsafe {
            let mut port: Port<u16> = Port::new(self.io_base + reg);
            port.read()
        }
    }
    
    /// Write word to I/O port
    #[inline]
    fn outw(&self, reg: u16, value: u16) {
        unsafe {
            let mut port: Port<u16> = Port::new(self.io_base + reg);
            port.write(value);
        }
    }
    
    /// Read dword from I/O port
    #[inline]
    fn inl(&self, reg: u16) -> u32 {
        unsafe {
            let mut port: Port<u32> = Port::new(self.io_base + reg);
            port.read()
        }
    }
    
    /// Write dword to I/O port
    #[inline]
    fn outl(&self, reg: u16, value: u32) {
        unsafe {
            let mut port: Port<u32> = Port::new(self.io_base + reg);
            port.write(value);
        }
    }
    
    /// Read MAC address from device
    fn read_mac(&mut self) {
        for i in 0..6 {
            self.mac[i] = self.inb(regs::MAC0 + i as u16);
        }
    }
    
    /// Software reset
    fn reset(&self) {
        // Power on
        self.outb(regs::CONFIG1, 0x00);
        
        // Software reset
        self.outb(regs::CMD, cmd::RST);
        
        // Wait for reset to complete
        for _ in 0..1000 {
            if self.inb(regs::CMD) & cmd::RST == 0 {
                break;
            }
        }
    }
    
    /// Get TX status register offset for descriptor
    fn tx_status_reg(&self, desc: usize) -> u16 {
        regs::TXSTATUS0 + (desc as u16 * 4)
    }
    
    /// Get TX address register offset for descriptor
    fn tx_addr_reg(&self, desc: usize) -> u16 {
        regs::TXADDR0 + (desc as u16 * 4)
    }
}

impl NetworkDriver for Rtl8139Driver {
    fn init(&mut self) -> Result<(), &'static str> {
        if self.initialized.load(Ordering::SeqCst) {
            return Ok(());
        }
        
        if self.io_base == 0 {
            return Err("RTL8139: No I/O base configured");
        }
        
        // Enable PCI bus mastering
        if let Some(ref pci_dev) = self.pci_device {
            pci_dev.enable_bus_master();
            pci_dev.enable_io();
        }
        
        // Reset device
        self.reset();
        
        // Read MAC address
        self.read_mac();
        log::info!(
            "RTL8139 MAC: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            self.mac[0], self.mac[1], self.mac[2],
            self.mac[3], self.mac[4], self.mac[5]
        );
        
        // Set RX buffer address
        let rx_addr = self.rx_buffer.as_ptr() as u32;
        self.outl(regs::RXBUF, rx_addr);
        
        // Set TX buffer addresses
        for i in 0..NUM_TX_DESC {
            let tx_addr = self.tx_buffers[i].as_ptr() as u32;
            self.outl(self.tx_addr_reg(i), tx_addr);
        }
        
        // Enable TX/RX interrupts
        self.outw(regs::IMR, 
            intr::ROK | intr::TOK | intr::RER | intr::TER | intr::RXOVW
        );
        
        // Configure RX: Accept broadcast, multicast, physical match
        // Use 8K buffer + wrap mode
        self.outl(regs::RCR,
            rcr::APM | rcr::AM | rcr::AB | rcr::WRAP | rcr::RBLEN_8K
        );
        
        // Configure TX
        self.outl(regs::TCR, 
            tcr::IFG_NORMAL | tcr::MXDMA_2K
        );
        
        // Enable transmitter and receiver
        self.outb(regs::CMD, cmd::TE | cmd::RE);
        
        self.initialized.store(true, Ordering::SeqCst);
        
        log::info!("RTL8139: Initialized");
        Ok(())
    }
    
    fn send(&mut self, data: &[u8]) -> Result<(), &'static str> {
        if !self.initialized.load(Ordering::SeqCst) {
            return Err("RTL8139: Not initialized");
        }
        
        if data.len() > TX_BUF_SIZE {
            return Err("RTL8139: Packet too large");
        }
        
        // Check if current TX descriptor is available
        let status = self.inl(self.tx_status_reg(self.tx_cur));
        if status & tx_status::OWN == 0 {
            // Still owned by hardware, wait or fail
            return Err("RTL8139: TX descriptor busy");
        }
        
        // Copy data to TX buffer
        self.tx_buffers[self.tx_cur][..data.len()].copy_from_slice(data);
        
        // Set TX status: size + clear status bits
        // RTL8139 starts transmission when we write to TSD
        let tx_cmd = data.len() as u32;
        self.outl(self.tx_status_reg(self.tx_cur), tx_cmd);
        
        log::trace!("RTL8139: Sent {} bytes (desc {})", data.len(), self.tx_cur);
        
        // Move to next descriptor
        self.tx_cur = (self.tx_cur + 1) % NUM_TX_DESC;
        
        Ok(())
    }
    
    fn receive(&mut self) -> Option<&[u8]> {
        if !self.initialized.load(Ordering::SeqCst) {
            return None;
        }
        
        // Check if buffer is empty
        let cmd = self.inb(regs::CMD);
        if cmd & cmd::BUFE != 0 {
            return None;
        }
        
        // Read packet header (4 bytes: status(2) + length(2))
        let header_offset = self.rx_cur;
        let status = u16::from_le_bytes([
            self.rx_buffer[header_offset],
            self.rx_buffer[header_offset + 1],
        ]);
        let length = u16::from_le_bytes([
            self.rx_buffer[header_offset + 2],
            self.rx_buffer[header_offset + 3],
        ]) as usize;
        
        // Check for receive OK
        if status & 0x0001 == 0 {
            log::warn!("RTL8139: RX error status 0x{:04X}", status);
            // Skip this packet
            self.rx_cur = (self.rx_cur + length + 4 + 3) & !3;
            self.rx_cur %= 8192; // Wrap at 8K
            self.outw(regs::CAPR, (self.rx_cur as u16).wrapping_sub(0x10));
            return None;
        }
        
        // Return pointer to packet data (skip 4-byte header)
        let data_offset = header_offset + 4;
        let data = &self.rx_buffer[data_offset..data_offset + length - 4]; // -4 for CRC
        
        log::trace!("RTL8139: Received {} bytes", length - 4);
        
        // Note: Caller should call receive_done() after processing
        Some(data)
    }
    
    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }
}

impl Rtl8139Driver {
    /// Signal that receive buffer has been processed
    pub fn receive_done(&mut self) {
        if !self.initialized.load(Ordering::SeqCst) {
            return;
        }
        
        // Get length from header
        let header_offset = self.rx_cur;
        let length = u16::from_le_bytes([
            self.rx_buffer[header_offset + 2],
            self.rx_buffer[header_offset + 3],
        ]) as usize;
        
        // Align to dword boundary and advance
        self.rx_cur = (self.rx_cur + length + 4 + 3) & !3;
        self.rx_cur %= 8192; // Wrap at 8K
        
        // Update CAPR (must be 0x10 less than actual position)
        self.outw(regs::CAPR, (self.rx_cur as u16).wrapping_sub(0x10));
    }
    
    /// Check if initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::SeqCst)
    }
    
    /// Handle interrupt
    pub fn handle_interrupt(&mut self) {
        let status = self.inw(regs::ISR);
        
        // Clear interrupt status
        self.outw(regs::ISR, status);
        
        if status & intr::TOK != 0 {
            log::trace!("RTL8139: TX OK");
        }
        if status & intr::ROK != 0 {
            log::trace!("RTL8139: RX OK");
        }
        if status & intr::TER != 0 {
            log::warn!("RTL8139: TX Error");
        }
        if status & intr::RER != 0 {
            log::warn!("RTL8139: RX Error");
        }
        if status & intr::RXOVW != 0 {
            log::warn!("RTL8139: RX Buffer Overflow");
        }
    }
}

/// Global RTL8139 driver instance
pub static RTL8139: Mutex<Option<Rtl8139Driver>> = Mutex::new(None);

/// Initialize RTL8139 driver
pub fn init() -> bool {
    if let Some(mut driver) = Rtl8139Driver::detect() {
        match driver.init() {
            Ok(_) => {
                *RTL8139.lock() = Some(driver);
                true
            }
            Err(e) => {
                log::error!("{}", e);
                false
            }
        }
    } else {
        log::debug!("RTL8139: No device found");
        false
    }
}
