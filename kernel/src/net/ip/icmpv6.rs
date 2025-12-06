//! # ICMPv6 - Internet Control Message Protocol for IPv6
//! 
//! RFC 4443 - ICMPv6 specification
//! RFC 4861 - Neighbor Discovery Protocol (NDP)

use alloc::vec::Vec;

/// ICMPv6 Message Types (RFC 4443)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Icmpv6Type {
    // Error Messages (0-127)
    DestinationUnreachable = 1,
    PacketTooBig = 2,
    TimeExceeded = 3,
    ParameterProblem = 4,
    
    // Informational Messages (128-255)
    EchoRequest = 128,
    EchoReply = 129,
    
    // Neighbor Discovery Protocol (RFC 4861)
    RouterSolicitation = 133,
    RouterAdvertisement = 134,
    NeighborSolicitation = 135,
    NeighborAdvertisement = 136,
    Redirect = 137,
    
    Unknown(u8),
}

impl From<u8> for Icmpv6Type {
    fn from(val: u8) -> Self {
        match val {
            1 => Self::DestinationUnreachable,
            2 => Self::PacketTooBig,
            3 => Self::TimeExceeded,
            4 => Self::ParameterProblem,
            128 => Self::EchoRequest,
            129 => Self::EchoReply,
            133 => Self::RouterSolicitation,
            134 => Self::RouterAdvertisement,
            135 => Self::NeighborSolicitation,
            136 => Self::NeighborAdvertisement,
            137 => Self::Redirect,
            other => Self::Unknown(other),
        }
    }
}

/// ICMPv6 Header (RFC 4443)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Icmpv6Header {
    pub msg_type: u8,
    pub code: u8,
    pub checksum: u16,
}

impl Icmpv6Header {
    pub const SIZE: usize = 4;
    
    pub fn new(msg_type: Icmpv6Type, code: u8) -> Self {
        Self {
            msg_type: msg_type as u8,
            code,
            checksum: 0,
        }
    }
    
    /// Calcule checksum (RFC 4443)
    pub fn calculate_checksum(
        &mut self,
        src_addr: &[u8; 16],
        dst_addr: &[u8; 16],
        payload: &[u8],
    ) {
        self.checksum = 0;
        
        // Pseudo-header + ICMPv6 message
        let mut sum = 0u32;
        
        // Source address
        for chunk in src_addr.chunks(2) {
            sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
        }
        
        // Dest address
        for chunk in dst_addr.chunks(2) {
            sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
        }
        
        // Length (ICMPv6 header + payload)
        let length = (Self::SIZE + payload.len()) as u32;
        sum += (length >> 16) as u32;
        sum += (length & 0xFFFF) as u32;
        
        // Next Header (58 = ICMPv6)
        sum += 58u32;
        
        // ICMPv6 header
        sum += self.msg_type as u32;
        sum += self.code as u32;
        
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
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        
        self.checksum = !(sum as u16);
    }
}

/// ICMPv6 Message
#[derive(Debug, Clone)]
pub struct Icmpv6Message {
    pub header: Icmpv6Header,
    pub payload: Vec<u8>,
}

impl Icmpv6Message {
    pub fn new(msg_type: Icmpv6Type, code: u8, payload: Vec<u8>) -> Self {
        Self {
            header: Icmpv6Header::new(msg_type, code),
            payload,
        }
    }
    
    /// Echo Request (ping)
    pub fn echo_request(id: u16, seq: u16, data: Vec<u8>) -> Self {
        let mut payload = Vec::with_capacity(4 + data.len());
        payload.extend_from_slice(&id.to_be_bytes());
        payload.extend_from_slice(&seq.to_be_bytes());
        payload.extend_from_slice(&data);
        
        Self::new(Icmpv6Type::EchoRequest, 0, payload)
    }
    
    /// Echo Reply (pong)
    pub fn echo_reply(id: u16, seq: u16, data: Vec<u8>) -> Self {
        let mut payload = Vec::with_capacity(4 + data.len());
        payload.extend_from_slice(&id.to_be_bytes());
        payload.extend_from_slice(&seq.to_be_bytes());
        payload.extend_from_slice(&data);
        
        Self::new(Icmpv6Type::EchoReply, 0, payload)
    }
    
    /// Neighbor Solicitation (NDP)
    pub fn neighbor_solicitation(target_addr: [u8; 16]) -> Self {
        let mut payload = Vec::with_capacity(20);
        payload.extend_from_slice(&[0u8; 4]); // Reserved
        payload.extend_from_slice(&target_addr);
        
        Self::new(Icmpv6Type::NeighborSolicitation, 0, payload)
    }
    
    /// Neighbor Advertisement (NDP)
    pub fn neighbor_advertisement(
        target_addr: [u8; 16],
        router: bool,
        solicited: bool,
        override_flag: bool,
    ) -> Self {
        let mut flags = 0u8;
        if router { flags |= 0x80; }
        if solicited { flags |= 0x40; }
        if override_flag { flags |= 0x20; }
        
        let mut payload = Vec::with_capacity(20);
        payload.push(flags);
        payload.extend_from_slice(&[0u8; 3]); // Reserved
        payload.extend_from_slice(&target_addr);
        
        Self::new(Icmpv6Type::NeighborAdvertisement, 0, payload)
    }
    
    /// Router Solicitation (NDP)
    pub fn router_solicitation() -> Self {
        let payload = vec![0u8; 4]; // Reserved
        Self::new(Icmpv6Type::RouterSolicitation, 0, payload)
    }
    
    /// Destination Unreachable
    pub fn destination_unreachable(code: u8, original_packet: &[u8]) -> Self {
        let mut payload = Vec::with_capacity(4 + original_packet.len());
        payload.extend_from_slice(&[0u8; 4]); // Unused
        payload.extend_from_slice(original_packet);
        
        Self::new(Icmpv6Type::DestinationUnreachable, code, payload)
    }
    
    /// Packet Too Big
    pub fn packet_too_big(mtu: u32, original_packet: &[u8]) -> Self {
        let mut payload = Vec::with_capacity(4 + original_packet.len());
        payload.extend_from_slice(&mtu.to_be_bytes());
        payload.extend_from_slice(original_packet);
        
        Self::new(Icmpv6Type::PacketTooBig, 0, payload)
    }
    
    /// Time Exceeded
    pub fn time_exceeded(code: u8, original_packet: &[u8]) -> Self {
        let mut payload = Vec::with_capacity(4 + original_packet.len());
        payload.extend_from_slice(&[0u8; 4]); // Unused
        payload.extend_from_slice(original_packet);
        
        Self::new(Icmpv6Type::TimeExceeded, code, payload)
    }
    
    /// Encode to bytes
    pub fn encode(&mut self, src_addr: &[u8; 16], dst_addr: &[u8; 16]) -> Vec<u8> {
        // Calculate checksum
        self.header.calculate_checksum(src_addr, dst_addr, &self.payload);
        
        let mut bytes = Vec::with_capacity(Icmpv6Header::SIZE + self.payload.len());
        bytes.push(self.header.msg_type);
        bytes.push(self.header.code);
        bytes.extend_from_slice(&self.header.checksum.to_be_bytes());
        bytes.extend_from_slice(&self.payload);
        
        bytes
    }
    
    /// Parse from bytes
    pub fn parse(data: &[u8]) -> Result<Self, Icmpv6Error> {
        if data.len() < Icmpv6Header::SIZE {
            return Err(Icmpv6Error::TooShort);
        }
        
        let header = Icmpv6Header {
            msg_type: data[0],
            code: data[1],
            checksum: u16::from_be_bytes([data[2], data[3]]),
        };
        
        let payload = data[Icmpv6Header::SIZE..].to_vec();
        
        Ok(Self { header, payload })
    }
}

/// ICMPv6 Errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icmpv6Error {
    TooShort,
    InvalidChecksum,
    UnknownType,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_echo_request() {
        let msg = Icmpv6Message::echo_request(1234, 1, vec![0x42; 32]);
        assert_eq!(msg.header.msg_type, 128);
        assert_eq!(msg.payload.len(), 4 + 32);
    }
    
    #[test]
    fn test_neighbor_solicitation() {
        let target = [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        let msg = Icmpv6Message::neighbor_solicitation(target);
        assert_eq!(msg.header.msg_type, 135);
        assert_eq!(msg.payload.len(), 20);
    }
    
    #[test]
    fn test_checksum() {
        let src = [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        let dst = [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2];
        
        let mut msg = Icmpv6Message::echo_request(1, 1, vec![0; 8]);
        msg.header.calculate_checksum(&src, &dst, &msg.payload);
        
        assert_ne!(msg.header.checksum, 0);
    }
}
