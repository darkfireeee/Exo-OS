//! Packet Buffer - sk_buff equivalent
//!
//! Efficient packet buffer management with zero-copy support

use alloc::vec::Vec;
use alloc::boxed::Box;
use core::ptr;

/// Maximum packet size
pub const MAX_PACKET_SIZE: usize = 2048;

/// Packet buffer (similar to Linux sk_buff)
pub struct PacketBuffer {
    /// Raw data
    data: Vec<u8>,
    
    /// Head pointer (start of headers)
    head: usize,
    
    /// Data pointer (start of payload)
    data_ptr: usize,
    
    /// Tail pointer (end of data)
    tail: usize,
    
    /// End pointer (end of buffer)
    end: usize,
    
    /// Length of data
    len: usize,
    
    /// Protocol hints
    protocol: Protocol,
    
    /// Checksum status
    checksum: ChecksumStatus,
}

/// Network protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Unknown,
    Ethernet,
    Arp,
    Ipv4,
    Ipv6,
    Icmp,
    Tcp,
    Udp,
}

/// Checksum status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumStatus {
    None,
    Partial,
    Complete,
    Unnecessary,
}

impl PacketBuffer {
    /// Create new packet buffer
    pub fn new(capacity: usize) -> Self {
        let mut data = Vec::with_capacity(capacity);
        data.resize(capacity, 0);
        
        Self {
            data,
            head: 0,
            data_ptr: 0,
            tail: 0,
            end: capacity,
            len: 0,
            protocol: Protocol::Unknown,
            checksum: ChecksumStatus::None,
        }
    }
    
    /// Create with default capacity
    pub fn with_default_capacity() -> Self {
        Self::new(MAX_PACKET_SIZE)
    }
    
    /// Reserve headroom for headers
    pub fn reserve_headroom(&mut self, len: usize) {
        self.data_ptr = len;
        self.tail = len;
    }
    
    /// Add data to tail
    pub fn put(&mut self, data: &[u8]) -> Result<(), BufferError> {
        if self.tail + data.len() > self.end {
            return Err(BufferError::NoSpace);
        }
        
        self.data[self.tail..self.tail + data.len()].copy_from_slice(data);
        self.tail += data.len();
        self.len += data.len();
        
        Ok(())
    }
    
    /// Add header space (move data_ptr back)
    pub fn push(&mut self, len: usize) -> Result<&mut [u8], BufferError> {
        if self.data_ptr < len {
            return Err(BufferError::NoSpace);
        }
        
        self.data_ptr -= len;
        self.len += len;
        
        Ok(&mut self.data[self.data_ptr..self.data_ptr + len])
    }
    
    /// Remove header space (move data_ptr forward)
    pub fn pull(&mut self, len: usize) -> Result<&[u8], BufferError> {
        if self.len < len {
            return Err(BufferError::NotEnoughData);
        }
        
        let slice = &self.data[self.data_ptr..self.data_ptr + len];
        self.data_ptr += len;
        self.len -= len;
        
        Ok(slice)
    }
    
    /// Get data slice
    pub fn data(&self) -> &[u8] {
        &self.data[self.data_ptr..self.tail]
    }
    
    /// Get mutable data slice
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data[self.data_ptr..self.tail]
    }
    
    /// Get data length
    pub fn len(&self) -> usize {
        self.len
    }
    
    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    
    /// Get headroom available
    pub fn headroom(&self) -> usize {
        self.data_ptr - self.head
    }
    
    /// Get tailroom available
    pub fn tailroom(&self) -> usize {
        self.end - self.tail
    }
    
    /// Set protocol
    pub fn set_protocol(&mut self, protocol: Protocol) {
        self.protocol = protocol;
    }
    
    /// Get protocol
    pub fn protocol(&self) -> Protocol {
        self.protocol
    }
    
    /// Clone packet (deep copy)
    pub fn clone_packet(&self) -> Self {
        Self {
            data: self.data.clone(),
            head: self.head,
            data_ptr: self.data_ptr,
            tail: self.tail,
            end: self.end,
            len: self.len,
            protocol: self.protocol,
            checksum: self.checksum,
        }
    }
    
    /// Reset buffer
    pub fn reset(&mut self) {
        self.head = 0;
        self.data_ptr = 0;
        self.tail = 0;
        self.len = 0;
        self.protocol = Protocol::Unknown;
        self.checksum = ChecksumStatus::None;
    }
}

/// Buffer errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferError {
    NoSpace,
    NotEnoughData,
    InvalidOffset,
}

/// Packet buffer pool for efficient allocation
pub struct PacketBufferPool {
    /// Pre-allocated buffers
    pool: spin::Mutex<Vec<PacketBuffer>>,
    
    /// Pool capacity
    capacity: usize,
}

impl PacketBufferPool {
    pub const fn new() -> Self {
        Self {
            pool: spin::Mutex::new(Vec::new()),
            capacity: 256,
        }
    }
    
    /// Initialize pool with pre-allocated buffers
    pub fn init(&self, count: usize) {
        let mut pool = self.pool.lock();
        
        for _ in 0..count {
            pool.push(PacketBuffer::with_default_capacity());
        }
        
        crate::logger::info(&alloc::format!(
            "[NET] Packet buffer pool initialized with {} buffers",
            count
        ));
    }
    
    /// Allocate buffer from pool
    pub fn alloc(&self) -> PacketBuffer {
        let mut pool = self.pool.lock();
        
        if let Some(mut buffer) = pool.pop() {
            buffer.reset();
            buffer
        } else {
            // Pool empty, allocate new
            PacketBuffer::with_default_capacity()
        }
    }
    
    /// Return buffer to pool
    pub fn free(&self, buffer: PacketBuffer) {
        let mut pool = self.pool.lock();
        
        if pool.len() < self.capacity {
            pool.push(buffer);
        }
        // Otherwise, drop (let allocator handle it)
    }
}

/// Global packet buffer pool
pub static PACKET_POOL: PacketBufferPool = PacketBufferPool::new();

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_packet_buffer() {
        let mut pkt = PacketBuffer::new(256);
        
        // Reserve headroom for headers
        pkt.reserve_headroom(64);
        
        // Add payload
        pkt.put(b"Hello, Network!").unwrap();
        assert_eq!(pkt.len(), 15);
        
        // Add ethernet header
        let eth_header = pkt.push(14).unwrap();
        assert_eq!(eth_header.len(), 14);
        assert_eq!(pkt.len(), 29);
    }
    
    #[test]
    fn test_headroom_tailroom() {
        let mut pkt = PacketBuffer::new(256);
        pkt.reserve_headroom(64);
        
        assert_eq!(pkt.headroom(), 64);
        assert_eq!(pkt.tailroom(), 192);
    }
}
