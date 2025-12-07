//! Network Device Drivers
//!
//! Collection of network interface drivers for Exo-OS.
//!
//! ## Supported Devices
//! - VirtIO Network (virtio-net) - Virtual devices
//! - Intel e1000/e1000e - Gigabit Ethernet
//! - Loopback - Local testing
//!
//! ## Performance
//! - Hardware offloads: TSO, GSO, GRO, checksum
//! - Multi-queue support (RSS/RPS)
//! - Zero-copy DMA
//! - NAPI polling

pub mod virtio_net;
pub mod e1000;
pub mod loopback;

pub use virtio_net::VirtioNetDriver;
pub use e1000::E1000Driver;
pub use loopback::LoopbackDevice;

use alloc::vec::Vec;
use alloc::sync::Arc;
use crate::sync::SpinLock;

/// Network device capabilities
#[derive(Debug, Clone, Copy)]
pub struct DeviceCapabilities {
    pub max_mtu: u16,
    pub mac_address: [u8; 6],
    pub checksum_offload: bool,
    pub tso_offload: bool,
    pub gso_offload: bool,
    pub gro_offload: bool,
    pub scatter_gather: bool,
    pub multi_queue: bool,
    pub queue_count: u16,
}

impl Default for DeviceCapabilities {
    fn default() -> Self {
        Self {
            max_mtu: 1500,
            mac_address: [0; 6],
            checksum_offload: false,
            tso_offload: false,
            gso_offload: false,
            gro_offload: false,
            scatter_gather: false,
            multi_queue: false,
            queue_count: 1,
        }
    }
}

/// Network device statistics
#[derive(Debug, Default, Clone, Copy)]
pub struct DeviceStats {
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
    pub rx_dropped: u64,
    pub tx_dropped: u64,
    pub multicast: u64,
}

/// Network device trait
pub trait NetworkDevice: Send + Sync {
    /// Get device name
    fn name(&self) -> &str;
    
    /// Get device capabilities
    fn capabilities(&self) -> &DeviceCapabilities;
    
    /// Get device statistics
    fn stats(&self) -> DeviceStats;
    
    /// Send a packet
    fn send(&self, packet: &[u8]) -> Result<(), DriverError>;
    
    /// Receive packets (returns list of received packets)
    fn receive(&self) -> Result<Vec<Vec<u8>>, DriverError>;
    
    /// Set promiscuous mode
    fn set_promiscuous(&self, enabled: bool) -> Result<(), DriverError>;
    
    /// Set MAC address
    fn set_mac_address(&self, mac: [u8; 6]) -> Result<(), DriverError>;
}

/// Driver errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverError {
    NotSupported,
    DeviceNotReady,
    TransmitFailed,
    ReceiveFailed,
    InvalidPacket,
    QueueFull,
    DmaError,
    HardwareError,
}

pub type DriverResult<T> = Result<T, DriverError>;

/// Device registry
pub struct DeviceRegistry {
    devices: SpinLock<Vec<Arc<dyn NetworkDevice>>>,
}

impl DeviceRegistry {
    pub const fn new() -> Self {
        Self {
            devices: SpinLock::new(Vec::new()),
        }
    }
    
    /// Register a network device
    pub fn register(&self, device: Arc<dyn NetworkDevice>) {
        let mut devices = self.devices.lock();
        devices.push(device);
        log::info!("Registered network device: {}", devices.last().unwrap().name());
    }
    
    /// Get all registered devices
    pub fn devices(&self) -> Vec<Arc<dyn NetworkDevice>> {
        self.devices.lock().clone()
    }
    
    /// Get device by name
    pub fn get_device(&self, name: &str) -> Option<Arc<dyn NetworkDevice>> {
        let devices = self.devices.lock();
        devices.iter()
            .find(|dev| dev.name() == name)
            .cloned()
    }
}

/// Global device registry
pub static DEVICE_REGISTRY: DeviceRegistry = DeviceRegistry::new();

/// Initialize all network drivers
pub fn init() -> DriverResult<()> {
    log::info!("Initializing network drivers");
    
    // Initialize loopback (always available)
    let loopback = Arc::new(loopback::LoopbackDevice::new());
    DEVICE_REGISTRY.register(loopback);
    
    // Try to detect and initialize hardware drivers
    virtio_net::init().ok();
    e1000::init().ok();
    
    Ok(())
}
