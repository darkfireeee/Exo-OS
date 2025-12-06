//! Zero-Copy Network Buffer Management
//!
//! Advanced buffer system that eliminates memory copies in the network stack.
//! Inspired by Linux's sk_buff but redesigned for Rust safety and performance.
//!
//! Features:
//! - Zero-copy packet forwarding
//! - Header manipulation without data copy
//! - Scatter-gather I/O support
//! - DMA-friendly memory layout
//! - Reference counting for shared buffers

use alloc::vec::Vec;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::ops::{Deref, DerefMut};

use crate::memory::PhysAddr;
use crate::net::{NetError, NetResult};

/// Network buffer (similar to Linux sk_buff)
///
/// This is the fundamental data structure for network packets.
/// It supports zero-copy operations through clever pointer manipulation.
pub struct NetBuffer {
    /// Reference to shared buffer data
    data: Arc<NetBufferData>,
    
    /// Offset to actual data start (allows prepending headers)
    head_offset: usize,
    
    /// Length of actual data
    len: usize,
    
    /// Packet metadata
    metadata: PacketMetadata,
}

/// Shared buffer data with reference counting
struct NetBufferData {
    /// Physical address (for DMA)
    phys_addr: PhysAddr,
    
    /// Actual buffer storage
    buffer: Vec<u8>,
    
    /// Reference count (atomic for lock-free sharing)
    refcount: AtomicUsize,
}

/// Packet metadata (control information)
#[derive(Debug, Clone, Default)]
pub struct PacketMetadata {
    /// Timestamp when packet was received/created
    pub timestamp: u64,
    
    /// Input interface ID
    pub if_index: u32,
    
    /// Protocol family (AF_INET, AF_INET6, etc.)
    pub protocol: u16,
    
    /// Packet priority (for QoS)
    pub priority: u8,
    
    /// Checksum status
    pub csum_valid: bool,
    pub csum_complete: bool,
    
    /// Hardware offload flags
    pub gso_type: GsoType,
    pub gso_size: u16,
    
    /// Layer 2 (Ethernet) info
    pub mac_header: u16,
    pub mac_len: u8,
    
    /// Layer 3 (IP) info  
    pub network_header: u16,
    pub network_len: u8,
    
    /// Layer 4 (TCP/UDP) info
    pub transport_header: u16,
    pub transport_len: u8,
}

/// Generic Segmentation Offload types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GsoType {
    #[default]
    None,
    TcpV4,
    TcpV6,
    Udp,
}

impl NetBuffer {
    /// Create new buffer with capacity
    pub fn new(capacity: usize) -> NetResult<Self> {
        let buffer = vec![0u8; capacity];
        let phys_addr = crate::memory::virt_to_phys(buffer.as_ptr() as usize)
            .ok_or(NetError::Memory(crate::memory::MemoryError::OutOfMemory))?;
        
        let data = Arc::new(NetBufferData {
            phys_addr,
            buffer,
            refcount: AtomicUsize::new(1),
        });
        
        Ok(Self {
            data,
            head_offset: 0,
            len: 0,
            metadata: PacketMetadata::default(),
        })
    }
    
    /// Create from existing data (copies data)
    pub fn from_slice(data: &[u8]) -> NetResult<Self> {
        let mut buffer = Self::new(data.len())?;
        buffer.push(data)?;
        Ok(buffer)
    }
    
    /// Create from raw pointer (zero-copy, for DMA)
    pub unsafe fn from_raw(ptr: *const u8, len: usize, phys_addr: PhysAddr) -> NetResult<Self> {
        let buffer = Vec::from_raw_parts(ptr as *mut u8, len, len);
        
        let data = Arc::new(NetBufferData {
            phys_addr,
            buffer,
            refcount: AtomicUsize::new(1),
        });
        
        Ok(Self {
            data,
            head_offset: 0,
            len,
            metadata: PacketMetadata::default(),
        })
    }
    
    /// Get physical address (for DMA)
    pub fn phys_addr(&self) -> PhysAddr {
        self.data.phys_addr + self.head_offset
    }
    
    /// Get reference to data slice
    pub fn data(&self) -> &[u8] {
        &self.data.buffer[self.head_offset..self.head_offset + self.len]
    }
    
    /// Get mutable reference to data slice
    pub fn data_mut(&mut self) -> NetResult<&mut [u8]> {
        // Ensure we have exclusive access (COW if needed)
        self.make_writable()?;
        
        let start = self.head_offset;
        let end = self.head_offset + self.len;
        Ok(&mut Arc::get_mut(&mut self.data).unwrap().buffer[start..end])
    }
    
    /// Reserve headroom for prepending headers
    pub fn reserve_headroom(&mut self, headroom: usize) -> NetResult<()> {
        if self.head_offset < headroom {
            // Need to shift data
            let new_capacity = self.data.buffer.len() + headroom;
            let mut new_buffer = vec![0u8; new_capacity];
            
            new_buffer[headroom..headroom + self.len].copy_from_slice(self.data());
            
            let phys_addr = crate::memory::virt_to_phys(new_buffer.as_ptr() as usize)
                .ok_or(NetError::Memory(crate::memory::MemoryError::OutOfMemory))?;
            
            self.data = Arc::new(NetBufferData {
                phys_addr,
                buffer: new_buffer,
                refcount: AtomicUsize::new(1),
            });
            
            self.head_offset = headroom;
        }
        
        Ok(())
    }
    
    /// Push (append) data to buffer
    pub fn push(&mut self, data: &[u8]) -> NetResult<()> {
        let new_len = self.len + data.len();
        
        if self.head_offset + new_len > self.data.buffer.len() {
            // Need to grow buffer
            let new_capacity = (self.head_offset + new_len).next_power_of_two();
            let mut new_buffer = vec![0u8; new_capacity];
            
            new_buffer[self.head_offset..self.head_offset + self.len]
                .copy_from_slice(self.data());
            
            let phys_addr = crate::memory::virt_to_phys(new_buffer.as_ptr() as usize)
                .ok_or(NetError::Memory(crate::memory::MemoryError::OutOfMemory))?;
            
            self.data = Arc::new(NetBufferData {
                phys_addr,
                buffer: new_buffer,
                refcount: AtomicUsize::new(1),
            });
        }
        
        self.make_writable()?;
        
        let buffer = &mut Arc::get_mut(&mut self.data).unwrap().buffer;
        buffer[self.head_offset + self.len..self.head_offset + new_len]
            .copy_from_slice(data);
        
        self.len = new_len;
        Ok(())
    }
    
    /// Push (prepend) header to buffer
    pub fn push_header(&mut self, header: &[u8]) -> NetResult<()> {
        if header.len() > self.head_offset {
            return Err(NetError::BufferFull);
        }
        
        self.make_writable()?;
        
        let new_offset = self.head_offset - header.len();
        let buffer = &mut Arc::get_mut(&mut self.data).unwrap().buffer;
        buffer[new_offset..self.head_offset].copy_from_slice(header);
        
        self.head_offset = new_offset;
        self.len += header.len();
        
        Ok(())
    }
    
    /// Pop (remove) header from buffer (zero-copy)
    pub fn pop_header(&mut self, len: usize) -> NetResult<()> {
        if len > self.len {
            return Err(NetError::InvalidPacket);
        }
        
        self.head_offset += len;
        self.len -= len;
        
        Ok(())
    }
    
    /// Clone buffer (increases refcount, zero-copy)
    pub fn shallow_clone(&self) -> Self {
        self.data.refcount.fetch_add(1, Ordering::SeqCst);
        
        Self {
            data: Arc::clone(&self.data),
            head_offset: self.head_offset,
            len: self.len,
            metadata: self.metadata.clone(),
        }
    }
    
    /// Ensure buffer is writable (COW if shared)
    fn make_writable(&mut self) -> NetResult<()> {
        if Arc::strong_count(&self.data) > 1 {
            // Buffer is shared, need to copy (COW)
            let mut new_buffer = vec![0u8; self.data.buffer.len()];
            new_buffer.copy_from_slice(&self.data.buffer);
            
            let phys_addr = crate::memory::virt_to_phys(new_buffer.as_ptr() as usize)
                .ok_or(NetError::Memory(crate::memory::MemoryError::OutOfMemory))?;
            
            self.data = Arc::new(NetBufferData {
                phys_addr,
                buffer: new_buffer,
                refcount: AtomicUsize::new(1),
            });
        }
        
        Ok(())
    }
    
    /// Get length
    pub fn len(&self) -> usize {
        self.len
    }
    
    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    
    /// Get metadata reference
    pub fn metadata(&self) -> &PacketMetadata {
        &self.metadata
    }
    
    /// Get mutable metadata reference
    pub fn metadata_mut(&mut self) -> &mut PacketMetadata {
        &mut self.metadata
    }
    
    /// Get capacity
    pub fn capacity(&self) -> usize {
        self.data.buffer.len()
    }
    
    /// Get available headroom
    pub fn headroom(&self) -> usize {
        self.head_offset
    }
    
    /// Get available tailroom
    pub fn tailroom(&self) -> usize {
        self.data.buffer.len() - self.head_offset - self.len
    }
}

impl Drop for NetBuffer {
    fn drop(&mut self) {
        // Decrement refcount
        if self.data.refcount.fetch_sub(1, Ordering::SeqCst) == 1 {
            // Last reference, buffer will be freed by Arc
        }
    }
}

impl Clone for NetBuffer {
    fn clone(&self) -> Self {
        self.shallow_clone()
    }
}

/// Scatter-gather list for zero-copy I/O
pub struct ScatterGatherList {
    /// List of buffer segments
    segments: Vec<SgSegment>,
    
    /// Total length across all segments
    total_len: usize,
}

/// Single scatter-gather segment
pub struct SgSegment {
    /// Physical address
    pub phys_addr: PhysAddr,
    
    /// Length
    pub len: usize,
    
    /// Virtual address (for CPU access)
    pub virt_addr: usize,
}

impl ScatterGatherList {
    pub fn new() -> Self {
        Self {
            segments: Vec::new(),
            total_len: 0,
        }
    }
    
    /// Add segment
    pub fn add_segment(&mut self, phys_addr: PhysAddr, virt_addr: usize, len: usize) {
        self.segments.push(SgSegment {
            phys_addr,
            len,
            virt_addr,
        });
        
        self.total_len += len;
    }
    
    /// Add NetBuffer as segment
    pub fn add_buffer(&mut self, buffer: &NetBuffer) {
        self.add_segment(
            buffer.phys_addr(),
            buffer.data().as_ptr() as usize,
            buffer.len(),
        );
    }
    
    /// Get total length
    pub fn len(&self) -> usize {
        self.total_len
    }
    
    /// Get segments
    pub fn segments(&self) -> &[SgSegment] {
        &self.segments
    }
    
    /// Coalesce into single buffer (copies data)
    pub fn coalesce(&self) -> NetResult<NetBuffer> {
        let mut buffer = NetBuffer::new(self.total_len)?;
        
        for segment in &self.segments {
            let data = unsafe {
                core::slice::from_raw_parts(segment.virt_addr as *const u8, segment.len)
            };
            buffer.push(data)?;
        }
        
        Ok(buffer)
    }
}

/// Ring buffer for network queues (lock-free SPSC)
pub struct NetRingBuffer<T> {
    buffer: Vec<Option<T>>,
    capacity: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl<T> NetRingBuffer<T> {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.next_power_of_two();
        let mut buffer = Vec::with_capacity(capacity);
        buffer.resize_with(capacity, || None);
        
        Self {
            buffer,
            capacity,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }
    
    /// Push element (returns false if full)
    pub fn push(&mut self, value: T) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        
        let next_tail = (tail + 1) & (self.capacity - 1);
        
        if next_tail == head {
            // Queue full
            return false;
        }
        
        self.buffer[tail] = Some(value);
        self.tail.store(next_tail, Ordering::Release);
        
        true
    }
    
    /// Pop element (returns None if empty)
    pub fn pop(&mut self) -> Option<T> {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        
        if head == tail {
            // Queue empty
            return None;
        }
        
        let value = self.buffer[head].take();
        let next_head = (head + 1) & (self.capacity - 1);
        self.head.store(next_head, Ordering::Release);
        
        value
    }
    
    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Acquire) == self.tail.load(Ordering::Acquire)
    }
    
    /// Check if full
    pub fn is_full(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        ((tail + 1) & (self.capacity - 1)) == head
    }
    
    /// Get length
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        
        if tail >= head {
            tail - head
        } else {
            self.capacity - head + tail
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_netbuffer_push() {
        let mut buf = NetBuffer::new(128).unwrap();
        buf.push(b"Hello").unwrap();
        buf.push(b" World").unwrap();
        
        assert_eq!(buf.len(), 11);
        assert_eq!(buf.data(), b"Hello World");
    }
    
    #[test]
    fn test_netbuffer_headers() {
        let mut buf = NetBuffer::new(128).unwrap();
        buf.reserve_headroom(32).unwrap();
        buf.push(b"Payload").unwrap();
        
        buf.push_header(b"IP:").unwrap();
        buf.push_header(b"ETH:").unwrap();
        
        assert_eq!(buf.data(), b"ETH:IP:Payload");
    }
    
    #[test]
    fn test_ring_buffer() {
        let mut ring = NetRingBuffer::new(4);
        
        assert!(ring.push(1));
        assert!(ring.push(2));
        assert!(ring.push(3));
        
        assert_eq!(ring.pop(), Some(1));
        assert_eq!(ring.pop(), Some(2));
        
        assert!(ring.push(4));
        assert!(ring.push(5));
        
        assert_eq!(ring.len(), 3);
    }
}
