//! Ethernet Layer 2 implementation.
//!
//! High-performance with:
//! - Zero-copy frame parsing
//! - Cache-aligned structures
//! - Inline fast paths

use crate::net::{NetError, NetResult};

/// MAC address (48 bits)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct MacAddress(pub [u8; 6]);

impl MacAddress {
    pub const BROADCAST: Self = Self([0xFF; 6]);

    #[inline(always)]
    pub const fn new(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }

    #[inline(always)]
    pub const fn is_broadcast(&self) -> bool {
        self.0[0] == 0xFF
            && self.0[1] == 0xFF
            && self.0[2] == 0xFF
            && self.0[3] == 0xFF
            && self.0[4] == 0xFF
            && self.0[5] == 0xFF
    }

    #[inline(always)]
    pub const fn is_multicast(&self) -> bool {
        (self.0[0] & 0x01) != 0
    }
}

/// EtherType values
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum EtherType {
    IPv4 = 0x0800,
    ARP = 0x0806,
    IPv6 = 0x86DD,
    Unknown(u16),
}

impl From<u16> for EtherType {
    #[inline]
    fn from(value: u16) -> Self {
        match value {
            0x0800 => EtherType::IPv4,
            0x0806 => EtherType::ARP,
            0x86DD => EtherType::IPv6,
            _ => EtherType::Unknown(value),
        }
    }
}

impl Into<u16> for EtherType {
    #[inline(always)]
    fn into(self) -> u16 {
        match self {
            EtherType::IPv4 => 0x0800,
            EtherType::ARP => 0x0806,
            EtherType::IPv6 => 0x86DD,
            EtherType::Unknown(v) => v,
        }
    }
}

/// Ethernet frame (zero-copy reference to buffer)
#[derive(Debug)]
pub struct EthernetFrame<'a> {
    buffer: &'a [u8],
}

impl<'a> EthernetFrame<'a> {
    /// Minimum Ethernet frame size (excludes FCS)
    pub const MIN_SIZE: usize = 14;

    /// Parse Ethernet frame from buffer (zero-copy).
    ///
    /// # Performance
    /// Target: < 100 cycles (just bounds checking)
    #[inline]
    pub fn parse(buffer: &'a [u8]) -> NetResult<Self> {
        if buffer.len() < Self::MIN_SIZE {
            return Err(NetError::InvalidPacket);
        }
        Ok(Self { buffer })
    }

    /// Gets destination MAC address.
    #[inline(always)]
    pub fn dst_mac(&self) -> MacAddress {
        MacAddress([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
            self.buffer[4],
            self.buffer[5],
        ])
    }

    /// Gets source MAC address.
    #[inline(always)]
    pub fn src_mac(&self) -> MacAddress {
        MacAddress([
            self.buffer[6],
            self.buffer[7],
            self.buffer[8],
            self.buffer[9],
            self.buffer[10],
            self.buffer[11],
        ])
    }

    /// Gets EtherType.
    #[inline(always)]
    pub fn ether_type(&self) -> EtherType {
        let value = u16::from_be_bytes([self.buffer[12], self.buffer[13]]);
        EtherType::from(value)
    }

    /// Gets payload (zero-copy reference).
    #[inline(always)]
    pub fn payload(&self) -> &[u8] {
        &self.buffer[Self::MIN_SIZE..]
    }
}

/// Mutable Ethernet frame for construction
pub struct EthernetFrameMut<'a> {
    buffer: &'a mut [u8],
}

impl<'a> EthernetFrameMut<'a> {
    /// Creates a new Ethernet frame in the buffer.
    #[inline]
    pub fn new(buffer: &'a mut [u8]) -> NetResult<Self> {
        if buffer.len() < EthernetFrame::MIN_SIZE {
            return Err(NetError::InvalidPacket);
        }
        Ok(Self { buffer })
    }

    /// Sets destination MAC.
    #[inline(always)]
    pub fn set_dst_mac(&mut self, mac: MacAddress) {
        self.buffer[0..6].copy_from_slice(&mac.0);
    }

    /// Sets source MAC.
    #[inline(always)]
    pub fn set_src_mac(&mut self, mac: MacAddress) {
        self.buffer[6..12].copy_from_slice(&mac.0);
    }

    /// Sets EtherType.
    #[inline(always)]
    pub fn set_ether_type(&mut self, ether_type: EtherType) {
        let value: u16 = ether_type.into();
        self.buffer[12..14].copy_from_slice(&value.to_be_bytes());
    }

    /// Gets mutable payload reference.
    #[inline(always)]
    pub fn payload_mut(&mut self) -> &mut [u8] {
        &mut self.buffer[EthernetFrame::MIN_SIZE..]
    }
}
