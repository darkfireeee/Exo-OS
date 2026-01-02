//! IPv4 Protocol Implementation
//!
//! Complete IPv4 stack with routing and ICMP

use super::buffer::PacketBuffer;
use super::socket::Ipv4Addr;

/// IPv4 header (20 bytes minimum)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Ipv4Header {
    /// Version (4 bits) + IHL (4 bits)
    pub version_ihl: u8,
    
    /// Type of Service
    pub tos: u8,
    
    /// Total length (header + data)
    pub total_length: u16,
    
    /// Identification
    pub identification: u16,
    
    /// Flags (3 bits) + Fragment offset (13 bits)
    pub flags_fragment: u16,
    
    /// Time to Live
    pub ttl: u8,
    
    /// Protocol (TCP=6, UDP=17, ICMP=1)
    pub protocol: u8,
    
    /// Header checksum
    pub checksum: u16,
    
    /// Source IP address
    pub src_addr: [u8; 4],
    
    /// Destination IP address
    pub dst_addr: [u8; 4],
}

/// IP protocols
pub mod protocol {
    pub const ICMP: u8 = 1;
    pub const TCP: u8 = 6;
    pub const UDP: u8 = 17;
}

impl Ipv4Header {
    pub const MIN_SIZE: usize = 20;
    
    /// Create new IPv4 header
    pub fn new(
        src: Ipv4Addr,
        dst: Ipv4Addr,
        protocol: u8,
        payload_len: u16,
    ) -> Self {
        let total_len = Self::MIN_SIZE as u16 + payload_len;
        
        let mut header = Self {
            version_ihl: 0x45, // Version 4, IHL 5 (20 bytes)
            tos: 0,
            total_length: total_len.to_be(),
            identification: 0,
            flags_fragment: 0,
            ttl: 64,
            protocol,
            checksum: 0,
            src_addr: src.0,
            dst_addr: dst.0,
        };
        
        // Calculate checksum
        header.checksum = header.calculate_checksum().to_be();
        
        header
    }
    
    /// Parse from buffer
    pub fn parse(data: &[u8]) -> Result<Self, IpError> {
        if data.len() < Self::MIN_SIZE {
            return Err(IpError::TooShort);
        }
        
        let header = Self {
            version_ihl: data[0],
            tos: data[1],
            total_length: u16::from_be_bytes([data[2], data[3]]),
            identification: u16::from_be_bytes([data[4], data[5]]),
            flags_fragment: u16::from_be_bytes([data[6], data[7]]),
            ttl: data[8],
            protocol: data[9],
            checksum: u16::from_be_bytes([data[10], data[11]]),
            src_addr: [data[12], data[13], data[14], data[15]],
            dst_addr: [data[16], data[17], data[18], data[19]],
        };
        
        // Validate version
        if header.version() != 4 {
            return Err(IpError::InvalidVersion);
        }
        
        // Verify checksum
        if !header.verify_checksum() {
            return Err(IpError::InvalidChecksum);
        }
        
        Ok(header)
    }
    
    /// Write to buffer
    pub fn write(&self, buffer: &mut [u8]) -> Result<(), IpError> {
        if buffer.len() < Self::MIN_SIZE {
            return Err(IpError::BufferTooSmall);
        }
        
        buffer[0] = self.version_ihl;
        buffer[1] = self.tos;
        buffer[2..4].copy_from_slice(&self.total_length.to_be_bytes());
        buffer[4..6].copy_from_slice(&self.identification.to_be_bytes());
        buffer[6..8].copy_from_slice(&self.flags_fragment.to_be_bytes());
        buffer[8] = self.ttl;
        buffer[9] = self.protocol;
        buffer[10..12].copy_from_slice(&self.checksum.to_be_bytes());
        buffer[12..16].copy_from_slice(&self.src_addr);
        buffer[16..20].copy_from_slice(&self.dst_addr);
        
        Ok(())
    }
    
    /// Get IP version
    pub fn version(&self) -> u8 {
        self.version_ihl >> 4
    }
    
    /// Get header length in bytes
    pub fn header_len(&self) -> usize {
        ((self.version_ihl & 0x0F) * 4) as usize
    }
    
    /// Get total length
    pub fn total_len(&self) -> u16 {
        u16::from_be(self.total_length)
    }
    
    /// Get payload length
    pub fn payload_len(&self) -> usize {
        self.total_len() as usize - self.header_len()
    }
    
    /// Calculate checksum
    pub fn calculate_checksum(&self) -> u16 {
        let mut sum: u32 = 0;
        
        // Convert header to u16 array
        let header_bytes = unsafe {
            core::slice::from_raw_parts(
                self as *const _ as *const u8,
                Self::MIN_SIZE,
            )
        };
        
        for i in (0..Self::MIN_SIZE).step_by(2) {
            if i == 10 {
                // Skip checksum field
                continue;
            }
            
            let word = u16::from_be_bytes([header_bytes[i], header_bytes[i + 1]]);
            sum += word as u32;
        }
        
        // Fold 32-bit sum to 16 bits
        while (sum >> 16) != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        
        !sum as u16
    }
    
    /// Verify checksum
    pub fn verify_checksum(&self) -> bool {
        let calculated = self.calculate_checksum();
        u16::from_be(self.checksum) == calculated
    }
    
    /// Get source address
    pub fn src(&self) -> Ipv4Addr {
        Ipv4Addr(self.src_addr)
    }
    
    /// Get destination address
    pub fn dst(&self) -> Ipv4Addr {
        Ipv4Addr(self.dst_addr)
    }
}

/// IP errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpError {
    TooShort,
    BufferTooSmall,
    InvalidVersion,
    InvalidChecksum,
    InvalidAddress,
    RoutingFailed,
}

/// ICMP packet types
pub mod icmp {
    pub const ECHO_REPLY: u8 = 0;
    pub const ECHO_REQUEST: u8 = 8;
    pub const DEST_UNREACHABLE: u8 = 3;
    pub const TIME_EXCEEDED: u8 = 11;
}

/// ICMP header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IcmpHeader {
    /// ICMP type
    pub icmp_type: u8,
    
    /// ICMP code
    pub code: u8,
    
    /// Checksum
    pub checksum: u16,
    
    /// Identifier (for echo)
    pub identifier: u16,
    
    /// Sequence number (for echo)
    pub sequence: u16,
}

impl IcmpHeader {
    pub const SIZE: usize = 8;
    
    /// Create echo request
    pub fn echo_request(identifier: u16, sequence: u16) -> Self {
        let mut header = Self {
            icmp_type: icmp::ECHO_REQUEST,
            code: 0,
            checksum: 0,
            identifier: identifier.to_be(),
            sequence: sequence.to_be(),
        };
        
        header.checksum = header.calculate_checksum(&[]).to_be();
        
        header
    }
    
    /// Calculate checksum
    pub fn calculate_checksum(&self, data: &[u8]) -> u16 {
        let mut sum: u32 = 0;
        
        // Header
        let header_bytes = unsafe {
            core::slice::from_raw_parts(
                self as *const _ as *const u8,
                Self::SIZE,
            )
        };
        
        for i in (0..Self::SIZE).step_by(2) {
            if i == 2 {
                // Skip checksum field
                continue;
            }
            
            let word = u16::from_be_bytes([header_bytes[i], header_bytes[i + 1]]);
            sum += word as u32;
        }
        
        // Data
        for i in (0..data.len()).step_by(2) {
            let word = if i + 1 < data.len() {
                u16::from_be_bytes([data[i], data[i + 1]])
            } else {
                u16::from_be_bytes([data[i], 0])
            };
            sum += word as u32;
        }
        
        // Fold
        while (sum >> 16) != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        
        !sum as u16
    }
}

/// Simple routing table
pub struct RoutingTable {
    default_gateway: Option<Ipv4Addr>,
}

impl RoutingTable {
    pub const fn new() -> Self {
        Self {
            default_gateway: None,
        }
    }
    
    /// Set default gateway
    pub fn set_default_gateway(&mut self, gateway: Ipv4Addr) {
        self.default_gateway = Some(gateway);
    }
    
    /// Route packet
    pub fn route(&self, dst: Ipv4Addr) -> Option<Ipv4Addr> {
        // Simple: always use default gateway
        self.default_gateway
    }
}

/// Global routing table
pub static ROUTING_TABLE: spin::Mutex<RoutingTable> = spin::Mutex::new(RoutingTable::new());

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_ipv4_header() {
        let src = Ipv4Addr::new(192, 168, 1, 1);
        let dst = Ipv4Addr::new(8, 8, 8, 8);
        
        let header = Ipv4Header::new(src, dst, protocol::TCP, 100);
        
        assert_eq!(header.version(), 4);
        assert_eq!(header.header_len(), 20);
        assert_eq!(header.total_len(), 120);
        assert!(header.verify_checksum());
    }
    
    #[test]
    fn test_icmp_echo() {
        let icmp = IcmpHeader::echo_request(1234, 1);
        
        assert_eq!(icmp.icmp_type, icmp::ECHO_REQUEST);
        assert_eq!(u16::from_be(icmp.identifier), 1234);
        assert_eq!(u16::from_be(icmp.sequence), 1);
    }
}
