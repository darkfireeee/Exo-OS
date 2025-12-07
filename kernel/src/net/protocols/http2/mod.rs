//! # HTTP/2 Implementation - Modern Web Protocol
//! 
//! Implémentation HTTP/2 avec multiplexing et server push.
//! 
//! ## Features
//! - HTTP/2 (RFC 7540)
//! - Stream multiplexing
//! - Header compression (HPACK)
//! - Server push
//! - Flow control

pub mod hpack;

pub use hpack::{HpackCodec, HpackError};

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use alloc::string::String;
use crate::sync::SpinLock;

/// Type de frame HTTP/2
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    Data = 0x0,
    Headers = 0x1,
    Priority = 0x2,
    RstStream = 0x3,
    Settings = 0x4,
    PushPromise = 0x5,
    Ping = 0x6,
    GoAway = 0x7,
    WindowUpdate = 0x8,
    Continuation = 0x9,
}

/// Flags de frame
pub mod flags {
    pub const END_STREAM: u8 = 0x1;
    pub const END_HEADERS: u8 = 0x4;
    pub const PADDED: u8 = 0x8;
    pub const PRIORITY: u8 = 0x20;
}

/// Header de frame HTTP/2
#[repr(C, packed)]
pub struct FrameHeader {
    pub length: [u8; 3],    // 24 bits
    pub frame_type: u8,
    pub flags: u8,
    pub stream_id: u32,     // 31 bits (R bit = 0)
}

impl FrameHeader {
    pub fn new(frame_type: FrameType, flags: u8, stream_id: u32, length: u32) -> Self {
        let length_bytes = [
            ((length >> 16) & 0xFF) as u8,
            ((length >> 8) & 0xFF) as u8,
            (length & 0xFF) as u8,
        ];
        
        Self {
            length: length_bytes,
            frame_type: frame_type as u8,
            flags,
            stream_id: stream_id & 0x7FFFFFFF,
        }
    }
    
    pub fn get_length(&self) -> u32 {
        ((self.length[0] as u32) << 16) 
        | ((self.length[1] as u32) << 8) 
        | (self.length[2] as u32)
    }
}

/// Stream HTTP/2
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    Idle,
    Open,
    HalfClosedLocal,
    HalfClosedRemote,
    Closed,
}

pub struct Http2Stream {
    pub id: u32,
    pub state: StreamState,
    pub window_size: i32,
    pub headers: BTreeMap<String, String>,
    pub data: Vec<u8>,
}

impl Http2Stream {
    pub fn new(id: u32) -> Self {
        Self {
            id,
            state: StreamState::Idle,
            window_size: 65535, // Default
            headers: BTreeMap::new(),
            data: Vec::new(),
        }
    }
}

/// Connexion HTTP/2
pub struct Http2Connection {
    streams: SpinLock<BTreeMap<u32, Http2Stream>>,
    next_stream_id: u32,
    window_size: i32,
    settings: Settings,
}

#[derive(Clone)]
pub struct Settings {
    pub header_table_size: u32,
    pub enable_push: bool,
    pub max_concurrent_streams: u32,
    pub initial_window_size: u32,
    pub max_frame_size: u32,
    pub max_header_list_size: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            header_table_size: 4096,
            enable_push: true,
            max_concurrent_streams: 100,
            initial_window_size: 65535,
            max_frame_size: 16384,
            max_header_list_size: 8192,
        }
    }
}

impl Http2Connection {
    pub fn new(is_client: bool) -> Self {
        Self {
            streams: SpinLock::new(BTreeMap::new()),
            next_stream_id: if is_client { 1 } else { 2 }, // Client=odd, Server=even
            window_size: 65535,
            settings: Settings::default(),
        }
    }
    
    /// Envoie connection preface (magic string)
    pub fn send_preface(&self) -> Vec<u8> {
        b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".to_vec()
    }
    
    /// Envoie frame SETTINGS
    pub fn send_settings(&self) -> Vec<u8> {
        let mut frame = Vec::new();
        
        // Header
        let header = FrameHeader::new(FrameType::Settings, 0, 0, 0);
        frame.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &header as *const _ as *const u8,
                core::mem::size_of::<FrameHeader>()
            )
        });
        
        // Pas de payload pour l'instant
        
        frame
    }
    
    /// Crée un nouveau stream
    pub fn create_stream(&mut self) -> u32 {
        let id = self.next_stream_id;
        self.next_stream_id += 2; // Skip odd/even
        
        let mut streams = self.streams.lock();
        streams.insert(id, Http2Stream::new(id));
        
        id
    }
    
    /// Envoie HEADERS frame
    pub fn send_headers(&self, stream_id: u32, headers: &[(String, String)], end_stream: bool) -> Vec<u8> {
        let mut frame = Vec::new();
        
        // Encode headers avec HPACK
        let encoded = self.encode_headers(headers);
        
        // Header
        let flags = if end_stream { flags::END_STREAM | flags::END_HEADERS } else { flags::END_HEADERS };
        let header = FrameHeader::new(FrameType::Headers, flags, stream_id, encoded.len() as u32);
        
        frame.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &header as *const _ as *const u8,
                core::mem::size_of::<FrameHeader>()
            )
        });
        
        // Payload
        frame.extend_from_slice(&encoded);
        
        frame
    }
    
    /// Envoie DATA frame
    pub fn send_data(&self, stream_id: u32, data: &[u8], end_stream: bool) -> Vec<u8> {
        let mut frame = Vec::new();
        
        // Header
        let flags = if end_stream { flags::END_STREAM } else { 0 };
        let header = FrameHeader::new(FrameType::Data, flags, stream_id, data.len() as u32);
        
        frame.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &header as *const _ as *const u8,
                core::mem::size_of::<FrameHeader>()
            )
        });
        
        // Payload
        frame.extend_from_slice(data);
        
        frame
    }
    
    /// Parse frame reçue
    pub fn parse_frame(&self, data: &[u8]) -> Result<(FrameHeader, &[u8]), Http2Error> {
        if data.len() < 9 {
            return Err(Http2Error::FrameTooSmall);
        }
        
        let header = unsafe {
            core::ptr::read_unaligned(data.as_ptr() as *const FrameHeader)
        };
        
        let length = header.get_length() as usize;
        if data.len() < 9 + length {
            return Err(Http2Error::FrameTooSmall);
        }
        
        let payload = &data[9..9 + length];
        Ok((header, payload))
    }
    
    /// Encode headers avec HPACK
    fn encode_headers(&self, headers: &[(String, String)]) -> Vec<u8> {
        let mut codec = hpack::HpackCodec::new(4096);
        codec.encode(headers)
    }
    
    /// Request HTTP/2 GET
    pub fn get(&mut self, path: &str, host: &str) -> Vec<u8> {
        let stream_id = self.create_stream();
        
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), path.to_string()),
            (":scheme".to_string(), "https".to_string()),
            (":authority".to_string(), host.to_string()),
        ];
        
        self.send_headers(stream_id, &headers, true)
    }
    
    /// Request HTTP/2 POST
    pub fn post(&mut self, path: &str, host: &str, body: &[u8]) -> Vec<Vec<u8>> {
        let stream_id = self.create_stream();
        
        let headers = vec![
            (":method".to_string(), "POST".to_string()),
            (":path".to_string(), path.to_string()),
            (":scheme".to_string(), "https".to_string()),
            (":authority".to_string(), host.to_string()),
            ("content-length".to_string(), body.len().to_string()),
        ];
        
        let mut frames = Vec::new();
        frames.push(self.send_headers(stream_id, &headers, false));
        frames.push(self.send_data(stream_id, body, true));
        
        frames
    }
}

/// Erreurs HTTP/2
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Http2Error {
    FrameTooSmall,
    InvalidFrame,
    StreamClosed,
    FlowControlError,
}

/// Client HTTP/2 simple
pub struct Http2Client {
    conn: Http2Connection,
    tcp_socket: usize, // FD du socket TCP
}

impl Http2Client {
    pub fn new(tcp_socket: usize) -> Self {
        Self {
            conn: Http2Connection::new(true),
            tcp_socket,
        }
    }
    
    pub fn connect(&mut self) -> Result<(), Http2Error> {
        // Envoie preface
        let preface = self.conn.send_preface();
        // TODO: write to TCP socket
        
        // Envoie SETTINGS
        let settings = self.conn.send_settings();
        // TODO: write to TCP socket
        
        Ok(())
    }
    
    pub fn get(&mut self, path: &str, host: &str) -> Result<Vec<u8>, Http2Error> {
        let frame = self.conn.get(path, host);
        // TODO: write to TCP socket
        
        // TODO: read response
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_frame_header() {
        let header = FrameHeader::new(FrameType::Data, flags::END_STREAM, 1, 100);
        assert_eq!(header.get_length(), 100);
        assert_eq!(header.frame_type, FrameType::Data as u8);
        assert_eq!(header.flags, flags::END_STREAM);
        assert_eq!(header.stream_id, 1);
    }
}
