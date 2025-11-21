//! Network subsystem for hybrid architecture
//!
//! Supports TCP/IP stack, UDP, Ethernet, and WireGuard VPN

use alloc::vec::Vec;
use crate::memory::MemoryError;

/// Network core (sockets, devices, buffers)
pub mod core;

/// Ethernet layer
pub mod ethernet;

/// IP layer (IPv4/IPv6)
pub mod ip;

/// TCP protocol
pub mod tcp;

/// UDP protocol
pub mod udp;

/// WireGuard VPN
pub mod wireguard;

/// Network errors
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
}

impl From<MemoryError> for NetError {
    fn from(e: MemoryError) -> Self {
        NetError::Memory(e)
    }
}

pub type NetResult<T> = Result<T, NetError>;

/// Network address (IPv4 or IPv6)
#[derive(Debug, Clone, Copy)]
pub enum IpAddress {
    V4([u8; 4]),
    V6([u8; 16]),
}

/// Initialize network subsystem
pub fn init() -> NetResult<()> {
    log::info!("Initializing network subsystem (hybrid architecture)");
    core::init()?;
    Ok(())
}
