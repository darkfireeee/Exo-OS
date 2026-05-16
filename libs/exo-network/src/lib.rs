#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

use smoltcp::time::Instant;
use smoltcp::wire::{Ipv4Address, Ipv4Cidr};

pub const EXO_MIN_IPV4_MTU: usize = 576;
pub const EXO_DEFAULT_ETHERNET_MTU: usize = 1500;
pub const EXO_MAX_SOCKET_BUDGET: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkPortKind {
    PacketStack,
    Ring3Runtime,
    HttpService,
    TlsService,
    DnsService,
    DhcpService,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkVerdict {
    NativeRing1,
    Ring3PostV02,
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkPort {
    pub name: &'static str,
    pub vendor_tree: &'static str,
    pub kind: NetworkPortKind,
    pub exo_boundary: &'static str,
    pub verdict: NetworkVerdict,
    pub phoenix_policy: &'static str,
}

pub const NETWORK_PORTS: &[NetworkPort] = &[
    NetworkPort {
        name: "smoltcp",
        vendor_tree: "smoltcp-upstream",
        kind: NetworkPortKind::PacketStack,
        exo_boundary: "network_server/Ring1/no_std",
        verdict: NetworkVerdict::NativeRing1,
        phoenix_policy: "serialize-or-drop-active-sockets-before-switch",
    },
    NetworkPort {
        name: "tokio",
        vendor_tree: "tokio-upstream",
        kind: NetworkPortKind::Ring3Runtime,
        exo_boundary: "Ring3 sync/io types only; no tokio runtime in v0.2",
        verdict: NetworkVerdict::Ring3PostV02,
        phoenix_policy: "stateless-types-only",
    },
    NetworkPort {
        name: "hyper",
        vendor_tree: "hyper-upstream",
        kind: NetworkPortKind::HttpService,
        exo_boundary: "Ring3 HTTP service",
        verdict: NetworkVerdict::Ring3PostV02,
        phoenix_policy: "recreate-connections-after-switch",
    },
    NetworkPort {
        name: "axum",
        vendor_tree: "axum-upstream",
        kind: NetworkPortKind::HttpService,
        exo_boundary: "Ring3 HTTP router",
        verdict: NetworkVerdict::Ring3PostV02,
        phoenix_policy: "recreate-router-state-after-switch",
    },
    NetworkPort {
        name: "rustls",
        vendor_tree: "rustls-upstream",
        kind: NetworkPortKind::TlsService,
        exo_boundary: "crypto_server/network_server TLS handoff",
        verdict: NetworkVerdict::NativeRing1,
        phoenix_policy: "discard-session-cache-before-switch",
    },
    NetworkPort {
        name: "hickory-dns",
        vendor_tree: "hickory-dns-upstream",
        kind: NetworkPortKind::DnsService,
        exo_boundary: "network_server DNS client/service",
        verdict: NetworkVerdict::NativeRing1,
        phoenix_policy: "flush-dns-cache-before-switch",
    },
    NetworkPort {
        name: "dhcp4r",
        vendor_tree: "dhcp4r-upstream",
        kind: NetworkPortKind::DhcpService,
        exo_boundary: "network_server DHCP service",
        verdict: NetworkVerdict::NativeRing1,
        phoenix_policy: "persist-lease-via-capability-or-renew",
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExoIpv4Config {
    pub address: [u8; 4],
    pub prefix_len: u8,
    pub gateway: Option<[u8; 4]>,
}

impl ExoIpv4Config {
    pub const fn new(address: [u8; 4], prefix_len: u8, gateway: Option<[u8; 4]>) -> Self {
        Self {
            address,
            prefix_len,
            gateway,
        }
    }

    pub fn cidr(self) -> Option<Ipv4Cidr> {
        if self.prefix_len > 32 {
            return None;
        }
        Some(Ipv4Cidr::new(
            Ipv4Address::new(
                self.address[0],
                self.address[1],
                self.address[2],
                self.address[3],
            ),
            self.prefix_len,
        ))
    }

    pub fn gateway_addr(self) -> Option<Ipv4Address> {
        self.gateway
            .map(|octets| Ipv4Address::new(octets[0], octets[1], octets[2], octets[3]))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExoNetClock {
    tick_ms: u64,
}

impl ExoNetClock {
    pub const fn new() -> Self {
        Self { tick_ms: 0 }
    }

    pub fn advance(&mut self, delta_ms: u64) -> Instant {
        self.tick_ms = self.tick_ms.saturating_add(delta_ms);
        self.now()
    }

    pub fn now(self) -> Instant {
        Instant::from_millis(self.tick_ms.min(i64::MAX as u64) as i64)
    }
}

impl Default for ExoNetClock {
    fn default() -> Self {
        Self::new()
    }
}

pub const fn clamp_mtu(requested: usize) -> usize {
    if requested < EXO_MIN_IPV4_MTU {
        EXO_MIN_IPV4_MTU
    } else if requested > EXO_DEFAULT_ETHERNET_MTU {
        EXO_DEFAULT_ETHERNET_MTU
    } else {
        requested
    }
}

pub const fn socket_budget(requested: usize) -> usize {
    if requested == 0 {
        1
    } else if requested > EXO_MAX_SOCKET_BUDGET {
        EXO_MAX_SOCKET_BUDGET
    } else {
        requested
    }
}

pub fn find_network_port(name: &str) -> Option<&'static NetworkPort> {
    NETWORK_PORTS.iter().find(|port| port.name == name)
}

pub fn network_port_allowed(name: &str) -> bool {
    find_network_port(name)
        .map(|port| port.verdict != NetworkVerdict::Rejected)
        .unwrap_or(false)
}

pub fn smoltcp_stress_signature(iterations: u32) -> u64 {
    let mut clock = ExoNetClock::new();
    let mut acc = 0x4558_4f4e_4554_u64;
    let total = iterations.max(1);
    for i in 0..total {
        let host = ((i % 250) + 1) as u8;
        let config = ExoIpv4Config::new([10, (i >> 8) as u8, (i >> 16) as u8, host], 24, None);
        let cidr = config.cidr().expect("valid stress cidr");
        let addr = cidr.address().octets();
        let now = clock.advance((i % 17) as u64 + 1);
        acc = acc.rotate_left(5)
            ^ ((addr[0] as u64) << 24)
            ^ ((addr[1] as u64) << 16)
            ^ ((addr[2] as u64) << 8)
            ^ addr[3] as u64
            ^ cidr.prefix_len() as u64
            ^ now.millis() as u64;
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtnetlink_is_removed_from_exoos_network_ports() {
        assert!(find_network_port("rtnetlink").is_none());
        assert!(!network_port_allowed("rtnetlink"));
    }

    #[test]
    fn native_network_ports_have_phoenix_policy() {
        for port in NETWORK_PORTS {
            if port.verdict == NetworkVerdict::NativeRing1 {
                assert!(!port.phoenix_policy.is_empty());
            }
        }
    }
}
