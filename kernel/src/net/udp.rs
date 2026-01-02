//! UDP Protocol Implementation
//!
//! User Datagram Protocol (connectionless)

use super::buffer::PacketBuffer;
use super::socket::Ipv4Addr;

/// UDP header (8 bytes)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct UdpHeader {
    /// Source port
    pub src_port: u16,
    
    /// Destination port
    pub dst_port: u16,
    
    /// Length (header + data)
    pub length: u16,
    
    /// Checksum
    pub checksum: u16,
}

impl UdpHeader {
    pub const SIZE: usize = 8;
    
    /// Create new UDP header
    pub fn new(src_port: u16, dst_port: u16, payload_len: u16) -> Self {
        Self {
            src_port: src_port.to_be(),
            dst_port: dst_port.to_be(),
            length: (Self::SIZE as u16 + payload_len).to_be(),
            checksum: 0, // Optional in IPv4
        }
    }
    
    /// Parse from buffer
    pub fn parse(data: &[u8]) -> Result<Self, UdpError> {
        if data.len() < Self::SIZE {
            return Err(UdpError::TooShort);
        }
        
        Ok(Self {
            src_port: u16::from_be_bytes([data[0], data[1]]),
            dst_port: u16::from_be_bytes([data[2], data[3]]),
            length: u16::from_be_bytes([data[4], data[5]]),
            checksum: u16::from_be_bytes([data[6], data[7]]),
        })
    }
    
    /// Write to buffer
    pub fn write(&self, buffer: &mut [u8]) -> Result<(), UdpError> {
        if buffer.len() < Self::SIZE {
            return Err(UdpError::BufferTooSmall);
        }
        
        buffer[0..2].copy_from_slice(&self.src_port.to_be_bytes());
        buffer[2..4].copy_from_slice(&self.dst_port.to_be_bytes());
        buffer[4..6].copy_from_slice(&self.length.to_be_bytes());
        buffer[6..8].copy_from_slice(&self.checksum.to_be_bytes());
        
        Ok(())
    }
    
    /// Get source port
    pub fn src_port(&self) -> u16 {
        u16::from_be(self.src_port)
    }
    
    /// Get destination port
    pub fn dst_port(&self) -> u16 {
        u16::from_be(self.dst_port)
    }
    
    /// Get total length
    pub fn length(&self) -> u16 {
        u16::from_be(self.length)
    }
    
    /// Get payload length
    pub fn payload_len(&self) -> usize {
        self.length() as usize - Self::SIZE
    }
    
    /// Calculate checksum (with pseudo-header)
    pub fn calculate_checksum(
        &self,
        src_ip: Ipv4Addr,
        dst_ip: Ipv4Addr,
        data: &[u8],
    ) -> u16 {
        let mut sum: u32 = 0;
        
        // Pseudo-header
        for i in 0..4 {
            sum += src_ip.0[i] as u32;
            sum += dst_ip.0[i] as u32;
        }
        sum += 17; // UDP protocol number
        sum += self.length() as u32;
        
        // UDP header
        sum += self.src_port() as u32;
        sum += self.dst_port() as u32;
        sum += self.length() as u32;
        // Skip checksum field
        
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

/// UDP errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UdpError {
    TooShort,
    BufferTooSmall,
    InvalidChecksum,
    InvalidPort,
    PortInUse,
}

/// UDP socket (minimal)
pub struct UdpSocket {
    local_port: u16,
    remote_addr: Option<(Ipv4Addr, u16)>,
}

impl UdpSocket {
    /// Create new UDP socket
    pub fn new() -> Self {
        Self {
            local_port: 0,
            remote_addr: None,
        }
    }
    
    /// Bind to local port
    pub fn bind(&mut self, port: u16) -> Result<(), UdpError> {
        if port == 0 {
            return Err(UdpError::InvalidPort);
        }
        
        // TODO: Check if port in use
        self.local_port = port;
        Ok(())
    }
    
    /// Connect to remote address
    pub fn connect(&mut self, addr: Ipv4Addr, port: u16) -> Result<(), UdpError> {
        self.remote_addr = Some((addr, port));
        Ok(())
    }
    
    /// Send datagram
    pub fn send(&self, data: &[u8]) -> Result<usize, UdpError> {
        if let Some((addr, port)) = self.remote_addr {
            // TODO: Actual send via IP layer
            Ok(data.len())
        } else {
            Err(UdpError::InvalidPort)
        }
    }
    
    /// Receive datagram
    pub fn recv(&self, buffer: &mut [u8]) -> Result<usize, UdpError> {
        // TODO: Actual receive
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_udp_header() {
        let header = UdpHeader::new(1234, 5678, 100);
        
        assert_eq!(header.src_port(), 1234);
        assert_eq!(header.dst_port(), 5678);
        assert_eq!(header.length(), 108); // 8 + 100
        assert_eq!(header.payload_len(), 100);
    }
    
    #[test]
    fn test_udp_socket() {
        let mut socket = UdpSocket::new();
        
        socket.bind(1234).unwrap();
        assert_eq!(socket.local_port, 1234);
        
        let addr = Ipv4Addr::new(127, 0, 0, 1);
        socket.connect(addr, 5678).unwrap();
        assert_eq!(socket.remote_addr, Some((addr, 5678)));
    }
}
