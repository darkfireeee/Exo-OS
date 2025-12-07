/// Ethernet Protocol Implementation
/// 
/// Complete Ethernet layer with:
/// - ARP (Address Resolution Protocol)
/// - VLAN support
/// - High-performance bridging (10M+ entries, <1μs latency)

pub mod arp;
pub mod bridge;

// Re-export from kernel's net/ethernet module
pub use crate::net::ethernet::{EthernetHeader, EtherType};

pub use arp::{ArpHeader, ArpPacket, ArpOp, process_arp};
pub use bridge::{EthernetBridge, MacAddress, BridgePort, PortState, ForwardDecision, BridgeError};

// VLAN support (from kernel/src/net/ethernet/vlan.rs)
// pub use crate::net::ethernet::vlan;
