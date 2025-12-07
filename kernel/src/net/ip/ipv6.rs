//! # IPv6 Protocol Implementation
//! 
//! RFC 8200 - Internet Protocol, Version 6 (IPv6)
//! 
//! High-performance with:
//! - Zero-copy packet parsing
//! - Extension headers support
//! - Flow label handling

use crate::net::{NetError, NetResult};

/// IPv6 address (128 bits)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Ipv6Address(pub [u8; 16]);

impl Ipv6Address {
    pub const LOCALHOST: Self = Self([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    pub const UNSPECIFIED: Self = Self([0; 16]);
    
    #[inline(always)]
    pub const fn new(segments: [u16; 8]) -> Self {
        Self([
            (segments[0] >> 8) as u8, (segments[0] & 0xFF) as u8,
            (segments[1] >> 8) as u8, (segments[1] & 0xFF) as u8,
            (segments[2] >> 8) as u8, (segments[2] & 0xFF) as u8,
            (segments[3] >> 8) as u8, (segments[3] & 0xFF) as u8,
            (segments[4] >> 8) as u8, (segments[4] & 0xFF) as u8,
            (segments[5] >> 8) as u8, (segments[5] & 0xFF) as u8,
            (segments[6] >> 8) as u8, (segments[6] & 0xFF) as u8,
            (segments[7] >> 8) as u8, (segments[7] & 0xFF) as u8,
        ])
    }
    
    #[inline(always)]
    pub const fn is_unspecified(&self) -> bool {
        self.0 == [0; 16]
    }
    
    #[inline(always)]
    pub const fn is_loopback(&self) -> bool {
        self.0[0..15] == [0; 15] && self.0[15] == 1
    }
    
    #[inline(always)]
    pub const fn is_multicast(&self) -> bool {
        self.0[0] == 0xFF
    }
    
    #[inline(always)]
    pub const fn is_link_local(&self) -> bool {
        self.0[0] == 0xFE && (self.0[1] & 0xC0) == 0x80
    }
    
    /// Check if this is an IPv4-mapped IPv6 address (::ffff:0:0/96)
    #[inline(always)]
    pub const fn is_ipv4_mapped(&self) -> bool {
        self.0[0..10] == [0; 10] && self.0[10] == 0xFF && self.0[11] == 0xFF
    }
    
    /// Extract IPv4 address if this is IPv4-mapped
    #[inline(always)]
    pub const fn to_ipv4(&self) -> Option<[u8; 4]> {
        if self.is_ipv4_mapped() {
            Some([self.0[12], self.0[13], self.0[14], self.0[15]])
        } else {
            None
        }
    }
}

/// IPv6 next header values
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NextHeader {
    HopByHop = 0,
    TCP = 6,
    UDP = 17,
    IPv6 = 41,
    Routing = 43,
    Fragment = 44,
    ESP = 50,
    AH = 51,
    ICMPv6 = 58,
    NoNextHeader = 59,
    DestinationOptions = 60,
    Unknown(u8),
}

impl From<u8> for NextHeader {
    fn from(value: u8) -> Self {
        match value {
            0 => NextHeader::HopByHop,
            6 => NextHeader::TCP,
            17 => NextHeader::UDP,
            41 => NextHeader::IPv6,
            43 => NextHeader::Routing,
            44 => NextHeader::Fragment,
            50 => NextHeader::ESP,
            51 => NextHeader::AH,
            58 => NextHeader::ICMPv6,
            59 => NextHeader::NoNextHeader,
            60 => NextHeader::DestinationOptions,
            _ => NextHeader::Unknown(value),
        }
    }
}

/// IPv6 packet (zero-copy reference)
#[derive(Debug)]
pub struct Ipv6Packet<'a> {
    buffer: &'a [u8],
}

impl<'a> Ipv6Packet<'a> {
    pub const HEADER_SIZE: usize = 40;
    
    /// Parse IPv6 packet from buffer (zero-copy)
    #[inline]
    pub fn parse(buffer: &'a [u8]) -> NetResult<Self> {
        if buffer.len() < Self::HEADER_SIZE {
            return Err(NetError::InvalidPacket);
        }
        
        // Validate version
        if (buffer[0] >> 4) != 6 {
            return Err(NetError::InvalidPacket);
        }
        
        Ok(Self { buffer })
    }
    
    /// Get IP version (always 6)
    #[inline(always)]
    pub fn version(&self) -> u8 {
        self.buffer[0] >> 4
    }
    
    /// Get traffic class (8 bits for QoS)
    #[inline(always)]
    pub fn traffic_class(&self) -> u8 {
        ((self.buffer[0] & 0x0F) << 4) | (self.buffer[1] >> 4)
    }
    
    /// Get flow label (20 bits)
    #[inline(always)]
    pub fn flow_label(&self) -> u32 {
        (((self.buffer[1] & 0x0F) as u32) << 16)
            | ((self.buffer[2] as u32) << 8)
            | (self.buffer[3] as u32)
    }
    
    /// Get payload length (excludes header)
    #[inline(always)]
    pub fn payload_length(&self) -> u16 {
        u16::from_be_bytes([self.buffer[4], self.buffer[5]])
    }
    
    /// Get next header
    #[inline(always)]
    pub fn next_header(&self) -> NextHeader {
        NextHeader::from(self.buffer[6])
    }
    
    /// Get hop limit
    #[inline(always)]
    pub fn hop_limit(&self) -> u8 {
        self.buffer[7]
    }
    
    /// Get source address
    #[inline(always)]
    pub fn src_addr(&self) -> Ipv6Address {
        let mut addr = [0u8; 16];
        addr.copy_from_slice(&self.buffer[8..24]);
        Ipv6Address(addr)
    }
    
    /// Get destination address
    #[inline(always)]
    pub fn dst_addr(&self) -> Ipv6Address {
        let mut addr = [0u8; 16];
        addr.copy_from_slice(&self.buffer[24..40]);
        Ipv6Address(addr)
    }
    
    /// Get payload (zero-copy)
    #[inline]
    pub fn payload(&self) -> &[u8] {
        if self.buffer.len() <= Self::HEADER_SIZE {
            &[]
        } else {
            &self.buffer[Self::HEADER_SIZE..]
        }
    }
}

/// IPv6 header (mutable)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Ipv6Header {
    pub version_tc_flow: [u8; 4],
    pub payload_length: u16,
    pub next_header: u8,
    pub hop_limit: u8,
    pub src_addr: [u8; 16],
    pub dst_addr: [u8; 16],
}

impl Ipv6Header {
    pub fn new(
        traffic_class: u8,
        flow_label: u32,
        payload_length: u16,
        next_header: u8,
        hop_limit: u8,
        src_addr: Ipv6Address,
        dst_addr: Ipv6Address,
    ) -> Self {
        // Version (6) + Traffic Class + Flow Label
        let mut version_tc_flow = [0u8; 4];
        version_tc_flow[0] = (6 << 4) | (traffic_class >> 4);
        version_tc_flow[1] = ((traffic_class & 0x0F) << 4) | ((flow_label >> 16) & 0x0F) as u8;
        version_tc_flow[2] = ((flow_label >> 8) & 0xFF) as u8;
        version_tc_flow[3] = (flow_label & 0xFF) as u8;
        
        Self {
            version_tc_flow,
            payload_length: payload_length.to_be(),
            next_header,
            hop_limit,
            src_addr: src_addr.0,
            dst_addr: dst_addr.0,
        }
    }
}

/// Extension header types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionHeader {
    HopByHop,
    Routing,
    Fragment,
    DestinationOptions,
    Authentication,
    EncapsulatingSecurityPayload,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_ipv6_address() {
        let localhost = Ipv6Address::LOCALHOST;
        assert!(localhost.is_loopback());
        assert!(!localhost.is_multicast());
        
        let unspec = Ipv6Address::UNSPECIFIED;
        assert!(unspec.is_unspecified());
    }
    
    #[test]
    fn test_ipv4_mapped() {
        // ::ffff:192.168.1.1
        let mapped = Ipv6Address([
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF,
            192, 168, 1, 1
        ]);
        
        assert!(mapped.is_ipv4_mapped());
        assert_eq!(mapped.to_ipv4(), Some([192, 168, 1, 1]));
    }
    
    #[test]
    fn test_parse_ipv6() {
        let mut packet = vec![0u8; 60];
        
        // Version 6, TC=0, FL=0
        packet[0] = 0x60;
        
        // Payload length = 20
        packet[4] = 0;
        packet[5] = 20;
        
        // Next header = TCP (6)
        packet[6] = 6;
        
        // Hop limit = 64
        packet[7] = 64;
        
        // Source = ::1
        packet[23] = 1;
        
        // Destination = ::1
        packet[39] = 1;
        
        let parsed = Ipv6Packet::parse(&packet).unwrap();
        
        assert_eq!(parsed.version(), 6);
        assert_eq!(parsed.payload_length(), 20);
        assert_eq!(parsed.hop_limit(), 64);
        assert!(parsed.src_addr().is_loopback());
        assert!(parsed.dst_addr().is_loopback());
    }
}
