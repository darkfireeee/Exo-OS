//! # IP Module
//! 
//! Internet Protocol (IPv4 et IPv6)

pub mod ipv4;
pub mod ipv6;
pub mod routing;
pub mod fragmentation;
pub mod icmpv6;

// Re-exports
pub use ipv4::{Ipv4Header, Ipv4Addr};
pub use ipv6::{Ipv6Header, Ipv6Addr};
pub use fragmentation::{FragmentManager, FragmentCache};
pub use icmpv6::{Icmpv6Message, Icmpv6Type};
