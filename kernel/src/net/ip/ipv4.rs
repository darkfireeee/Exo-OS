//! IPv4 protocol implementation.
//!
//! High-performance with:
//! - Zero-copy packet parsing
//! - Fast checksum computation
//! - Inline critical paths

use crate::net::{NetError, NetResult};

/// IPv4 address
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Ipv4Address(pub [u8; 4]);

impl Ipv4Address {
    pub const BROADCAST: Self = Self([255, 255, 255, 255]);
    pub const LOCALHOST: Self = Self([127, 0, 0, 1]);

    #[inline(always)]
    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self([a, b, c, d])
    }

    #[inline(always)]
    pub const fn is_broadcast(&self) -> bool {
        self.0[0] == 255 && self.0[1] == 255 && self.0[2] == 255 && self.0[3] == 255
    }

    #[inline(always)]
    pub const fn is_multicast(&self) -> bool {
        (self.0[0] & 0xF0) == 0xE0
    }
}

/// IPv4 protocol numbers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IpProtocol {
    ICMP = 1,
    TCP = 6,
    UDP = 17,
    Unknown(u8),
}

impl From<u8> for IpProtocol {
    #[inline(always)]
    fn from(value: u8) -> Self {
        match value {
            1 => IpProtocol::ICMP,
            6 => IpProtocol::TCP,
            17 => IpProtocol::UDP,
            _ => IpProtocol::Unknown(value),
        }
    }
}

/// IPv4 packet (zero-copy reference)
#[derive(Debug)]
pub struct Ipv4Packet<'a> {
    buffer: &'a [u8],
}

impl<'a> Ipv4Packet<'a> {
    pub const MIN_HEADER_SIZE: usize = 20;

    /// Parse IPv4 packet from buffer (zero-copy).
    ///
    /// # Performance
    /// Target: < 150 cycles (bounds + checksum validation)
    #[inline]
    pub fn parse(buffer: &'a [u8]) -> NetResult<Self> {
        if buffer.len() < Self::MIN_HEADER_SIZE {
            return Err(NetError::InvalidPacket);
        }

        // Validate version
        if (buffer[0] >> 4) != 4 {
            return Err(NetError::InvalidPacket);
        }

        Ok(Self { buffer })
    }

    /// Gets IP version (always 4).
    #[inline(always)]
    pub fn version(&self) -> u8 {
        self.buffer[0] >> 4
    }

    /// Gets header length in bytes.
    #[inline(always)]
    pub fn header_len(&self) -> usize {
        ((self.buffer[0] & 0x0F) as usize) * 4
    }

    /// Gets total length.
    #[inline(always)]
    pub fn total_len(&self) -> u16 {
        u16::from_be_bytes([self.buffer[2], self.buffer[3]])
    }

    /// Gets TTL (Time To Live).
    #[inline(always)]
    pub fn ttl(&self) -> u8 {
        self.buffer[8]
    }

    /// Gets protocol.
    #[inline(always)]
    pub fn protocol(&self) -> IpProtocol {
        IpProtocol::from(self.buffer[9])
    }

    /// Gets source IP address.
    #[inline(always)]
    pub fn src_addr(&self) -> Ipv4Address {
        Ipv4Address([
            self.buffer[12],
            self.buffer[13],
            self.buffer[14],
            self.buffer[15],
        ])
    }

    /// Gets destination IP address.
    #[inline(always)]
    pub fn dst_addr(&self) -> Ipv4Address {
        Ipv4Address([
            self.buffer[16],
            self.buffer[17],
            self.buffer[18],
            self.buffer[19],
        ])
    }

    /// Gets payload (zero-copy).
    #[inline]
    pub fn payload(&self) -> &[u8] {
        let header_len = self.header_len();
        if header_len >= self.buffer.len() {
            &[]
        } else {
            &self.buffer[header_len..]
        }
    }

    /// Validates header checksum.
    ///
    /// # Performance
    /// Target: < 200 cycles for 20-byte header
    #[inline]
    pub fn verify_checksum(&self) -> bool {
        let header_len = self.header_len();
        if header_len > self.buffer.len() {
            return false;
        }

        checksum(&self.buffer[..header_len]) == 0
    }
}

/// Computes Internet checksum.
///
/// # Performance
/// Uses 32-bit accumulation for better performance.
#[inline]
pub fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;

    // Process 16-bit words
    while i + 1 < data.len() {
        let word = u16::from_be_bytes([data[i], data[i + 1]]);
        sum += word as u32;
        i += 2;
    }

    // Handle odd byte
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }

    // Fold 32-bit sum to 16 bits
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !sum as u16
}
