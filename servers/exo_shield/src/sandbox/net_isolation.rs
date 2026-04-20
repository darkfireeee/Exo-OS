//! Network isolation for the exo_shield sandbox.
//!
//! Provides allowed-ports / allowed-hosts lists, protocol filtering, and
//! bandwidth-limit enforcement — all with static fixed-capacity arrays.

use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of allowed port entries.
pub const MAX_ALLOWED_PORTS: usize = 32;

/// Maximum number of allowed host entries.
pub const MAX_ALLOWED_HOSTS: usize = 16;

/// Maximum length of a hostname pattern in bytes.
pub const MAX_HOST_LEN: usize = 64;

// ---------------------------------------------------------------------------
// Protocol
// ---------------------------------------------------------------------------

/// Network protocol identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum Protocol {
    Tcp = 6,
    Udp = 17,
    Icmp = 1,
    Raw = 255,
}

impl Protocol {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            6 => Some(Protocol::Tcp),
            17 => Some(Protocol::Udp),
            1 => Some(Protocol::Icmp),
            255 => Some(Protocol::Raw),
            _ => None,
        }
    }
}

/// Bitfield of allowed protocols.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ProtocolFilter(u8);

impl ProtocolFilter {
    pub const NONE: u8 = 0x00;
    pub const TCP: u8 = 0x01;
    pub const UDP: u8 = 0x02;
    pub const ICMP: u8 = 0x04;
    pub const RAW: u8 = 0x08;
    pub const ALL: u8 = 0x0F;

    pub const fn new(bits: u8) -> Self {
        Self(bits & Self::ALL)
    }

    pub fn allow(&mut self, proto: Protocol) {
        match proto {
            Protocol::Tcp => self.0 |= Self::TCP,
            Protocol::Udp => self.0 |= Self::UDP,
            Protocol::Icmp => self.0 |= Self::ICMP,
            Protocol::Raw => self.0 |= Self::RAW,
        }
    }

    pub fn deny(&mut self, proto: Protocol) {
        match proto {
            Protocol::Tcp => self.0 &= !Self::TCP,
            Protocol::Udp => self.0 &= !Self::UDP,
            Protocol::Icmp => self.0 &= !Self::ICMP,
            Protocol::Raw => self.0 &= !Self::RAW,
        }
    }

    pub fn is_allowed(&self, proto: Protocol) -> bool {
        let bit = match proto {
            Protocol::Tcp => Self::TCP,
            Protocol::Udp => Self::UDP,
            Protocol::Icmp => Self::ICMP,
            Protocol::Raw => Self::RAW,
        };
        self.0 & bit != 0
    }

    pub const fn bits(self) -> u8 {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Host entry
// ---------------------------------------------------------------------------

/// A hostname pattern (glob-style) with a validity flag.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct HostEntry {
    pattern: [u8; MAX_HOST_LEN],
    len: u16,
    active: bool,
}

impl HostEntry {
    pub const fn empty() -> Self {
        Self {
            pattern: [0u8; MAX_HOST_LEN],
            len: 0,
            active: false,
        }
    }

    pub fn from_bytes(pattern: &[u8]) -> Option<Self> {
        if pattern.is_empty() || pattern.len() >= MAX_HOST_LEN {
            return None;
        }
        let mut buf = [0u8; MAX_HOST_LEN];
        let mut i = 0;
        while i < pattern.len() {
            buf[i] = pattern[i];
            i += 1;
        }
        Some(Self {
            pattern: buf,
            len: pattern.len() as u16,
            active: true,
        })
    }

    #[inline]
    pub fn pattern_str(&self) -> &[u8] {
        &self.pattern[..self.len as usize]
    }

    #[inline]
    pub fn is_active(&self) -> bool {
        self.active
    }
}

// ---------------------------------------------------------------------------
// Bandwidth limit
// ---------------------------------------------------------------------------

/// Bandwidth limit in bytes per second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct BandwidthLimit {
    /// Bytes per second allowed (0 = no limit).
    bytes_per_sec: u64,
    /// Current window consumption in bytes.
    consumed: AtomicU64,
    /// Window start timestamp (arbitrary tick unit).
    window_start: AtomicU64,
    /// Window duration in ticks.
    window_ticks: u64,
}

impl BandwidthLimit {
    pub const fn unlimited() -> Self {
        Self {
            bytes_per_sec: 0,
            consumed: AtomicU64::new(0),
            window_start: AtomicU64::new(0),
            window_ticks: 1_000,
        }
    }

    pub const fn new(bytes_per_sec: u64, window_ticks: u64) -> Self {
        Self {
            bytes_per_sec,
            consumed: AtomicU64::new(0),
            window_start: AtomicU64::new(0),
            window_ticks,
        }
    }

    /// Check whether transmitting `nbytes` at `now` ticks would exceed the
    /// limit.  Returns `true` if the transmission is allowed.
    pub fn check_and_account(&self, nbytes: u64, now: u64) -> bool {
        if self.bytes_per_sec == 0 {
            return true; // unlimited
        }
        let start = self.window_start.load(Ordering::Acquire);
        if now >= start + self.window_ticks {
            // New window — reset counter.
            self.consumed.store(nbytes, Ordering::Release);
            self.window_start.store(now, Ordering::Release);
            return nbytes <= self.bytes_per_sec;
        }
        // Same window — add to consumed.
        let prev = self.consumed.fetch_add(nbytes, Ordering::AcqRel);
        prev + nbytes <= self.bytes_per_sec
    }

    /// Reset the bandwidth counter (e.g. on rate-limit policy change).
    pub fn reset(&self) {
        self.consumed.store(0, Ordering::Release);
        self.window_start.store(0, Ordering::Release);
    }

    /// Update the bytes-per-sec limit.
    pub fn set_limit(&mut self, bytes_per_sec: u64) {
        self.bytes_per_sec = bytes_per_sec;
        self.reset();
    }
}

// ---------------------------------------------------------------------------
// Network isolation config
// ---------------------------------------------------------------------------

/// Complete network isolation configuration for one sandbox.
#[derive(Debug)]
#[repr(C)]
pub struct NetIsolationConfig {
    /// Allowed TCP/UDP ports.
    allowed_ports: [u16; MAX_ALLOWED_PORTS],
    /// Number of active port entries.
    port_count: u32,
    /// Allowed host patterns.
    allowed_hosts: [HostEntry; MAX_ALLOWED_HOSTS],
    /// Number of active host entries.
    host_count: u32,
    /// Protocol filter.
    proto_filter: ProtocolFilter,
    /// Outbound bandwidth limit.
    bw_out: BandwidthLimit,
    /// Inbound bandwidth limit.
    bw_in: BandwidthLimit,
    /// Generation counter for cache invalidation.
    generation: AtomicU32,
}

impl NetIsolationConfig {
    /// Create a default config that denies everything.
    pub const fn new_deny_all() -> Self {
        Self {
            allowed_ports: [0u16; MAX_ALLOWED_PORTS],
            port_count: 0,
            allowed_hosts: [HostEntry::empty(); MAX_ALLOWED_HOSTS],
            host_count: 0,
            proto_filter: ProtocolFilter::new(ProtocolFilter::NONE),
            bw_out: BandwidthLimit::unlimited(),
            bw_in: BandwidthLimit::unlimited(),
            generation: AtomicU32::new(0),
        }
    }

    /// Create a default config that allows everything.
    pub const fn new_allow_all() -> Self {
        Self {
            allowed_ports: [0u16; MAX_ALLOWED_PORTS],
            port_count: 0,
            allowed_hosts: [HostEntry::empty(); MAX_ALLOWED_HOSTS],
            host_count: 0,
            proto_filter: ProtocolFilter::new(ProtocolFilter::ALL),
            bw_out: BandwidthLimit::unlimited(),
            bw_in: BandwidthLimit::unlimited(),
            generation: AtomicU32::new(0),
        }
    }

    // -----------------------------------------------------------------------
    // Port management
    // -----------------------------------------------------------------------

    /// Add a port to the allowed list.  Returns `false` if full.
    pub fn add_port(&mut self, port: u16) -> bool {
        // Check for duplicate
        for i in 0..self.port_count as usize {
            if self.allowed_ports[i] == port {
                return true; // already present
            }
        }
        if self.port_count as usize >= MAX_ALLOWED_PORTS {
            return false;
        }
        self.allowed_ports[self.port_count as usize] = port;
        self.port_count += 1;
        self.generation.fetch_add(1, Ordering::Release);
        true
    }

    /// Remove a port from the allowed list.
    pub fn remove_port(&mut self, port: u16) -> bool {
        let count = self.port_count as usize;
        for i in 0..count {
            if self.allowed_ports[i] == port {
                for j in i..count.saturating_sub(1) {
                    self.allowed_ports[j] = self.allowed_ports[j + 1];
                }
                self.allowed_ports[count - 1] = 0;
                self.port_count -= 1;
                self.generation.fetch_add(1, Ordering::Release);
                return true;
            }
        }
        false
    }

    /// Check whether a port is allowed.
    pub fn is_port_allowed(&self, port: u16) -> bool {
        // If no ports are listed, the protocol filter decides.
        if self.port_count == 0 {
            return true;
        }
        for i in 0..self.port_count as usize {
            if self.allowed_ports[i] == port {
                return true;
            }
        }
        false
    }

    // -----------------------------------------------------------------------
    // Host management
    // -----------------------------------------------------------------------

    /// Add a host pattern to the allowed list.  Returns `false` if full.
    pub fn add_host(&mut self, entry: HostEntry) -> bool {
        if self.host_count as usize >= MAX_ALLOWED_HOSTS {
            return false;
        }
        self.allowed_hosts[self.host_count as usize] = entry;
        self.host_count += 1;
        self.generation.fetch_add(1, Ordering::Release);
        true
    }

    /// Remove a host pattern.
    pub fn remove_host(&mut self, pattern: &[u8]) -> bool {
        let count = self.host_count as usize;
        for i in 0..count {
            if self.allowed_hosts[i].is_active()
                && Self::host_eq(&self.allowed_hosts[i], pattern)
            {
                for j in i..count.saturating_sub(1) {
                    self.allowed_hosts[j] = self.allowed_hosts[j + 1];
                }
                self.allowed_hosts[count - 1] = HostEntry::empty();
                self.host_count -= 1;
                self.generation.fetch_add(1, Ordering::Release);
                return true;
            }
        }
        false
    }

    /// Check whether a hostname is allowed using glob matching.
    pub fn is_host_allowed(&self, hostname: &[u8]) -> bool {
        if self.host_count == 0 {
            return true;
        }
        for i in 0..self.host_count as usize {
            let entry = &self.allowed_hosts[i];
            if entry.is_active() && glob_match(entry.pattern_str(), hostname) {
                return true;
            }
        }
        false
    }

    // -----------------------------------------------------------------------
    // Protocol filter
    // -----------------------------------------------------------------------

    /// Check whether a protocol is permitted.
    pub fn is_protocol_allowed(&self, proto: Protocol) -> bool {
        self.proto_filter.is_allowed(proto)
    }

    /// Allow a protocol.
    pub fn allow_protocol(&mut self, proto: Protocol) {
        self.proto_filter.allow(proto);
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Deny a protocol.
    pub fn deny_protocol(&mut self, proto: Protocol) {
        self.proto_filter.deny(proto);
        self.generation.fetch_add(1, Ordering::Release);
    }

    // -----------------------------------------------------------------------
    // Bandwidth
    // -----------------------------------------------------------------------

    /// Check outbound bandwidth; returns `true` if the transmission of
    /// `nbytes` at tick `now` is within limits.
    pub fn check_bw_out(&self, nbytes: u64, now: u64) -> bool {
        self.bw_out.check_and_account(nbytes, now)
    }

    /// Check inbound bandwidth.
    pub fn check_bw_in(&self, nbytes: u64, now: u64) -> bool {
        self.bw_in.check_and_account(nbytes, now)
    }

    /// Set outbound bandwidth limit.
    pub fn set_bw_out(&mut self, bytes_per_sec: u64) {
        self.bw_out.set_limit(bytes_per_sec);
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Set inbound bandwidth limit.
    pub fn set_bw_in(&mut self, bytes_per_sec: u64) {
        self.bw_in.set_limit(bytes_per_sec);
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Return the generation counter.
    pub fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }

    /// Port count.
    pub fn port_count(&self) -> u32 {
        self.port_count
    }

    /// Host count.
    pub fn host_count(&self) -> u32 {
        self.host_count
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn host_eq(entry: &HostEntry, pattern: &[u8]) -> bool {
        let p = entry.pattern_str();
        p.len() == pattern.len() && {
            let mut i = 0;
            while i < p.len() {
                if p[i] != pattern[i] {
                    return false;
                }
                i += 1;
            }
            true
        }
    }
}

// ---------------------------------------------------------------------------
// Simple glob matcher for hostnames (supports `*` only)
// ---------------------------------------------------------------------------

/// Glob match for hostnames: `*` matches any sequence of characters,
/// all other bytes are compared literally.
pub fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    let mut pi = 0usize;
    let mut ti = 0usize;
    let mut star_pi: Option<usize> = None;
    let mut star_ti: usize = 0;

    loop {
        if pi == pattern.len() && ti == text.len() {
            return true;
        }
        if pi < pattern.len() && pattern[pi] == b'*' {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
            continue;
        }
        if pi < pattern.len() && ti < text.len() && pattern[pi] == text[ti] {
            pi += 1;
            ti += 1;
            continue;
        }
        if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
            continue;
        }
        return false;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_filter_roundtrip() {
        let mut pf = ProtocolFilter::new(ProtocolFilter::NONE);
        pf.allow(Protocol::Tcp);
        assert!(pf.is_allowed(Protocol::Tcp));
        assert!(!pf.is_allowed(Protocol::Udp));
        pf.deny(Protocol::Tcp);
        assert!(!pf.is_allowed(Protocol::Tcp));
    }

    #[test]
    fn port_management() {
        let mut cfg = NetIsolationConfig::new_deny_all();
        assert!(cfg.add_port(80));
        assert!(cfg.add_port(443));
        assert!(cfg.is_port_allowed(80));
        assert!(!cfg.is_port_allowed(22));
        assert!(cfg.remove_port(80));
        assert!(!cfg.is_port_allowed(80));
    }

    #[test]
    fn host_glob() {
        assert!(glob_match(b"*.example.com", b"www.example.com"));
        assert!(glob_match(b"*.example.com", b".example.com"));
        assert!(!glob_match(b"*.example.com", b"example.com"));
        assert!(glob_match(b"*", b"anything"));
    }

    #[test]
    fn bandwidth_limit() {
        let bw = BandwidthLimit::new(1000, 100);
        assert!(bw.check_and_account(500, 0));
        assert!(!bw.check_and_account(600, 50)); // 500+600 > 1000
        assert!(bw.check_and_account(500, 100)); // new window
    }
}
