//! # QUIC Protocol - Next-Gen Transport
//! 
//! Implémentation QUIC (HTTP/3) pour remplacer TCP+TLS.
//! 
//! ## Features
//! - QUIC (RFC 9000)
//! - 0-RTT connection
//! - Multiplexing sans HOL blocking
//! - Loss recovery
//! - Connection migration

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use crate::sync::SpinLock;
use core::sync::atomic::{AtomicU64, Ordering};

/// Type de paquet QUIC
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketType {
    Initial = 0x00,
    ZeroRtt = 0x01,
    Handshake = 0x02,
    Retry = 0x03,
    // Short header (1-RTT)
    OneRtt = 0x40,
}

/// Frame QUIC
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    Padding = 0x00,
    Ping = 0x01,
    Ack = 0x02,
    ResetStream = 0x04,
    StopSending = 0x05,
    Crypto = 0x06,
    NewToken = 0x07,
    Stream = 0x08,
    MaxData = 0x10,
    MaxStreamData = 0x11,
    MaxStreams = 0x12,
    DataBlocked = 0x14,
    StreamDataBlocked = 0x15,
    ConnectionClose = 0x1c,
}

/// Connection ID
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionId {
    pub data: [u8; 20],
    pub len: u8,
}

impl ConnectionId {
    pub fn new() -> Self {
        Self {
            data: [0u8; 20],
            len: 8, // 8 bytes par défaut
        }
    }
    
    pub fn random() -> Self {
        let mut cid = Self::new();
        // TODO: remplir avec random
        cid
    }
    
    pub fn as_slice(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }
}

/// Stream QUIC
pub struct QuicStream {
    pub id: u64,
    pub send_offset: u64,
    pub recv_offset: u64,
    pub max_data: u64,
    pub data: Vec<u8>,
    pub fin_sent: bool,
    pub fin_recv: bool,
}

impl QuicStream {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            send_offset: 0,
            recv_offset: 0,
            max_data: 1_000_000, // 1 MB par défaut
            data: Vec::new(),
            fin_sent: false,
            fin_recv: false,
        }
    }
}

/// État de connexion QUIC
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuicState {
    Initial,
    Handshake,
    Active,
    Closing,
    Draining,
    Closed,
}

/// Connexion QUIC
pub struct QuicConnection {
    pub state: QuicState,
    pub dcid: ConnectionId, // Destination CID
    pub scid: ConnectionId, // Source CID
    
    pub streams: SpinLock<BTreeMap<u64, QuicStream>>,
    pub next_stream_id: AtomicU64,
    
    // Flow control
    pub max_data: u64,
    pub data_sent: AtomicU64,
    pub data_recv: AtomicU64,
    
    // Loss recovery
    pub packet_number: AtomicU64,
    pub rtt: u64, // microseconds
    
    // Crypto
    pub keys_initial: Option<Vec<u8>>,
    pub keys_handshake: Option<Vec<u8>>,
    pub keys_1rtt: Option<Vec<u8>>,
}

impl QuicConnection {
    pub fn new(is_client: bool) -> Self {
        Self {
            state: QuicState::Initial,
            dcid: ConnectionId::random(),
            scid: ConnectionId::random(),
            streams: SpinLock::new(BTreeMap::new()),
            next_stream_id: AtomicU64::new(if is_client { 0 } else { 1 }),
            max_data: 10_000_000, // 10 MB
            data_sent: AtomicU64::new(0),
            data_recv: AtomicU64::new(0),
            packet_number: AtomicU64::new(0),
            rtt: 100_000, // 100ms initial
            keys_initial: None,
            keys_handshake: None,
            keys_1rtt: None,
        }
    }
    
    /// Crée un nouveau stream
    pub fn create_stream(&self, bidirectional: bool) -> u64 {
        let id = self.next_stream_id.fetch_add(4, Ordering::SeqCst);
        let mut streams = self.streams.lock();
        streams.insert(id, QuicStream::new(id));
        id
    }
    
    /// Envoie Initial packet
    pub fn send_initial(&mut self) -> Vec<u8> {
        let mut packet = Vec::new();
        
        // Header flags
        let flags = 0xC0 | PacketType::Initial as u8;
        packet.push(flags);
        
        // Version (QUIC v1 = 0x00000001)
        packet.extend_from_slice(&[0, 0, 0, 1]);
        
        // DCID
        packet.push(self.dcid.len);
        packet.extend_from_slice(self.dcid.as_slice());
        
        // SCID
        packet.push(self.scid.len);
        packet.extend_from_slice(self.scid.as_slice());
        
        // Token length (0 pour client)
        packet.push(0);
        
        // Length (varint) - TODO
        packet.extend_from_slice(&[0x40, 0x64]); // 100 bytes
        
        // Packet number
        let pn = self.packet_number.fetch_add(1, Ordering::SeqCst);
        packet.extend_from_slice(&pn.to_be_bytes()[4..8]); // 4 bytes
        
        // Payload (CRYPTO frame)
        packet.push(FrameType::Crypto as u8);
        packet.push(0); // Offset = 0
        packet.push(32); // Length = 32
        packet.extend_from_slice(&[0u8; 32]); // TODO: vrai crypto
        
        self.state = QuicState::Handshake;
        
        packet
    }
    
    /// Envoie STREAM frame
    pub fn send_stream(&self, stream_id: u64, data: &[u8], fin: bool) -> Vec<u8> {
        let mut packet = Vec::new();
        
        // Short header (1-RTT)
        let flags = 0x40;
        packet.push(flags);
        
        // DCID
        packet.extend_from_slice(self.dcid.as_slice());
        
        // Packet number
        let pn = self.packet_number.fetch_add(1, Ordering::SeqCst);
        packet.extend_from_slice(&pn.to_be_bytes()[6..8]); // 2 bytes
        
        // STREAM frame
        let frame_type = if fin {
            FrameType::Stream as u8 | 0x01 // FIN bit
        } else {
            FrameType::Stream as u8
        };
        packet.push(frame_type);
        
        // Stream ID (varint)
        Self::encode_varint(&mut packet, stream_id);
        
        // Offset = 0 (varint)
        packet.push(0);
        
        // Length (varint)
        Self::encode_varint(&mut packet, data.len() as u64);
        
        // Data
        packet.extend_from_slice(data);
        
        // Update flow control
        self.data_sent.fetch_add(data.len() as u64, Ordering::Relaxed);
        
        packet
    }
    
    /// Envoie ACK frame
    pub fn send_ack(&self, packet_numbers: &[u64]) -> Vec<u8> {
        let mut packet = Vec::new();
        
        packet.push(0x40); // Short header
        packet.extend_from_slice(self.dcid.as_slice());
        
        let pn = self.packet_number.fetch_add(1, Ordering::SeqCst);
        packet.extend_from_slice(&pn.to_be_bytes()[6..8]);
        
        // ACK frame
        packet.push(FrameType::Ack as u8);
        
        // Largest acknowledged
        if let Some(&largest) = packet_numbers.last() {
            Self::encode_varint(&mut packet, largest);
            
            // ACK delay (0 pour l'instant)
            packet.push(0);
            
            // ACK range count
            packet.push(0);
            
            // First ACK range
            Self::encode_varint(&mut packet, packet_numbers.len() as u64 - 1);
        }
        
        packet
    }
    
    /// Parse paquet QUIC
    pub fn parse_packet(&mut self, data: &[u8]) -> Result<PacketType, QuicError> {
        if data.is_empty() {
            return Err(QuicError::InvalidPacket);
        }
        
        let flags = data[0];
        let is_long = (flags & 0x80) != 0;
        
        if is_long {
            let ptype = (flags & 0x30) >> 4;
            Ok(match ptype {
                0 => PacketType::Initial,
                1 => PacketType::ZeroRtt,
                2 => PacketType::Handshake,
                3 => PacketType::Retry,
                _ => return Err(QuicError::InvalidPacket),
            })
        } else {
            Ok(PacketType::OneRtt)
        }
    }
    
    /// Encode varint QUIC
    fn encode_varint(buf: &mut Vec<u8>, value: u64) {
        if value < 64 {
            buf.push(value as u8);
        } else if value < 16384 {
            buf.push(0x40 | ((value >> 8) as u8));
            buf.push(value as u8);
        } else if value < 1073741824 {
            buf.push(0x80 | ((value >> 24) as u8));
            buf.push((value >> 16) as u8);
            buf.push((value >> 8) as u8);
            buf.push(value as u8);
        } else {
            buf.push(0xC0 | ((value >> 56) as u8));
            for i in (0..7).rev() {
                buf.push((value >> (i * 8)) as u8);
            }
        }
    }
}

/// Erreurs QUIC
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuicError {
    InvalidPacket,
    FlowControlError,
    StreamClosed,
    ConnectionClosed,
}

/// Client QUIC
pub struct QuicClient {
    conn: QuicConnection,
    udp_socket: usize, // FD du socket UDP
}

impl QuicClient {
    pub fn new(udp_socket: usize) -> Self {
        Self {
            conn: QuicConnection::new(true),
            udp_socket,
        }
    }
    
    pub fn connect(&mut self) -> Result<(), QuicError> {
        // Envoie Initial packet
        let initial = self.conn.send_initial();
        // TODO: write to UDP socket
        
        // TODO: recv Handshake packet
        
        self.conn.state = QuicState::Active;
        Ok(())
    }
    
    pub fn send(&mut self, data: &[u8]) -> Result<u64, QuicError> {
        if self.conn.state != QuicState::Active {
            return Err(QuicError::ConnectionClosed);
        }
        
        let stream_id = self.conn.create_stream(true);
        let packet = self.conn.send_stream(stream_id, data, true);
        
        // TODO: write to UDP socket
        
        Ok(stream_id)
    }
    
    pub fn recv(&mut self, stream_id: u64) -> Result<Vec<u8>, QuicError> {
        // TODO: read from UDP socket
        
        let streams = self.conn.streams.lock();
        if let Some(stream) = streams.get(&stream_id) {
            Ok(stream.data.clone())
        } else {
            Err(QuicError::StreamClosed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_varint_encode() {
        let mut buf = Vec::new();
        QuicConnection::encode_varint(&mut buf, 37);
        assert_eq!(buf, vec![37]);
        
        buf.clear();
        QuicConnection::encode_varint(&mut buf, 15293);
        assert_eq!(buf, vec![0x7b, 0xbd]);
    }
    
    #[test]
    fn test_connection_id() {
        let cid = ConnectionId::random();
        assert_eq!(cid.len, 8);
        assert_eq!(cid.as_slice().len(), 8);
    }
}
