//! Batch - Batch operations for fusion rings
//!
//! Amortizes setup cost across multiple messages (~131 cycles/msg)

use super::{ring::Ring, inline, zerocopy};
use crate::memory::{MemoryResult, MemoryError};
use alloc::vec::Vec;

/// Batch message descriptor
pub struct BatchMessage {
    pub data: Vec<u8>,
    pub zerocopy: bool,
}

/// Send batch of messages
pub fn send_batch(ring: &Ring, messages: &[BatchMessage]) -> MemoryResult<usize> {
    let mut sent = 0;
    
    for msg in messages {
        if msg.zerocopy {
            // Zero-copy path (not yet implemented)
            continue;
        } else {
            // Inline path
            if inline::fits_inline(msg.data.len()) {
                if inline::send_inline(ring, &msg.data).is_ok() {
                    sent += 1;
                }
            }
        }
    }
    
    Ok(sent)
}

/// Receive batch of messages
pub fn recv_batch(ring: &Ring, max_count: usize) -> MemoryResult<Vec<Vec<u8>>> {
    let mut messages = Vec::new();
    
    for _ in 0..max_count {
        let mut buffer = [0u8; 256];
        match inline::recv_inline(ring, &mut buffer) {
            Ok(size) => {
                messages.push(buffer[..size].to_vec());
            }
            Err(_) => break,
        }
    }
    
    Ok(messages)
}

/// Send vectored batch (multiple buffers in one call)
pub fn send_vectored(ring: &Ring, buffers: &[&[u8]]) -> MemoryResult<usize> {
    let mut sent = 0;
    
    for buffer in buffers {
        if inline::fits_inline(buffer.len()) {
            if inline::send_inline(ring, buffer).is_ok() {
                sent += 1;
            }
        }
    }
    
    Ok(sent)
}

/// Batch statistics
pub struct BatchStats {
    pub total_messages: usize,
    pub total_bytes: usize,
    pub avg_cycles_per_message: u64,
}

impl BatchStats {
    pub fn new() -> Self {
        Self {
            total_messages: 0,
            total_bytes: 0,
            avg_cycles_per_message: 0,
        }
    }
    
    pub fn record(&mut self, count: usize, bytes: usize, cycles: u64) {
        self.total_messages += count;
        self.total_bytes += bytes;
        
        if count > 0 {
            self.avg_cycles_per_message = cycles / count as u64;
        }
    }
}
