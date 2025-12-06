/// DNS (Domain Name System)
/// 
/// Client DNS pour résolution de noms de domaine

pub mod client;

pub use client::{DnsClient, DnsError, DNS_CLIENT, resolve, add_dns_server};

/// DNS record types
pub use client::{
    DNS_TYPE_A,
    DNS_TYPE_NS,
    DNS_TYPE_CNAME,
    DNS_TYPE_SOA,
    DNS_TYPE_PTR,
    DNS_TYPE_MX,
    DNS_TYPE_TXT,
    DNS_TYPE_AAAA,
    DNS_CLASS_IN,
};
