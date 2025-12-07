//! # ICMP - Internet Control Message Protocol (IPv4)
//! 
//! RFC 792 - ICMP specification
//! 
//! High-performance ICMP with:
//! - Echo Request/Reply (ping)
//! - Destination Unreachable
//! - Time Exceeded
//! - Redirect
//! - Source Quench

use alloc::vec::Vec;
use crate::net::ip::Ipv4Address;

/// ICMP Message Types (RFC 792)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IcmpType {
    EchoReply = 0,
    DestinationUnreachable = 3,
    SourceQuench = 4,
    Redirect = 5,
    EchoRequest = 8,
    RouterAdvertisement = 9,
    RouterSolicitation = 10,
    TimeExceeded = 11,
    ParameterProblem = 12,
    Timestamp = 13,
    TimestampReply = 14,
    Unknown(u8),
}

impl From<u8> for IcmpType {
    fn from(value: u8) -> Self {
        match value {
            0 => IcmpType::EchoReply,
            3 => IcmpType::DestinationUnreachable,
            4 => IcmpType::SourceQuench,
            5 => IcmpType::Redirect,
            8 => IcmpType::EchoRequest,
            9 => IcmpType::RouterAdvertisement,
            10 => IcmpType::RouterSolicitation,
            11 => IcmpType::TimeExceeded,
            12 => IcmpType::ParameterProblem,
            13 => IcmpType::Timestamp,
            14 => IcmpType::TimestampReply,
            _ => IcmpType::Unknown(value),
        }
    }
}

impl From<IcmpType> for u8 {
    fn from(t: IcmpType) -> Self {
        match t {
            IcmpType::EchoReply => 0,
            IcmpType::DestinationUnreachable => 3,
            IcmpType::SourceQuench => 4,
            IcmpType::Redirect => 5,
            IcmpType::EchoRequest => 8,
            IcmpType::RouterAdvertisement => 9,
            IcmpType::RouterSolicitation => 10,
            IcmpType::TimeExceeded => 11,
            IcmpType::ParameterProblem => 12,
            IcmpType::Timestamp => 13,
            IcmpType::TimestampReply => 14,
            IcmpType::Unknown(v) => v,
        }
    }
}

/// ICMP Destination Unreachable Codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DestUnreachableCode {
    NetworkUnreachable = 0,
    HostUnreachable = 1,
    ProtocolUnreachable = 2,
    PortUnreachable = 3,
    FragmentationNeeded = 4,
    SourceRouteFailed = 5,
    NetworkUnknown = 6,
    HostUnknown = 7,
    HostIsolated = 8,
    NetworkProhibited = 9,
    HostProhibited = 10,
    NetworkUnreachableForTos = 11,
    HostUnreachableForTos = 12,
    CommunicationProhibited = 13,
    HostPrecedenceViolation = 14,
    PrecedenceCutoff = 15,
}

/// ICMP Time Exceeded Codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TimeExceededCode {
    TtlExpired = 0,
    FragmentReassemblyTimeExceeded = 1,
}

/// ICMP Redirect Codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RedirectCode {
    Network = 0,
    Host = 1,
    TosNetwork = 2,
    TosHost = 3,
}

/// ICMP Header (RFC 792)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IcmpHeader {
    pub msg_type: u8,
    pub code: u8,
    pub checksum: u16,
    pub rest_of_header: u32, // Varies by type
}

impl IcmpHeader {
    pub fn new(msg_type: IcmpType, code: u8) -> Self {
        Self {
            msg_type: msg_type.into(),
            code,
            checksum: 0,
            rest_of_header: 0,
        }
    }
    
    /// Calculate ICMP checksum
    pub fn calculate_checksum(&mut self, payload: &[u8]) {
        self.checksum = 0;
        
        let mut sum: u32 = 0;
        
        // Header
        let header_bytes = unsafe {
            core::slice::from_raw_parts(
                self as *const _ as *const u8,
                core::mem::size_of::<IcmpHeader>()
            )
        };
        
        for chunk in header_bytes.chunks(2) {
            let word = if chunk.len() == 2 {
                u16::from_be_bytes([chunk[0], chunk[1]]) as u32
            } else {
                (chunk[0] as u32) << 8
            };
            sum += word;
        }
        
        // Payload
        for chunk in payload.chunks(2) {
            let word = if chunk.len() == 2 {
                u16::from_be_bytes([chunk[0], chunk[1]]) as u32
            } else {
                (chunk[0] as u32) << 8
            };
            sum += word;
        }
        
        // Fold 32-bit sum to 16 bits
        while sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        
        self.checksum = (!sum as u16).to_be();
    }
    
    /// Verify checksum
    pub fn verify_checksum(&self, payload: &[u8]) -> bool {
        let mut sum: u32 = 0;
        
        // Header
        let header_bytes = unsafe {
            core::slice::from_raw_parts(
                self as *const _ as *const u8,
                core::mem::size_of::<IcmpHeader>()
            )
        };
        
        for chunk in header_bytes.chunks(2) {
            let word = if chunk.len() == 2 {
                u16::from_be_bytes([chunk[0], chunk[1]]) as u32
            } else {
                (chunk[0] as u32) << 8
            };
            sum += word;
        }
        
        // Payload
        for chunk in payload.chunks(2) {
            let word = if chunk.len() == 2 {
                u16::from_be_bytes([chunk[0], chunk[1]]) as u32
            } else {
                (chunk[0] as u32) << 8
            };
            sum += word;
        }
        
        // Fold and check
        while sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        
        sum == 0xFFFF
    }
}

/// ICMP Echo Request/Reply
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IcmpEcho {
    pub identifier: u16,
    pub sequence: u16,
}

/// ICMP Message
pub struct IcmpMessage {
    pub header: IcmpHeader,
    pub payload: Vec<u8>,
}

impl IcmpMessage {
    pub fn new(msg_type: IcmpType, code: u8, payload: Vec<u8>) -> Self {
        Self {
            header: IcmpHeader::new(msg_type, code),
            payload,
        }
    }
    
    /// Create Echo Request (ping)
    pub fn echo_request(identifier: u16, sequence: u16, payload: Vec<u8>) -> Self {
        let mut msg = Self::new(IcmpType::EchoRequest, 0, payload);
        msg.header.rest_of_header = ((identifier as u32) << 16) | (sequence as u32);
        msg
    }
    
    /// Create Echo Reply (pong)
    pub fn echo_reply(identifier: u16, sequence: u16, payload: Vec<u8>) -> Self {
        let mut msg = Self::new(IcmpType::EchoReply, 0, payload);
        msg.header.rest_of_header = ((identifier as u32) << 16) | (sequence as u32);
        msg
    }
    
    /// Create Destination Unreachable
    pub fn dest_unreachable(code: DestUnreachableCode, original_packet: &[u8]) -> Self {
        // Include IP header + first 8 bytes of original datagram
        let payload = original_packet[..original_packet.len().min(28)].to_vec();
        Self::new(IcmpType::DestinationUnreachable, code as u8, payload)
    }
    
    /// Create Time Exceeded
    pub fn time_exceeded(code: TimeExceededCode, original_packet: &[u8]) -> Self {
        let payload = original_packet[..original_packet.len().min(28)].to_vec();
        Self::new(IcmpType::TimeExceeded, code as u8, payload)
    }
    
    /// Create Redirect
    pub fn redirect(code: RedirectCode, gateway: Ipv4Address, original_packet: &[u8]) -> Self {
        let mut msg = Self::new(IcmpType::Redirect, code as u8, Vec::new());
        // Gateway address in rest_of_header
        msg.header.rest_of_header = u32::from_be_bytes(gateway.0);
        // Original packet in payload
        msg.payload = original_packet[..original_packet.len().min(28)].to_vec();
        msg
    }
    
    /// Serialize to bytes
    pub fn to_bytes(&mut self) -> Vec<u8> {
        self.header.calculate_checksum(&self.payload);
        
        let mut bytes = Vec::with_capacity(8 + self.payload.len());
        
        // Header
        bytes.push(self.header.msg_type);
        bytes.push(self.header.code);
        bytes.extend_from_slice(&self.header.checksum.to_be_bytes());
        bytes.extend_from_slice(&self.header.rest_of_header.to_be_bytes());
        
        // Payload
        bytes.extend_from_slice(&self.payload);
        
        bytes
    }
    
    /// Parse from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self, IcmpError> {
        if data.len() < 8 {
            return Err(IcmpError::TooShort);
        }
        
        let header = IcmpHeader {
            msg_type: data[0],
            code: data[1],
            checksum: u16::from_be_bytes([data[2], data[3]]),
            rest_of_header: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
        };
        
        let payload = data[8..].to_vec();
        
        // Verify checksum
        if !header.verify_checksum(&payload) {
            return Err(IcmpError::BadChecksum);
        }
        
        Ok(Self { header, payload })
    }
}

/// ICMP Errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IcmpError {
    TooShort,
    BadChecksum,
    UnknownType,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_echo_request() {
        let payload = b"Hello, World!".to_vec();
        let mut msg = IcmpMessage::echo_request(1234, 5678, payload.clone());
        
        let bytes = msg.to_bytes();
        assert!(bytes.len() > 8);
        assert_eq!(bytes[0], IcmpType::EchoRequest as u8);
        
        // Parse back
        let parsed = IcmpMessage::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.header.msg_type, IcmpType::EchoRequest as u8);
        assert_eq!(parsed.payload, payload);
    }
    
    #[test]
    fn test_dest_unreachable() {
        let original = vec![0x45, 0x00, 0x00, 0x3c]; // IP header start
        let msg = IcmpMessage::dest_unreachable(
            DestUnreachableCode::PortUnreachable,
            &original
        );
        
        assert_eq!(msg.header.msg_type, IcmpType::DestinationUnreachable as u8);
        assert_eq!(msg.header.code, DestUnreachableCode::PortUnreachable as u8);
    }
}
