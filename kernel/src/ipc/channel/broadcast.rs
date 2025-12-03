//! Broadcast Channels - One-to-Many IPC
//!
//! Allows one sender to broadcast messages to multiple receivers

use crate::ipc::fusion_ring::{FusionRing, Ring};
use crate::memory::{MemoryResult, MemoryError};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Broadcast sender
pub struct BroadcastSender {
    /// Rings for each receiver
    rings: Arc<Mutex<Vec<Arc<FusionRing>>>>,
    
    /// Number of receivers
    receiver_count: Arc<AtomicUsize>,
}

impl BroadcastSender {
    /// Send message to all receivers
    pub fn send(&self, data: &[u8]) -> MemoryResult<usize> {
        let rings = self.rings.lock();
        let mut sent_count = 0;
        
        for ring in rings.iter() {
            if ring.send(data).is_ok() {
                sent_count += 1;
            }
        }
        
        Ok(sent_count)
    }
    
    /// Get number of active receivers
    pub fn receiver_count(&self) -> usize {
        self.receiver_count.load(Ordering::Acquire)
    }
}

/// Broadcast receiver
pub struct BroadcastReceiver {
    /// Personal ring
    ring: Arc<FusionRing>,
    
    /// Reference to sender's ring list
    rings: Arc<Mutex<Vec<Arc<FusionRing>>>>,
    
    /// Receiver count
    receiver_count: Arc<AtomicUsize>,
    
    /// Index in ring list
    index: usize,
}

impl BroadcastReceiver {
    /// Receive message
    pub fn recv(&self, buffer: &mut [u8]) -> MemoryResult<usize> {
        self.ring.recv(buffer)
    }
    
    /// Try receive (non-blocking)
    pub fn try_recv(&self, buffer: &mut [u8]) -> MemoryResult<usize> {
        self.ring.recv(buffer)
    }
}

impl Drop for BroadcastReceiver {
    fn drop(&mut self) {
        // Remove from ring list
        let mut rings = self.rings.lock();
        if self.index < rings.len() {
            rings.remove(self.index);
        }
        
        // Decrement count
        self.receiver_count.fetch_sub(1, Ordering::Release);
    }
}

/// Broadcast channel
pub struct BroadcastChannel {
    sender: BroadcastSender,
    rings: Arc<Mutex<Vec<Arc<FusionRing>>>>,
    receiver_count: Arc<AtomicUsize>,
}

impl BroadcastChannel {
    /// Create new broadcast channel
    pub fn new() -> Self {
        let rings = Arc::new(Mutex::new(Vec::new()));
        let receiver_count = Arc::new(AtomicUsize::new(0));
        
        let sender = BroadcastSender {
            rings: rings.clone(),
            receiver_count: receiver_count.clone(),
        };
        
        Self {
            sender,
            rings,
            receiver_count,
        }
    }
    
    /// Get sender
    pub fn sender(&self) -> &BroadcastSender {
        &self.sender
    }
    
    /// Subscribe (create new receiver)
    pub fn subscribe(&self) -> MemoryResult<BroadcastReceiver> {
        // Create new ring for this receiver with proper initialization
        let ring = Arc::new(FusionRing::new(BROADCAST_RING_CAPACITY));
        
        let mut rings = self.rings.lock();
        let index = rings.len();
        rings.push(ring.clone());
        
        self.receiver_count.fetch_add(1, Ordering::Release);
        
        Ok(BroadcastReceiver {
            ring,
            rings: self.rings.clone(),
            receiver_count: self.receiver_count.clone(),
            index,
        })
    }
    
    /// Get number of active receivers
    pub fn receiver_count(&self) -> usize {
        self.receiver_count.load(Ordering::Acquire)
    }
}

/// Create broadcast channel with initial capacity
pub fn broadcast_channel(capacity: usize) -> BroadcastChannel {
    let mut channel = BroadcastChannel::new();
    
    // Pre-allocate space for receivers
    channel.rings.lock().reserve(capacity);
    
    channel
}

/// Default ring capacity for broadcast receivers
const BROADCAST_RING_CAPACITY: usize = 256;
