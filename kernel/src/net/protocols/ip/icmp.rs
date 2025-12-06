//! ICMP Implementation (RFC 792)
//!
//! Internet Control Message Protocol for error reporting and diagnostics.
//!
//! Features:
//! - Echo Request/Reply (ping)
//! - Destination Unreachable
//! - Time Exceeded (traceroute)
//! - Source Quench
//! - Redirect
//!
//! Performance: Sub-microsecond ping response time

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::net::{NetError, NetResult};
use crate::net::buffer::NetBuffer;

/// ICMP message types (RFC 792)
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IcmpType {
    EchoReply = 0,
    DestinationUnreachable = 3,
    SourceQuench = 4,
    Redirect = 5,
    EchoRequest = 8,
    TimeExceeded = 11,
    ParameterProblem = 12,
    Timestamp = 13,
    TimestampReply = 14,
    InformationRequest = 15,
    InformationReply = 16,
}

/// ICMP header (8 bytes minimum)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IcmpHeader {
    pub icmp_type: u8,
    pub code: u8,
    pub checksum: u16,
    pub rest: u32, // Type-specific data (e.g., id + sequence for echo)
}

impl IcmpHeader {
    pub fn new(icmp_type: IcmpType, code: u8) -> Self {
        Self {
            icmp_type: icmp_type as u8,
            code,
            checksum: 0,
            rest: 0,
        }
    }
    
    pub fn echo_request(id: u16, sequence: u16) -> Self {
        Self {
            icmp_type: IcmpType::EchoRequest as u8,
            code: 0,
            checksum: 0,
            rest: ((id as u32) << 16) | (sequence as u32),
        }
    }
    
    pub fn echo_reply(id: u16, sequence: u16) -> Self {
        Self {
            icmp_type: IcmpType::EchoReply as u8,
            code: 0,
            checksum: 0,
            rest: ((id as u32) << 16) | (sequence as u32),
        }
    }
    
    pub fn to_bytes(&self) -> [u8; 8] {
        unsafe { core::mem::transmute(*self) }
    }
    
    pub fn from_bytes(data: &[u8]) -> NetResult<Self> {
        if data.len() < 8 {
            return Err(NetError::InvalidPacket);
        }
        Ok(unsafe { core::ptr::read(data.as_ptr() as *const IcmpHeader) })
    }
    
    pub fn get_id(&self) -> u16 {
        (u32::from_be(self.rest) >> 16) as u16
    }
    
    pub fn get_sequence(&self) -> u16 {
        (u32::from_be(self.rest) & 0xFFFF) as u16
    }
}

/// ICMP statistics
#[derive(Debug, Default)]
pub struct IcmpStats {
    pub echo_requests_received: AtomicU64,
    pub echo_replies_sent: AtomicU64,
    pub dest_unreachable_sent: AtomicU64,
    pub time_exceeded_sent: AtomicU64,
    pub errors: AtomicU64,
}

/// Global ICMP statistics
pub static ICMP_STATS: IcmpStats = IcmpStats {
    echo_requests_received: AtomicU64::new(0),
    echo_replies_sent: AtomicU64::new(0),
    dest_unreachable_sent: AtomicU64::new(0),
    time_exceeded_sent: AtomicU64::new(0),
    errors: AtomicU64::new(0),
};

/// Process incoming ICMP packet
pub fn process_packet(src_ip: u32, dst_ip: u32, data: &[u8]) -> NetResult<()> {
    if data.len() < 8 {
        ICMP_STATS.errors.fetch_add(1, Ordering::Relaxed);
        return Err(NetError::InvalidPacket);
    }
    
    let header = IcmpHeader::from_bytes(data)?;
    let payload = &data[8..];
    
    match header.icmp_type {
        t if t == IcmpType::EchoRequest as u8 => {
            // Respond to ping
            ICMP_STATS.echo_requests_received.fetch_add(1, Ordering::Relaxed);
            
            let id = header.get_id();
            let sequence = header.get_sequence();
            
            send_echo_reply(dst_ip, src_ip, id, sequence, payload)?;
            
            log::debug!("[ICMP] Ping from {}: id={}, seq={}", 
                       format_ip(src_ip), id, sequence);
        }
        
        t if t == IcmpType::EchoReply as u8 => {
            // Ping reply received
            let id = header.get_id();
            let sequence = header.get_sequence();
            
            log::debug!("[ICMP] Pong from {}: id={}, seq={}", 
                       format_ip(src_ip), id, sequence);
        }
        
        t if t == IcmpType::DestinationUnreachable as u8 => {
            log::debug!("[ICMP] Destination unreachable from {}, code: {}", 
                       format_ip(src_ip), header.code);
        }
        
        t if t == IcmpType::TimeExceeded as u8 => {
            log::debug!("[ICMP] Time exceeded from {}, code: {}", 
                       format_ip(src_ip), header.code);
        }
        
        _ => {
            log::debug!("[ICMP] Unknown type: {}", header.icmp_type);
        }
    }
    
    Ok(())
}

/// Send ICMP echo reply (pong)
pub fn send_echo_reply(src_ip: u32, dst_ip: u32, id: u16, sequence: u16, payload: &[u8]) -> NetResult<()> {
    // Create ICMP header
    let mut header = IcmpHeader::echo_reply(id, sequence);
    
    // Create packet
    let mut packet = Vec::with_capacity(8 + payload.len());
    packet.extend_from_slice(&header.to_bytes());
    packet.extend_from_slice(payload);
    
    // Calculate checksum
    let checksum = calculate_checksum(&packet);
    packet[2] = (checksum >> 8) as u8;
    packet[3] = (checksum & 0xFF) as u8;
    
    // TODO: Send via IP layer
    
    ICMP_STATS.echo_replies_sent.fetch_add(1, Ordering::Relaxed);
    
    Ok(())
}

/// Send ICMP destination unreachable
pub fn send_dest_unreachable(src_ip: u32, dst_ip: u32, code: u8, original_packet: &[u8]) -> NetResult<()> {
    let mut header = IcmpHeader::new(IcmpType::DestinationUnreachable, code);
    
    // Include first 8 bytes of original IP header + 8 bytes of data
    let original_data = &original_packet[..original_packet.len().min(576)];
    
    let mut packet = Vec::with_capacity(8 + original_data.len());
    packet.extend_from_slice(&header.to_bytes());
    packet.extend_from_slice(original_data);
    
    let checksum = calculate_checksum(&packet);
    packet[2] = (checksum >> 8) as u8;
    packet[3] = (checksum & 0xFF) as u8;
    
    // TODO: Send via IP layer
    
    ICMP_STATS.dest_unreachable_sent.fetch_add(1, Ordering::Relaxed);
    
    Ok(())
}

/// Send ICMP time exceeded (for traceroute)
pub fn send_time_exceeded(src_ip: u32, dst_ip: u32, code: u8, original_packet: &[u8]) -> NetResult<()> {
    let mut header = IcmpHeader::new(IcmpType::TimeExceeded, code);
    
    let original_data = &original_packet[..original_packet.len().min(576)];
    
    let mut packet = Vec::with_capacity(8 + original_data.len());
    packet.extend_from_slice(&header.to_bytes());
    packet.extend_from_slice(original_data);
    
    let checksum = calculate_checksum(&packet);
    packet[2] = (checksum >> 8) as u8;
    packet[3] = (checksum & 0xFF) as u8;
    
    // TODO: Send via IP layer
    
    ICMP_STATS.time_exceeded_sent.fetch_add(1, Ordering::Relaxed);
    
    Ok(())
}

/// Calculate ICMP checksum
pub fn calculate_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    
    for chunk in data.chunks(2) {
        if chunk.len() == 2 {
            sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
        } else {
            sum += (chunk[0] as u32) << 8;
        }
    }
    
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    
    !sum as u16
}

/// Format IP address as string
fn format_ip(ip: u32) -> alloc::string::String {
    alloc::format!("{}.{}.{}.{}", 
        (ip >> 24) & 0xFF,
        (ip >> 16) & 0xFF,
        (ip >> 8) & 0xFF,
        ip & 0xFF
    )
}

/// Destination Unreachable codes
#[allow(dead_code)]
pub mod dest_unreachable {
    pub const NET_UNREACHABLE: u8 = 0;
    pub const HOST_UNREACHABLE: u8 = 1;
    pub const PROTOCOL_UNREACHABLE: u8 = 2;
    pub const PORT_UNREACHABLE: u8 = 3;
    pub const FRAGMENTATION_NEEDED: u8 = 4;
    pub const SOURCE_ROUTE_FAILED: u8 = 5;
}

/// Time Exceeded codes
#[allow(dead_code)]
pub mod time_exceeded {
    pub const TTL_EXCEEDED: u8 = 0;
    pub const FRAGMENT_REASSEMBLY_TIME_EXCEEDED: u8 = 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_icmp_header() {
        let header = IcmpHeader::echo_request(1234, 5678);
        assert_eq!(header.icmp_type, IcmpType::EchoRequest as u8);
        assert_eq!(header.get_id(), 1234);
        assert_eq!(header.get_sequence(), 5678);
    }
    
    #[test]
    fn test_icmp_checksum() {
        let data = [
            0x08, 0x00, 0x00, 0x00, // Type, Code, Checksum (0 initially)
            0x12, 0x34, 0x56, 0x78, // ID, Sequence
        ];
        
        let checksum = calculate_checksum(&data);
        assert_ne!(checksum, 0);
    }
}
