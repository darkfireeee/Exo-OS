//! PCI Bus Driver
//!
//! Provides PCI device enumeration and configuration space access.
//! Supports:
//! - Configuration space read/write (Type 0 and Type 1)
//! - Device enumeration
//! - BAR (Base Address Register) decoding
//! - MSI/MSI-X interrupt support

use alloc::vec::Vec;
use alloc::string::String;
use spin::Mutex;
use x86_64::instructions::port::{Port, PortWriteOnly, PortReadOnly};

/// PCI Configuration Address port (0xCF8)
const PCI_CONFIG_ADDR: u16 = 0xCF8;
/// PCI Configuration Data port (0xCFC)
const PCI_CONFIG_DATA: u16 = 0xCFC;

/// Global PCI bus
pub static PCI_BUS: Mutex<PciBus> = Mutex::new(PciBus::new());

/// PCI Device ID structure
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciDeviceId {
    pub vendor_id: u16,
    pub device_id: u16,
}

impl PciDeviceId {
    pub const fn new(vendor_id: u16, device_id: u16) -> Self {
        Self { vendor_id, device_id }
    }
}

/// Well-known PCI device IDs
pub mod ids {
    use super::PciDeviceId;
    
    // Intel
    pub const INTEL_E1000: PciDeviceId = PciDeviceId::new(0x8086, 0x100E);
    pub const INTEL_E1000_82545EM: PciDeviceId = PciDeviceId::new(0x8086, 0x100F);
    pub const INTEL_PRO1000_MT: PciDeviceId = PciDeviceId::new(0x8086, 0x1004);
    
    // Realtek
    pub const REALTEK_RTL8139: PciDeviceId = PciDeviceId::new(0x10EC, 0x8139);
    pub const REALTEK_RTL8169: PciDeviceId = PciDeviceId::new(0x10EC, 0x8169);
    
    // VirtIO (QEMU)
    pub const VIRTIO_NET: PciDeviceId = PciDeviceId::new(0x1AF4, 0x1000);
    pub const VIRTIO_BLK: PciDeviceId = PciDeviceId::new(0x1AF4, 0x1001);
    pub const VIRTIO_CONSOLE: PciDeviceId = PciDeviceId::new(0x1AF4, 0x1003);
    
    // AMD
    pub const AMD_PCNET: PciDeviceId = PciDeviceId::new(0x1022, 0x2000);
}

/// PCI Class codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PciClass {
    Unclassified = 0x00,
    MassStorage = 0x01,
    Network = 0x02,
    Display = 0x03,
    Multimedia = 0x04,
    Memory = 0x05,
    Bridge = 0x06,
    SimpleCommunication = 0x07,
    BaseSystemPeripheral = 0x08,
    InputDevice = 0x09,
    DockingStation = 0x0A,
    Processor = 0x0B,
    SerialBus = 0x0C,
    Wireless = 0x0D,
    IntelligentController = 0x0E,
    SatelliteCommunication = 0x0F,
    Encryption = 0x10,
    SignalProcessing = 0x11,
    Unknown = 0xFF,
}

impl From<u8> for PciClass {
    fn from(val: u8) -> Self {
        match val {
            0x00 => PciClass::Unclassified,
            0x01 => PciClass::MassStorage,
            0x02 => PciClass::Network,
            0x03 => PciClass::Display,
            0x04 => PciClass::Multimedia,
            0x05 => PciClass::Memory,
            0x06 => PciClass::Bridge,
            0x07 => PciClass::SimpleCommunication,
            0x08 => PciClass::BaseSystemPeripheral,
            0x09 => PciClass::InputDevice,
            0x0A => PciClass::DockingStation,
            0x0B => PciClass::Processor,
            0x0C => PciClass::SerialBus,
            0x0D => PciClass::Wireless,
            0x0E => PciClass::IntelligentController,
            0x0F => PciClass::SatelliteCommunication,
            0x10 => PciClass::Encryption,
            0x11 => PciClass::SignalProcessing,
            _ => PciClass::Unknown,
        }
    }
}

/// PCI Base Address Register type
#[derive(Debug, Clone, Copy)]
pub enum PciBar {
    /// Memory-mapped I/O
    Memory {
        address: u64,
        size: u64,
        prefetchable: bool,
        is_64bit: bool,
    },
    /// Port I/O
    Io {
        port: u32,
        size: u32,
    },
    /// Not implemented/empty
    None,
}

/// PCI Device
#[derive(Debug, Clone)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: PciClass,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision: u8,
    pub header_type: u8,
    pub bars: [PciBar; 6],
    pub interrupt_line: u8,
    pub interrupt_pin: u8,
}

impl PciDevice {
    /// Get device location as BDF (Bus:Device.Function)
    pub fn bdf(&self) -> String {
        alloc::format!("{:02x}:{:02x}.{}", self.bus, self.device, self.function)
    }
    
    /// Check if device matches vendor/device ID
    pub fn matches(&self, id: PciDeviceId) -> bool {
        self.vendor_id == id.vendor_id && self.device_id == id.device_id
    }
    
    /// Get first memory BAR address
    pub fn memory_base(&self) -> Option<u64> {
        for bar in &self.bars {
            if let PciBar::Memory { address, .. } = bar {
                if *address != 0 {
                    return Some(*address);
                }
            }
        }
        None
    }
    
    /// Get first I/O BAR port
    pub fn io_base(&self) -> Option<u32> {
        for bar in &self.bars {
            if let PciBar::Io { port, .. } = bar {
                if *port != 0 {
                    return Some(*port);
                }
            }
        }
        None
    }
    
    /// Enable bus mastering (for DMA)
    pub fn enable_bus_master(&self) {
        let command = pci_config_read_word(self.bus, self.device, self.function, 0x04);
        pci_config_write_word(self.bus, self.device, self.function, 0x04, command | 0x04);
    }
    
    /// Enable memory space access
    pub fn enable_memory(&self) {
        let command = pci_config_read_word(self.bus, self.device, self.function, 0x04);
        pci_config_write_word(self.bus, self.device, self.function, 0x04, command | 0x02);
    }
    
    /// Enable I/O space access
    pub fn enable_io(&self) {
        let command = pci_config_read_word(self.bus, self.device, self.function, 0x04);
        pci_config_write_word(self.bus, self.device, self.function, 0x04, command | 0x01);
    }
    
    /// Disable interrupts
    pub fn disable_interrupts(&self) {
        let command = pci_config_read_word(self.bus, self.device, self.function, 0x04);
        pci_config_write_word(self.bus, self.device, self.function, 0x04, command | 0x400);
    }
}

/// PCI Bus manager
pub struct PciBus {
    devices: Vec<PciDevice>,
    initialized: bool,
}

impl PciBus {
    pub const fn new() -> Self {
        Self {
            devices: Vec::new(),
            initialized: false,
        }
    }
    
    /// Initialize and enumerate PCI bus
    pub fn init(&mut self) {
        if self.initialized {
            return;
        }
        
        log::info!("Enumerating PCI bus...");
        self.enumerate();
        self.initialized = true;
        
        log::info!("Found {} PCI device(s)", self.devices.len());
        for dev in &self.devices {
            log::debug!(
                "  {} - {:04x}:{:04x} class {:02x}:{:02x}",
                dev.bdf(),
                dev.vendor_id,
                dev.device_id,
                dev.class as u8,
                dev.subclass
            );
        }
    }
    
    /// Enumerate all PCI devices
    fn enumerate(&mut self) {
        // Check all buses (0-255), devices (0-31), functions (0-7)
        for bus in 0..=255u8 {
            for device in 0..32u8 {
                self.check_device(bus, device);
            }
        }
    }
    
    /// Check a specific device
    fn check_device(&mut self, bus: u8, device: u8) {
        let vendor_id = pci_config_read_word(bus, device, 0, 0x00);
        if vendor_id == 0xFFFF {
            return; // No device
        }
        
        self.check_function(bus, device, 0);
        
        // Check if multi-function device
        let header_type = pci_config_read_byte(bus, device, 0, 0x0E);
        if header_type & 0x80 != 0 {
            // Multi-function device
            for function in 1..8u8 {
                let vendor = pci_config_read_word(bus, device, function, 0x00);
                if vendor != 0xFFFF {
                    self.check_function(bus, device, function);
                }
            }
        }
    }
    
    /// Check a specific function and add to device list
    fn check_function(&mut self, bus: u8, device: u8, function: u8) {
        let vendor_id = pci_config_read_word(bus, device, function, 0x00);
        let device_id = pci_config_read_word(bus, device, function, 0x02);
        let class_subclass = pci_config_read_word(bus, device, function, 0x0A);
        let prog_if_rev = pci_config_read_word(bus, device, function, 0x08);
        let header_type = pci_config_read_byte(bus, device, function, 0x0E);
        let interrupt = pci_config_read_word(bus, device, function, 0x3C);
        
        let class = PciClass::from((class_subclass >> 8) as u8);
        let subclass = (class_subclass & 0xFF) as u8;
        let prog_if = (prog_if_rev >> 8) as u8;
        let revision = (prog_if_rev & 0xFF) as u8;
        
        // Read BARs (only for Type 0 headers)
        let mut bars = [PciBar::None; 6];
        if header_type & 0x7F == 0 {
            let mut i = 0;
            while i < 6 {
                let bar_offset = 0x10 + (i as u8 * 4);
                let bar_value = pci_config_read_dword(bus, device, function, bar_offset);
                
                if bar_value == 0 {
                    i += 1;
                    continue;
                }
                
                if bar_value & 0x01 != 0 {
                    // I/O space
                    let port = bar_value & 0xFFFFFFFC;
                    
                    // Determine size
                    pci_config_write_dword(bus, device, function, bar_offset, 0xFFFFFFFF);
                    let size_mask = pci_config_read_dword(bus, device, function, bar_offset);
                    pci_config_write_dword(bus, device, function, bar_offset, bar_value);
                    
                    let size = !((size_mask & 0xFFFFFFFC) as u32).wrapping_add(1);
                    
                    bars[i] = PciBar::Io {
                        port,
                        size: if size == 0 { 256 } else { size },
                    };
                } else {
                    // Memory space
                    let is_64bit = (bar_value >> 1) & 0x03 == 0x02;
                    let prefetchable = (bar_value >> 3) & 0x01 != 0;
                    
                    let address = if is_64bit && i < 5 {
                        let high = pci_config_read_dword(bus, device, function, bar_offset + 4);
                        ((high as u64) << 32) | ((bar_value & 0xFFFFFFF0) as u64)
                    } else {
                        (bar_value & 0xFFFFFFF0) as u64
                    };
                    
                    // Determine size (simplified)
                    pci_config_write_dword(bus, device, function, bar_offset, 0xFFFFFFFF);
                    let size_mask = pci_config_read_dword(bus, device, function, bar_offset);
                    pci_config_write_dword(bus, device, function, bar_offset, bar_value);
                    
                    let size = !((size_mask & 0xFFFFFFF0) as u64).wrapping_add(1);
                    
                    bars[i] = PciBar::Memory {
                        address,
                        size: if size == 0 { 4096 } else { size },
                        prefetchable,
                        is_64bit,
                    };
                    
                    if is_64bit {
                        i += 1; // Skip next BAR (used for high 32 bits)
                    }
                }
                i += 1;
            }
        }
        
        self.devices.push(PciDevice {
            bus,
            device,
            function,
            vendor_id,
            device_id,
            class,
            subclass,
            prog_if,
            revision,
            header_type: header_type & 0x7F,
            bars,
            interrupt_line: (interrupt & 0xFF) as u8,
            interrupt_pin: ((interrupt >> 8) & 0xFF) as u8,
        });
    }
    
    /// Find devices by class
    pub fn find_by_class(&self, class: PciClass) -> Vec<&PciDevice> {
        self.devices.iter().filter(|d| d.class == class).collect()
    }
    
    /// Find device by vendor/device ID
    pub fn find_device(&self, id: PciDeviceId) -> Option<&PciDevice> {
        self.devices.iter().find(|d| d.matches(id))
    }
    
    /// Find all network devices
    pub fn find_network_devices(&self) -> Vec<&PciDevice> {
        self.find_by_class(PciClass::Network)
    }
    
    /// Get all devices
    pub fn devices(&self) -> &[PciDevice] {
        &self.devices
    }
}

// ============================================================================
// PCI Configuration Space Access
// ============================================================================

/// Build PCI configuration address
#[inline]
fn pci_config_address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC)
        | 0x80000000 // Enable bit
}

/// Read a byte from PCI configuration space
pub fn pci_config_read_byte(bus: u8, device: u8, function: u8, offset: u8) -> u8 {
    let address = pci_config_address(bus, device, function, offset);
    let shift = (offset & 0x03) * 8;
    
    unsafe {
        let mut addr_port: PortWriteOnly<u32> = PortWriteOnly::new(PCI_CONFIG_ADDR);
        let mut data_port: Port<u32> = Port::new(PCI_CONFIG_DATA);
        
        addr_port.write(address);
        ((data_port.read() >> shift) & 0xFF) as u8
    }
}

/// Read a word (16-bit) from PCI configuration space
pub fn pci_config_read_word(bus: u8, device: u8, function: u8, offset: u8) -> u16 {
    let address = pci_config_address(bus, device, function, offset);
    let shift = (offset & 0x02) * 8;
    
    unsafe {
        let mut addr_port: PortWriteOnly<u32> = PortWriteOnly::new(PCI_CONFIG_ADDR);
        let mut data_port: Port<u32> = Port::new(PCI_CONFIG_DATA);
        
        addr_port.write(address);
        ((data_port.read() >> shift) & 0xFFFF) as u16
    }
}

/// Read a dword (32-bit) from PCI configuration space
pub fn pci_config_read_dword(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address = pci_config_address(bus, device, function, offset);
    
    unsafe {
        let mut addr_port: PortWriteOnly<u32> = PortWriteOnly::new(PCI_CONFIG_ADDR);
        let mut data_port: Port<u32> = Port::new(PCI_CONFIG_DATA);
        
        addr_port.write(address);
        data_port.read()
    }
}

/// Write a word to PCI configuration space
pub fn pci_config_write_word(bus: u8, device: u8, function: u8, offset: u8, value: u16) {
    let address = pci_config_address(bus, device, function, offset);
    let shift = (offset & 0x02) * 8;
    
    unsafe {
        let mut addr_port: PortWriteOnly<u32> = PortWriteOnly::new(PCI_CONFIG_ADDR);
        let mut data_port: Port<u32> = Port::new(PCI_CONFIG_DATA);
        
        addr_port.write(address);
        let current = data_port.read();
        let mask = !(0xFFFFu32 << shift);
        let new_value = (current & mask) | ((value as u32) << shift);
        data_port.write(new_value);
    }
}

/// Write a dword to PCI configuration space
pub fn pci_config_write_dword(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    let address = pci_config_address(bus, device, function, offset);
    
    unsafe {
        let mut addr_port: PortWriteOnly<u32> = PortWriteOnly::new(PCI_CONFIG_ADDR);
        let mut data_port: Port<u32> = Port::new(PCI_CONFIG_DATA);
        
        addr_port.write(address);
        data_port.write(value);
    }
}

/// Initialize PCI subsystem
pub fn init() {
    PCI_BUS.lock().init();
}
