/// UDP Protocol Implementation
/// 
/// High-performance User Datagram Protocol with:
/// - Zero-copy send/receive
/// - Multicast and broadcast support
/// - Connected and unconnected modes
/// - Hardware checksum offload
/// 
/// Performance targets:
/// - 20M+ packets/sec per core
/// - <5μs latency
/// - Lock-free queues

pub mod socket;
pub mod multicast;

pub use socket::{UdpSocket, UdpSocketError, SocketState, SocketOptions};
pub use multicast::{MulticastManager, MulticastGroup, MulticastError};

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// UDP header (RFC 768) - 8 bytes
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct UdpHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub length: u16,
    pub checksum: u16,
}

impl UdpHeader {
    /// Create a new UDP header
    #[inline]
    pub const fn new(src_port: u16, dst_port: u16, length: u16) -> Self {
        Self {
            src_port: src_port.to_be(),
            dst_port: dst_port.to_be(),
            length: length.to_be(),
            checksum: 0,
        }
    }

    /// Get source port (host byte order)
    #[inline]
    pub fn src_port(&self) -> u16 {
        u16::from_be(self.src_port)
    }

    /// Get destination port (host byte order)
    #[inline]
    pub fn dst_port(&self) -> u16 {
        u16::from_be(self.dst_port)
    }

    /// Get total length (host byte order)
    #[inline]
    pub fn length(&self) -> u16 {
        u16::from_be(self.length)
    }

    /// Get payload length (excludes 8-byte header)
    #[inline]
    pub fn payload_length(&self) -> usize {
        (self.length() as usize).saturating_sub(8)
    }

    /// Get checksum (host byte order)
    #[inline]
    pub fn checksum(&self) -> u16 {
        u16::from_be(self.checksum)
    }

    /// Set checksum
    #[inline]
    pub fn set_checksum(&mut self, checksum: u16) {
        self.checksum = checksum.to_be();
    }

    /// Convert header to bytes
    pub fn to_bytes(&self) -> [u8; 8] {
        unsafe { core::mem::transmute(*self) }
    }

    /// Parse header from bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 8 {
            return None;
        }
        Some(unsafe { core::ptr::read(data.as_ptr() as *const UdpHeader) })
    }

    /// Calculate UDP checksum
    /// 
    /// For IPv4, checksum is optional (can be 0)
    /// For IPv6, checksum is mandatory
    pub fn calculate_checksum(
        &self,
        src_addr: &[u8],
        dst_addr: &[u8],
        payload: &[u8],
    ) -> u16 {
        let mut sum: u32 = 0;

        // Pseudo-header: source address
        for chunk in src_addr.chunks(2) {
            if chunk.len() == 2 {
                sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
            } else {
                sum += (chunk[0] as u32) << 8;
            }
        }

        // Pseudo-header: destination address
        for chunk in dst_addr.chunks(2) {
            if chunk.len() == 2 {
                sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
            } else {
                sum += (chunk[0] as u32) << 8;
            }
        }

        // Pseudo-header: protocol (17 for UDP) and length
        sum += 17;
        sum += self.length() as u32;

        // UDP header (excluding checksum field)
        sum += self.src_port() as u32;
        sum += self.dst_port() as u32;
        sum += self.length() as u32;

        // Payload
        for chunk in payload.chunks(2) {
            if chunk.len() == 2 {
                sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
            } else {
                sum += (chunk[0] as u32) << 8;
            }
        }

        // Fold 32-bit sum to 16 bits
        while sum >> 16 != 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }

        // One's complement
        !sum as u16
    }
}

/// UDP datagram
#[derive(Debug)]
pub struct UdpDatagram {
    /// UDP header
    pub header: UdpHeader,
    /// Payload data
    pub payload: Vec<u8>,
}

impl UdpDatagram {
    /// Create a new UDP datagram
    pub fn new(src_port: u16, dst_port: u16, payload: Vec<u8>) -> Self {
        let length = 8 + payload.len() as u16;
        Self {
            header: UdpHeader::new(src_port, dst_port, length),
            payload,
        }
    }

    /// Get the total size of the datagram
    pub fn size(&self) -> usize {
        8 + self.payload.len()
    }

    /// Serialize the datagram to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.size());
        bytes.extend_from_slice(&self.header.to_bytes());
        bytes.extend_from_slice(&self.payload);
        bytes
    }

    /// Parse a datagram from bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 8 {
            return None;
        }

        let header = UdpHeader::from_bytes(data)?;
        let payload_len = header.payload_length();

        if data.len() < 8 + payload_len {
            return None;
        }

        let payload = data[8..8 + payload_len].to_vec();

        Some(Self { header, payload })
    }
}

/// UDP statistics
#[derive(Debug)]
pub struct UdpStats {
    /// Datagrams sent
    pub datagrams_sent: AtomicU64,
    /// Datagrams received
    pub datagrams_received: AtomicU64,
    /// Datagrams dropped (no port)
    pub datagrams_no_port: AtomicU64,
    /// Datagrams dropped (checksum error)
    pub datagrams_checksum_error: AtomicU64,
    /// Datagrams dropped (buffer full)
    pub datagrams_buffer_full: AtomicU64,
    /// Total bytes sent
    pub bytes_sent: AtomicU64,
    /// Total bytes received
    pub bytes_received: AtomicU64,
}

impl UdpStats {
    pub fn new() -> Self {
        Self {
            datagrams_sent: AtomicU64::new(0),
            datagrams_received: AtomicU64::new(0),
            datagrams_no_port: AtomicU64::new(0),
            datagrams_checksum_error: AtomicU64::new(0),
            datagrams_buffer_full: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> UdpStatsSnapshot {
        UdpStatsSnapshot {
            datagrams_sent: self.datagrams_sent.load(Ordering::Relaxed),
            datagrams_received: self.datagrams_received.load(Ordering::Relaxed),
            datagrams_no_port: self.datagrams_no_port.load(Ordering::Relaxed),
            datagrams_checksum_error: self.datagrams_checksum_error.load(Ordering::Relaxed),
            datagrams_buffer_full: self.datagrams_buffer_full.load(Ordering::Relaxed),
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot of UDP statistics
#[derive(Debug, Clone, Copy)]
pub struct UdpStatsSnapshot {
    pub datagrams_sent: u64,
    pub datagrams_received: u64,
    pub datagrams_no_port: u64,
    pub datagrams_checksum_error: u64,
    pub datagrams_buffer_full: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
}

/// Global UDP statistics
static UDP_STATS: Mutex<UdpStats> = Mutex::new(UdpStats {
    datagrams_sent: AtomicU64::new(0),
    datagrams_received: AtomicU64::new(0),
    datagrams_no_port: AtomicU64::new(0),
    datagrams_checksum_error: AtomicU64::new(0),
    datagrams_buffer_full: AtomicU64::new(0),
    bytes_sent: AtomicU64::new(0),
    bytes_received: AtomicU64::new(0),
});

/// Get UDP statistics
pub fn get_stats() -> UdpStatsSnapshot {
    UDP_STATS.lock().snapshot()
}

/// Initialize UDP protocol
pub fn init() {
    multicast::init();
}
