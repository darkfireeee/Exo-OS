//! VPN Subsystem - Virtual Private Networks
//!
//! Collection of VPN protocols for Exo-OS.
//!
//! ## Supported Protocols
//! - IPsec - Industry standard (ESP/AH + IKEv2)
//! - OpenVPN - Open-source SSL/TLS VPN
//! - WireGuard - Modern, minimal VPN (see ../wireguard/)
//!
//! ## Architecture
//! Each VPN protocol is implemented as a separate module
//! with consistent interfaces for tunnel creation and packet processing.

pub mod ipsec;
pub mod openvpn;

pub use ipsec::{IpsecEngine, SecurityAssociation, IpsecProtocol, IpsecMode};
pub use openvpn::{OpenVpnEngine, OpenVpnSession, OpCode as OpenVpnOpCode};

/// VPN tunnel interface
pub trait VpnTunnel: Send + Sync {
    /// Get tunnel name
    fn name(&self) -> &str;
    
    /// Encapsulate outbound packet
    fn encapsulate(&self, packet: &[u8]) -> Result<alloc::vec::Vec<u8>, VpnError>;
    
    /// Decapsulate inbound packet
    fn decapsulate(&self, packet: &[u8]) -> Result<alloc::vec::Vec<u8>, VpnError>;
    
    /// Get tunnel statistics
    fn stats(&self) -> VpnStats;
}

/// VPN statistics
#[derive(Debug, Default, Clone, Copy)]
pub struct VpnStats {
    pub packets_encapsulated: u64,
    pub packets_decapsulated: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub errors: u64,
}

/// VPN errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpnError {
    NotConnected,
    InvalidPacket,
    EncryptionFailed,
    DecryptionFailed,
    TunnelNotFound,
}

pub type VpnResult<T> = Result<T, VpnError>;

/// Initialize VPN subsystem
pub fn init() -> VpnResult<()> {
    log::info!("Initializing VPN subsystem");
    
    ipsec::init().map_err(|_| VpnError::EncryptionFailed)?;
    openvpn::init().map_err(|_| VpnError::EncryptionFailed)?;
    
    log::info!("VPN subsystem initialized (IPsec, OpenVPN)");
    Ok(())
}
