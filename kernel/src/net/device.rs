//! Network Device Interface
//!
//! Abstract interface for network devices (NICs)

use super::buffer::PacketBuffer;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::boxed::Box;

/// Network device trait
pub trait NetworkDevice: Send + Sync {
    /// Get device name
    fn name(&self) -> &str;
    
    /// Get MAC address
    fn mac_address(&self) -> [u8; 6];
    
    /// Get MTU (Maximum Transmission Unit)
    fn mtu(&self) -> usize;
    
    /// Check if device is up
    fn is_up(&self) -> bool;
    
    /// Bring device up
    fn up(&mut self) -> Result<(), DeviceError>;
    
    /// Bring device down
    fn down(&mut self) -> Result<(), DeviceError>;
    
    /// Transmit packet
    fn transmit(&mut self, packet: PacketBuffer) -> Result<(), DeviceError>;
    
    /// Receive packet (non-blocking)
    fn receive(&mut self) -> Result<Option<PacketBuffer>, DeviceError>;
    
    /// Get statistics
    fn stats(&self) -> DeviceStats;
}

/// Device errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceError {
    NotReady,
    Busy,
    Timeout,
    NoBuffer,
    InvalidPacket,
    HardwareError,
}

/// Device statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct DeviceStats {
    /// Packets transmitted
    pub tx_packets: u64,
    
    /// Bytes transmitted
    pub tx_bytes: u64,
    
    /// Transmission errors
    pub tx_errors: u64,
    
    /// Packets received
    pub rx_packets: u64,
    
    /// Bytes received
    pub rx_bytes: u64,
    
    /// Reception errors
    pub rx_errors: u64,
    
    /// Packets dropped
    pub rx_dropped: u64,
}

/// Loopback device (for testing)
pub struct LoopbackDevice {
    name: String,
    mac: [u8; 6],
    up: bool,
    stats: DeviceStats,
    rx_queue: spin::Mutex<Vec<PacketBuffer>>,
}

impl LoopbackDevice {
    pub fn new() -> Self {
        Self {
            name: String::from("lo"),
            mac: [0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            up: false,
            stats: DeviceStats::default(),
            rx_queue: spin::Mutex::new(Vec::new()),
        }
    }
}

impl NetworkDevice for LoopbackDevice {
    fn name(&self) -> &str {
        &self.name
    }
    
    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }
    
    fn mtu(&self) -> usize {
        65536 // Loopback can handle large packets
    }
    
    fn is_up(&self) -> bool {
        self.up
    }
    
    fn up(&mut self) -> Result<(), DeviceError> {
        self.up = true;
        Ok(())
    }
    
    fn down(&mut self) -> Result<(), DeviceError> {
        self.up = false;
        Ok(())
    }
    
    fn transmit(&mut self, packet: PacketBuffer) -> Result<(), DeviceError> {
        if !self.up {
            return Err(DeviceError::NotReady);
        }
        
        self.stats.tx_packets += 1;
        self.stats.tx_bytes += packet.len() as u64;
        
        // Loopback: TX → RX immediately
        self.stats.rx_packets += 1;
        self.stats.rx_bytes += packet.len() as u64;
        
        let mut queue = self.rx_queue.lock();
        queue.push(packet);
        
        Ok(())
    }
    
    fn receive(&mut self) -> Result<Option<PacketBuffer>, DeviceError> {
        if !self.up {
            return Err(DeviceError::NotReady);
        }
        
        let mut queue = self.rx_queue.lock();
        Ok(queue.pop())
    }
    
    fn stats(&self) -> DeviceStats {
        self.stats
    }
}

/// Network device registry
pub struct DeviceRegistry {
    devices: spin::Mutex<Vec<Box<dyn NetworkDevice>>>,
}

impl DeviceRegistry {
    pub const fn new() -> Self {
        Self {
            devices: spin::Mutex::new(Vec::new()),
        }
    }
    
    /// Register new device
    pub fn register(&self, device: Box<dyn NetworkDevice>) {
        let name = String::from(device.name());
        let mut devices = self.devices.lock();
        devices.push(device);
        
        crate::logger::info(&alloc::format!(
            "[NET] Registered network device: {}",
            name
        ));
    }
    
    /// Get device by name
    pub fn get(&self, name: &str) -> Option<usize> {
        let devices = self.devices.lock();
        devices.iter().position(|d| d.name() == name)
    }
    
    /// List all devices
    pub fn list(&self) -> Vec<String> {
        let devices = self.devices.lock();
        devices.iter().map(|d| String::from(d.name())).collect()
    }
}

/// Global device registry
pub static DEVICE_REGISTRY: DeviceRegistry = DeviceRegistry::new();

/// Initialize network subsystem
pub fn init() {
    // Create and register loopback device
    let mut lo = Box::new(LoopbackDevice::new());
    let _ = lo.up();
    DEVICE_REGISTRY.register(lo);
    
    crate::logger::info("[NET] Network subsystem initialized");
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::buffer::PacketBuffer;
    
    #[test]
    fn test_loopback() {
        let mut lo = LoopbackDevice::new();
        lo.up().unwrap();
        
        assert!(lo.is_up());
        assert_eq!(lo.name(), "lo");
    }
    
    #[test]
    fn test_loopback_echo() {
        let mut lo = LoopbackDevice::new();
        lo.up().unwrap();
        
        // Send packet
        let mut pkt = PacketBuffer::with_default_capacity();
        pkt.put(b"Echo test").unwrap();
        lo.transmit(pkt).unwrap();
        
        // Receive it back
        let rx_pkt = lo.receive().unwrap();
        assert!(rx_pkt.is_some());
        
        let stats = lo.stats();
        assert_eq!(stats.tx_packets, 1);
        assert_eq!(stats.rx_packets, 1);
    }
}
