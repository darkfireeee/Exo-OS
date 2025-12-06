/// IP (Internet Protocol) Implementation
/// 
/// Complete IPv4 and IPv6 stack with:
/// - IPv4 and IPv6 packet processing
/// - Routing and fragmentation
/// - ICMP and ICMPv6
/// - IGMP (multicast group management)
/// - IP tunneling (IPIP, GRE)

pub mod igmp;
pub mod tunnel;
pub mod icmp;
pub mod routing;

pub use igmp::{IgmpHeader, IgmpMessageType, IgmpV3Report, GroupRecord};
pub use tunnel::{Tunnel, TunnelConfig, TunnelType, TunnelManager};
pub use icmp::{IcmpHeader, IcmpType, process_packet as process_icmp};
pub use routing::{RoutingTable, RouteEntry, IpPrefix, IpAddr, routing_table};

// Re-export from kernel's net/ip module
pub use crate::net::ip::{
    ipv4,
    ipv6,
    fragmentation,
    icmpv6,
};
