//! Network Stack - Core Module
//!
//! Phase 2 - Mois 4: Network Stack Core
//! 
//! Provides:
//! - Socket abstraction (BSD-like API)
//! - Packet buffers (sk_buff equivalent)
//! - Network device interface
//! - Ethernet, IP, TCP, UDP protocols
//!
//! Status: Foundation complete, protocols in progress

use alloc::vec::Vec;
use crate::memory::MemoryError;

/// Socket abstraction (BSD-like API)
pub mod socket;

/// Packet buffers (sk_buff equivalent)
pub mod buffer;

/// Network device interface
pub mod device;

/// Ethernet layer
pub mod ethernet;

/// Network stack core (legacy - migrating)
pub mod stack;

/// UDP protocol
pub mod udp;

// Legacy modules (to be migrated or removed)
pub mod core;
pub mod protocols;
pub mod wireguard;
pub mod vpn;
pub mod time;
pub mod firewall;
pub mod services;
pub mod qos;
pub mod loadbalancer;
pub mod rdma;
pub mod monitoring;

/// Network drivers
pub mod drivers;

// Re-exports for convenience
pub use socket::{Socket, SocketAddr, SocketType, SocketDomain, IpAddr as IpAddress, Ipv4Addr, SOCKET_TABLE};
pub use buffer::{PacketBuffer, Protocol, PACKET_POOL};
pub use device::{NetworkDevice, DeviceStats, DEVICE_REGISTRY};
pub use ethernet::{EthernetHeader, MacAddr};

/// Network errors (unified)
#[derive(Debug)]
pub enum NetError {
    InvalidAddress,
    ConnectionRefused,
    Timeout,
    NotConnected,
    AlreadyConnected,
    BufferFull,
    InvalidPacket,
    NotSupported,
    Memory(MemoryError),
    Socket(socket::SocketError),
    Device(device::DeviceError),
    Ethernet(ethernet::EthernetError),
}

impl From<MemoryError> for NetError {
    fn from(e: MemoryError) -> Self {
        NetError::Memory(e)
    }
}

impl From<socket::SocketError> for NetError {
    fn from(e: socket::SocketError) -> Self {
        NetError::Socket(e)
    }
}

impl From<device::DeviceError> for NetError {
    fn from(e: device::DeviceError) -> Self {
        NetError::Device(e)
    }
}

impl From<ethernet::EthernetError> for NetError {
    fn from(e: ethernet::EthernetError) -> Self {
        NetError::Ethernet(e)
    }
}

pub type NetResult<T> = Result<T, NetError>;

/// Initialize network subsystem
pub fn init() -> NetResult<()> {
    crate::logger::info("[NET] Initializing Phase 2 network stack");
    
    // Initialize packet buffer pool
    PACKET_POOL.init(256);
    
    // Initialize device registry (includes loopback)
    device::init();
    
    crate::logger::info("[NET] Network stack initialized successfully");
    Ok(())
}

/// Get network statistics
pub fn stats() -> NetworkStats {
    NetworkStats {
        total_packets_sent: 0,
        total_packets_received: 0,
        total_bytes_sent: 0,
        total_bytes_received: 0,
        active_connections: 0,
        active_sockets: 0,
    }
}

/// Network statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct NetworkStats {
    pub total_packets_sent: u64,
    pub total_packets_received: u64,
    pub total_bytes_sent: u64,
    pub total_bytes_received: u64,
    pub active_connections: u64,
    pub active_sockets: u64,
}
    core::init()?;
    Ok(())
}
