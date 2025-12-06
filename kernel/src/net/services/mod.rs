/// Network Services
/// 
/// Services réseau de haut niveau:
/// - DHCP: Configuration automatique
/// - DNS: Résolution de noms
/// - NTP: Synchronisation temporelle (TODO)

pub mod dhcp;
pub mod dns;
// pub mod ntp;  // TODO

pub use dhcp::{DhcpClient, DhcpState, DhcpAction};
pub use dns::{DnsClient, resolve, add_dns_server};
