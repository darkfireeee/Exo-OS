//! # IP Module
//! 
//! Internet Protocol (IPv4 et IPv6)

pub mod ipv4;
pub mod ipv6;
pub mod icmp;
pub mod icmpv6;
pub mod routing;
pub mod fragmentation;

// Re-exports
pub use ipv4::{Ipv4Address, Ipv4Packet, IpProtocol, checksum};
pub use ipv6::{Ipv6Address, Ipv6Packet, Ipv6Header, NextHeader};
pub use icmp::{IcmpMessage, IcmpType, IcmpError};
pub use icmpv6::{Icmpv6Message, Icmpv6Type};
pub use fragmentation::{FragmentManager, FragmentCache, IpFragment, FragmentKey};
pub use routing::{RoutingTable, Route, IpNetwork, IpAddr, RouteSource, ROUTING_TABLE};

// Backward compatibility aliases
pub use ipv4::Ipv4Address as Ipv4Addr;
pub use ipv6::Ipv6Address as Ipv6Addr;
pub use ipv4::Ipv4Packet as Ipv4Header;
pub use ipv6::Ipv6Header as Ipv6Hdr;
