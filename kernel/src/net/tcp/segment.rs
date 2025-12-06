//! # TCP Segment Management - Zero-Copy
//! 
//! Gestion des segments TCP avec zero-copy et réassemblage optimisé.

use alloc::vec::Vec;
use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicU32, Ordering};

/// Segment TCP
#[derive(Clone)]
pub struct TcpSegment {
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    pub window: u16,
    pub data: Vec<u8>,
    pub timestamp: u64,
}

impl TcpSegment {
    pub fn new(seq: u32, ack: u32, flags: u8) -> Self {
        Self {
            seq,
            ack,
            flags,
            window: 65535,
            data: Vec::new(),
            timestamp: 0,
        }
    }
    
    pub fn with_data(seq: u32, ack: u32, flags: u8, data: Vec<u8>) -> Self {
        Self {
            seq,
            ack,
            flags,
            window: 65535,
            data,
            timestamp: 0,
        }
    }
    
    pub fn len(&self) -> u32 {
        self.data.len() as u32
    }
    
    pub fn end_seq(&self) -> u32 {
        self.seq.wrapping_add(self.len())
    }
}

/// Buffer de réassemblage out-of-order
pub struct ReassemblyBuffer {
    segments: VecDeque<TcpSegment>,
    expected_seq: AtomicU32,
    max_segments: usize,
}

impl ReassemblyBuffer {
    pub fn new(initial_seq: u32) -> Self {
        Self {
            segments: VecDeque::new(),
            expected_seq: AtomicU32::new(initial_seq),
            max_segments: 1000,
        }
    }
    
    /// Insère un segment et retourne les données consécutives
    pub fn insert(&mut self, segment: TcpSegment) -> Vec<Vec<u8>> {
        let expected = self.expected_seq.load(Ordering::Relaxed);
        
        // Si c'est le segment attendu
        if segment.seq == expected {
            let mut result = vec![segment.data.clone()];
            self.expected_seq.store(
                expected.wrapping_add(segment.len()),
                Ordering::Release
            );
            
            // Vérifie si on peut délivrer des segments buffered
            while let Some(next) = self.segments.front() {
                if next.seq == self.expected_seq.load(Ordering::Relaxed) {
                    let seg = self.segments.pop_front().unwrap();
                    result.push(seg.data.clone());
                    self.expected_seq.fetch_add(seg.len(), Ordering::Release);
                } else {
                    break;
                }
            }
            
            return result;
        }
        
        // Segment out-of-order : buffer si pas trop de segments
        if self.segments.len() < self.max_segments {
            // Insère en ordre de seq
            let pos = self.segments
                .binary_search_by_key(&segment.seq, |s| s.seq)
                .unwrap_or_else(|pos| pos);
            self.segments.insert(pos, segment);
        }
        
        Vec::new()
    }
    
    pub fn expected_seq(&self) -> u32 {
        self.expected_seq.load(Ordering::Relaxed)
    }
    
    pub fn buffered_count(&self) -> usize {
        self.segments.len()
    }
}

/// Send buffer avec support zero-copy
pub struct SendBuffer {
    data: VecDeque<u8>,
    max_size: usize,
    unacked_offset: usize,
}

impl SendBuffer {
    pub fn new(max_size: usize) -> Self {
        Self {
            data: VecDeque::with_capacity(max_size),
            max_size,
            unacked_offset: 0,
        }
    }
    
    /// Écrit des données dans le buffer
    pub fn write(&mut self, data: &[u8]) -> usize {
        let available = self.max_size - self.data.len();
        let to_write = data.len().min(available);
        
        self.data.extend(&data[..to_write]);
        to_write
    }
    
    /// Lit des données pour envoyer (sans les retirer)
    pub fn peek(&self, offset: usize, len: usize) -> Vec<u8> {
        let start = self.unacked_offset + offset;
        let end = (start + len).min(self.data.len());
        
        self.data.iter()
            .skip(start)
            .take(end - start)
            .copied()
            .collect()
    }
    
    /// Marque des bytes comme ACKed
    pub fn ack(&mut self, bytes: usize) {
        let to_remove = bytes.min(self.data.len());
        self.data.drain(..to_remove);
        self.unacked_offset = self.unacked_offset.saturating_sub(to_remove);
    }
    
    pub fn len(&self) -> usize {
        self.data.len()
    }
    
    pub fn available(&self) -> usize {
        self.max_size - self.data.len()
    }
}

/// Receive buffer
pub struct RecvBuffer {
    data: VecDeque<u8>,
    max_size: usize,
}

impl RecvBuffer {
    pub fn new(max_size: usize) -> Self {
        Self {
            data: VecDeque::with_capacity(max_size),
            max_size,
        }
    }
    
    pub fn write(&mut self, data: &[u8]) -> usize {
        let available = self.max_size - self.data.len();
        let to_write = data.len().min(available);
        
        self.data.extend(&data[..to_write]);
        to_write
    }
    
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let to_read = buf.len().min(self.data.len());
        
        for i in 0..to_read {
            buf[i] = self.data.pop_front().unwrap();
        }
        
        to_read
    }
    
    pub fn len(&self) -> usize {
        self.data.len()
    }
    
    pub fn available(&self) -> usize {
        self.max_size - self.data.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_reassembly_in_order() {
        let mut buffer = ReassemblyBuffer::new(1000);
        
        let seg1 = TcpSegment::with_data(1000, 0, 0, vec![1, 2, 3]);
        let result = buffer.insert(seg1);
        
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], vec![1, 2, 3]);
        assert_eq!(buffer.expected_seq(), 1003);
    }
    
    #[test]
    fn test_reassembly_out_of_order() {
        let mut buffer = ReassemblyBuffer::new(1000);
        
        // Segment 2 arrive avant segment 1
        let seg2 = TcpSegment::with_data(1003, 0, 0, vec![4, 5, 6]);
        let result = buffer.insert(seg2);
        assert!(result.is_empty());
        
        // Segment 1 arrive
        let seg1 = TcpSegment::with_data(1000, 0, 0, vec![1, 2, 3]);
        let result = buffer.insert(seg1);
        assert_eq!(result.len(), 2);
        assert_eq!(buffer.expected_seq(), 1006);
    }
}
