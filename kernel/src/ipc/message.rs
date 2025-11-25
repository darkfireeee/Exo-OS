//! IPC Message format
//! 
//! Efficient message structure with inline and zero-copy support

use core::mem;
use alloc::vec::Vec;

/// Message types for classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    /// Regular data message
    Data = 0,
    /// Request message expecting response
    Request = 1,
    /// Response to a request
    Response = 2,
    /// Error notification
    Error = 3,
    /// Control message (open, close, etc.)
    Control = 4,
}

/// Message header (32 bytes)
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy)]
pub struct MessageHeader {
    /// Message type
    pub msg_type: MessageType,
    /// Message flags
    pub flags: u8,
    /// Priority (0 = lowest, 255 = highest)
    pub priority: u8,
    /// Reserved for alignment
    _reserved: u8,
    /// Total message size (header + payload)
    pub total_size: u32,
    /// Sender process ID
    pub sender: u64,
    /// Destination process ID
    pub dest: u64,
    /// Request ID for request/response matching
    pub request_id: u64,
}

impl MessageHeader {
    pub const SIZE: usize = mem::size_of::<Self>();
    
    pub fn new(msg_type: MessageType, sender: u64, dest: u64) -> Self {
        Self {
            msg_type,
            flags: 0,
            priority: 128,
            _reserved: 0,
            total_size: Self::SIZE as u32,
            sender,
            dest,
            request_id: 0,
        }
    }
    
    pub fn with_payload_size(mut self, payload_size: usize) -> Self {
        self.total_size = (Self::SIZE + payload_size) as u32;
        self
    }
    
    pub fn payload_size(&self) -> usize {
        (self.total_size as usize).saturating_sub(Self::SIZE)
    }
    
    pub fn is_inline(&self) -> bool {
        self.payload_size() <= INLINE_THRESHOLD
    }
}

/// Maximum payload size for inline messages (56 bytes)
/// Total inline message = 32 (header) + 56 (payload) = 88 bytes (fits in cache line)
pub const INLINE_THRESHOLD: usize = 56;

/// Message with inline or zero-copy payload
#[derive(Debug)]
pub enum Message {
    /// Small message with inline data (≤56 bytes)
    Inline {
        header: MessageHeader,
        data: [u8; INLINE_THRESHOLD],
    },
    /// Large message with heap-allocated data (>56 bytes)
    ZeroCopy {
        header: MessageHeader,
        data: Vec<u8>,
    },
}

impl Message {
    /// Create a new message with inline data
    pub fn new_inline(header: MessageHeader, data: &[u8]) -> Option<Self> {
        if data.len() > INLINE_THRESHOLD {
            return None;
        }
        
        let mut inline_data = [0u8; INLINE_THRESHOLD];
        inline_data[..data.len()].copy_from_slice(data);
        
        Some(Message::Inline {
            header: header.with_payload_size(data.len()),
            data: inline_data,
        })
    }
    
    /// Create a new message with zero-copy data
    pub fn new_zero_copy(header: MessageHeader, data: Vec<u8>) -> Self {
        Message::ZeroCopy {
            header: header.with_payload_size(data.len()),
            data,
        }
    }
    
    /// Create message automatically choosing inline or zero-copy
    pub fn new(header: MessageHeader, data: Vec<u8>) -> Self {
        if data.len() <= INLINE_THRESHOLD {
            let mut inline_data = [0u8; INLINE_THRESHOLD];
            inline_data[..data.len()].copy_from_slice(&data);
            Message::Inline {
                header: header.with_payload_size(data.len()),
                data: inline_data,
            }
        } else {
            Message::new_zero_copy(header, data)
        }
    }
    
    /// Get message header
    pub fn header(&self) -> &MessageHeader {
        match self {
            Message::Inline { header, .. } => header,
            Message::ZeroCopy { header, .. } => header,
        }
    }
    
    /// Get message payload as slice
    pub fn payload(&self) -> &[u8] {
        match self {
            Message::Inline { header, data } => {
                &data[..header.payload_size()]
            }
            Message::ZeroCopy { data, .. } => data.as_slice(),
        }
    }
    
    /// Check if message is inline
    pub fn is_inline(&self) -> bool {
        matches!(self, Message::Inline { .. })
    }
    
    /// Get total message size
    pub fn total_size(&self) -> usize {
        self.header().total_size as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_message_sizes() {
        assert_eq!(MessageHeader::SIZE, 32);
        assert_eq!(INLINE_THRESHOLD, 56);
        
        let header = MessageHeader::new(MessageType::Data, 1, 2);
        assert_eq!(header.total_size, 32);
    }
    
    #[test]
    fn test_inline_message() {
        let header = MessageHeader::new(MessageType::Data, 1, 2);
        let data = b"Hello, World!";
        
        let msg = Message::new_inline(header, data).unwrap();
        assert!(msg.is_inline());
        assert_eq!(msg.payload(), data);
    }
}
