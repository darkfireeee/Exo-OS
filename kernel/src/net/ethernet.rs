//! Ethernet Layer
//!
//! Ethernet frame handling and MAC address management

use super::buffer::PacketBuffer;

/// Ethernet frame header (14 bytes)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct EthernetHeader {
    /// Destination MAC address
    pub dst_mac: [u8; 6],
    
    /// Source MAC address
    pub src_mac: [u8; 6],
    
    /// EtherType (protocol)
    pub ether_type: u16,
}

/// EtherType values
pub mod ether_type {
    pub const IPV4: u16 = 0x0800;
    pub const ARP: u16 = 0x0806;
    pub const IPV6: u16 = 0x86DD;
}

impl EthernetHeader {
    pub const SIZE: usize = 14;
    
    /// Create new Ethernet header
    pub fn new(dst_mac: [u8; 6], src_mac: [u8; 6], ether_type: u16) -> Self {
        Self {
            dst_mac,
            src_mac,
            ether_type: ether_type.to_be(),
        }
    }
    
    /// Parse from packet buffer
    pub fn parse(data: &[u8]) -> Result<Self, EthernetError> {
        if data.len() < Self::SIZE {
            return Err(EthernetError::TooShort);
        }
        
        Ok(Self {
            dst_mac: [data[0], data[1], data[2], data[3], data[4], data[5]],
            src_mac: [data[6], data[7], data[8], data[9], data[10], data[11]],
            ether_type: u16::from_be_bytes([data[12], data[13]]),
        })
    }
    
    /// Write to buffer
    pub fn write(&self, buffer: &mut [u8]) -> Result<(), EthernetError> {
        if buffer.len() < Self::SIZE {
            return Err(EthernetError::BufferTooSmall);
        }
        
        buffer[0..6].copy_from_slice(&self.dst_mac);
        buffer[6..12].copy_from_slice(&self.src_mac);
        buffer[12..14].copy_from_slice(&self.ether_type.to_be_bytes());
        
        Ok(())
    }
    
    /// Get EtherType
    pub fn protocol(&self) -> u16 {
        u16::from_be(self.ether_type)
    }
    
    /// Check if broadcast
    pub fn is_broadcast(&self) -> bool {
        self.dst_mac == [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]
    }
    
    /// Check if multicast
    pub fn is_multicast(&self) -> bool {
        self.dst_mac[0] & 0x01 != 0
    }
}

/// Ethernet errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EthernetError {
    TooShort,
    BufferTooSmall,
    InvalidMac,
    InvalidEtherType,
}

/// MAC address utilities
pub struct MacAddr(pub [u8; 6]);

impl MacAddr {
    /// Broadcast MAC
    pub const fn broadcast() -> Self {
        Self([0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF])
    }
    
    /// Zero MAC
    pub const fn zero() -> Self {
        Self([0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
    }
    
    /// Check if broadcast
    pub fn is_broadcast(&self) -> bool {
        self.0 == [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]
    }
    
    /// Check if multicast
    pub fn is_multicast(&self) -> bool {
        self.0[0] & 0x01 != 0
    }
    
    /// Check if unicast
    pub fn is_unicast(&self) -> bool {
        !self.is_multicast() && !self.is_broadcast()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_ethernet_header() {
        let header = EthernetHeader::new(
            [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
            [0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
            ether_type::IPV4,
        );
        
        assert!(header.is_broadcast());
        assert_eq!(header.protocol(), ether_type::IPV4);
    }
    
    #[test]
    fn test_mac_addr() {
        let broadcast = MacAddr::broadcast();
        assert!(broadcast.is_broadcast());
        
        let unicast = MacAddr([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        assert!(unicast.is_unicast());
    }
}
