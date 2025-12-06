/// Ethernet Protocol Implementation
/// 
/// Complete Ethernet layer with:
/// - ARP (Address Resolution Protocol)
/// - VLAN support
/// - Bridging (TODO)

pub mod arp;

// Re-export from kernel's net/ethernet module
pub use crate::net::ethernet::{EthernetHeader, EtherType};

pub use arp::{ArpHeader, ArpPacket, ArpOp, process_arp};

// VLAN support (from kernel/src/net/ethernet/vlan.rs)
// pub use crate::net::ethernet::vlan;
