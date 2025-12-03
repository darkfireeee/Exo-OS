//! Advanced Channels - Multicast, Anycast, and Priority Channels
//!
//! High-level channel abstractions built on UltraFastRing.
//!
//! ## Channel Types:
//! - **PriorityChannel**: Separate queues per priority class
//! - **MulticastChannel**: One sender, multiple receivers
//! - **AnycastChannel**: Load-balanced distribution to receivers
//! - **RequestReplyChannel**: Correlated request/response pairs

use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::sync::Arc;
use spin::Mutex;

use super::ultra_fast_ring::UltraFastRing;
use super::advanced::{
    PriorityClass, MulticastGroup, MulticastReceiver,
    AnycastGroup, AnycastPolicy, LaneStats, rdtsc,
};
use crate::memory::{MemoryResult, MemoryError};
use crate::ipc::IpcResult;

// =============================================================================
// PRIORITY CHANNEL
// =============================================================================

/// Priority channel with separate queues per priority class
///
/// Messages are always delivered in priority order - higher priority
/// messages are received before lower priority ones, regardless of
/// when they were sent.
pub struct PriorityChannel {
    /// Separate ring per priority (5 levels)
    rings: [Box<UltraFastRing>; 5],
    
    /// Channel ID
    id: u64,
    
    /// Closed flag
    closed: AtomicBool,
    
    /// Total messages sent
    total_sent: AtomicU64,
    
    /// Total messages received
    total_received: AtomicU64,
}

impl PriorityChannel {
    /// Create new priority channel with capacity per lane
    pub fn new(capacity_per_lane: usize) -> Self {
        Self {
            rings: [
                UltraFastRing::new(capacity_per_lane),
                UltraFastRing::new(capacity_per_lane),
                UltraFastRing::new(capacity_per_lane),
                UltraFastRing::new(capacity_per_lane),
                UltraFastRing::new(capacity_per_lane),
            ],
            id: crate::ipc::alloc_channel_id(),
            closed: AtomicBool::new(false),
            total_sent: AtomicU64::new(0),
            total_received: AtomicU64::new(0),
        }
    }
    
    /// Get channel ID
    pub fn id(&self) -> u64 {
        self.id
    }
    
    /// Send message with priority
    #[inline]
    pub fn send(&self, data: &[u8], priority: PriorityClass) -> MemoryResult<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(MemoryError::NotFound);
        }
        
        let ring = &self.rings[priority as usize];
        ring.send_fast(data, priority)?;
        self.total_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    
    /// Send blocking
    pub fn send_blocking(&self, data: &[u8], priority: PriorityClass) -> MemoryResult<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(MemoryError::NotFound);
        }
        
        let ring = &self.rings[priority as usize];
        ring.send_blocking(data, priority)?;
        self.total_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    
    /// Receive highest priority message available
    #[inline]
    pub fn recv(&self, buffer: &mut [u8]) -> MemoryResult<(usize, PriorityClass)> {
        if self.closed.load(Ordering::Acquire) && self.is_empty() {
            return Err(MemoryError::NotFound);
        }
        
        // Check priorities in order (highest first)
        for priority in [
            PriorityClass::RealTime,
            PriorityClass::High,
            PriorityClass::Normal,
            PriorityClass::Low,
            PriorityClass::Bulk,
        ] {
            let ring = &self.rings[priority as usize];
            if let Ok((size, _, _)) = ring.recv_fast(buffer) {
                self.total_received.fetch_add(1, Ordering::Relaxed);
                return Ok((size, priority));
            }
        }
        
        Err(MemoryError::NotFound)
    }
    
    /// Receive blocking
    pub fn recv_blocking(&self, buffer: &mut [u8]) -> MemoryResult<(usize, PriorityClass)> {
        loop {
            match self.recv(buffer) {
                Ok(result) => return Ok(result),
                Err(MemoryError::NotFound) if !self.closed.load(Ordering::Acquire) => {
                    crate::scheduler::yield_now();
                }
                Err(e) => return Err(e),
            }
        }
    }
    
    /// Check if all lanes are empty
    pub fn is_empty(&self) -> bool {
        self.rings.iter().all(|r| r.is_empty())
    }
    
    /// Get total length across all lanes
    pub fn len(&self) -> usize {
        self.rings.iter().map(|r| r.len()).sum()
    }
    
    /// Close the channel
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
    
    /// Get statistics
    pub fn stats(&self) -> PriorityChannelStats {
        PriorityChannelStats {
            id: self.id,
            total_sent: self.total_sent.load(Ordering::Relaxed),
            total_received: self.total_received.load(Ordering::Relaxed),
            lane_lengths: [
                self.rings[0].len(),
                self.rings[1].len(),
                self.rings[2].len(),
                self.rings[3].len(),
                self.rings[4].len(),
            ],
            closed: self.closed.load(Ordering::Relaxed),
        }
    }
}

/// Priority channel statistics
#[derive(Debug, Clone)]
pub struct PriorityChannelStats {
    pub id: u64,
    pub total_sent: u64,
    pub total_received: u64,
    pub lane_lengths: [usize; 5],
    pub closed: bool,
}

// =============================================================================
// MULTICAST CHANNEL
// =============================================================================

/// Multicast channel - one sender, multiple receivers
///
/// Each receiver gets a copy of every message. Slow receivers
/// will eventually drop messages (configurable).
pub struct MulticastChannel {
    /// Shared ring buffer
    ring: Box<UltraFastRing>,
    
    /// Receiver states
    receivers: Mutex<Vec<Arc<MulticastReceiverState>>>,
    
    /// Channel ID
    id: u64,
    
    /// Current producer sequence
    producer_seq: AtomicU64,
    
    /// Max lag before dropping (0 = never drop)
    max_lag: u64,
    
    /// Closed flag
    closed: AtomicBool,
}

/// Per-receiver state for multicast
#[repr(C, align(64))]
pub struct MulticastReceiverState {
    /// Receiver's current sequence
    sequence: AtomicU64,
    /// Active flag
    active: AtomicBool,
    /// Messages dropped due to lag
    dropped: AtomicU64,
    /// Receiver ID
    id: u64,
    _pad: [u8; 31],
}

impl MulticastChannel {
    /// Create new multicast channel
    pub fn new(capacity: usize, max_lag: u64) -> Self {
        Self {
            ring: UltraFastRing::new(capacity),
            receivers: Mutex::new(Vec::new()),
            id: crate::ipc::alloc_channel_id(),
            producer_seq: AtomicU64::new(0),
            max_lag,
            closed: AtomicBool::new(false),
        }
    }
    
    /// Send to all receivers
    pub fn send(&self, data: &[u8], priority: PriorityClass) -> MemoryResult<usize> {
        if self.closed.load(Ordering::Acquire) {
            return Err(MemoryError::NotFound);
        }
        
        // Send to ring
        self.ring.send_fast(data, priority)?;
        
        let seq = self.producer_seq.fetch_add(1, Ordering::Release);
        
        // Check for slow receivers (drop if too far behind)
        if self.max_lag > 0 {
            let receivers = self.receivers.lock();
            for recv in receivers.iter() {
                if recv.active.load(Ordering::Relaxed) {
                    let recv_seq = recv.sequence.load(Ordering::Relaxed);
                    if seq.wrapping_sub(recv_seq) > self.max_lag {
                        recv.dropped.fetch_add(1, Ordering::Relaxed);
                        // Advance slow receiver to avoid blocking
                        recv.sequence.store(seq.wrapping_sub(self.max_lag / 2), Ordering::Relaxed);
                    }
                }
            }
        }
        
        // Return number of active receivers
        Ok(self.receiver_count())
    }
    
    /// Subscribe (create new receiver)
    pub fn subscribe(&self) -> Arc<MulticastReceiverState> {
        let receiver = Arc::new(MulticastReceiverState {
            sequence: AtomicU64::new(self.producer_seq.load(Ordering::Acquire)),
            active: AtomicBool::new(true),
            dropped: AtomicU64::new(0),
            id: crate::ipc::alloc_channel_id(),
            _pad: [0; 31],
        });
        
        self.receivers.lock().push(Arc::clone(&receiver));
        receiver
    }
    
    /// Unsubscribe receiver
    pub fn unsubscribe(&self, receiver: &MulticastReceiverState) {
        receiver.active.store(false, Ordering::Release);
        
        // Cleanup inactive receivers periodically
        let mut receivers = self.receivers.lock();
        receivers.retain(|r| r.active.load(Ordering::Relaxed));
    }
    
    /// Receive for specific receiver
    pub fn recv(&self, receiver: &MulticastReceiverState, buffer: &mut [u8]) -> MemoryResult<usize> {
        if !receiver.active.load(Ordering::Acquire) {
            return Err(MemoryError::NotFound);
        }
        
        // Check if data available
        let recv_seq = receiver.sequence.load(Ordering::Acquire);
        let prod_seq = self.producer_seq.load(Ordering::Acquire);
        
        if recv_seq >= prod_seq {
            return Err(MemoryError::NotFound);
        }
        
        // Receive from ring
        let (size, _, _) = self.ring.recv_fast(buffer)?;
        
        // Advance receiver sequence
        receiver.sequence.fetch_add(1, Ordering::Release);
        
        Ok(size)
    }
    
    /// Get number of active receivers
    pub fn receiver_count(&self) -> usize {
        self.receivers.lock()
            .iter()
            .filter(|r| r.active.load(Ordering::Relaxed))
            .count()
    }
    
    /// Close channel
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
}

// =============================================================================
// ANYCAST CHANNEL (LOAD BALANCED)
// =============================================================================

/// Anycast channel - load-balanced distribution to receivers
///
/// Each message goes to exactly one receiver, selected by policy.
pub struct AnycastChannel {
    /// Per-receiver rings
    receivers: Mutex<Vec<(Arc<AnycastReceiverState>, Box<UltraFastRing>)>>,
    
    /// Channel ID
    id: u64,
    
    /// Load balancing policy
    policy: AnycastPolicy,
    
    /// Round-robin counter
    next_receiver: AtomicU64,
    
    /// Closed flag
    closed: AtomicBool,
    
    /// Total sent
    total_sent: AtomicU64,
}

/// Per-receiver state for anycast
pub struct AnycastReceiverState {
    /// Receiver ID
    pub id: u64,
    /// Active flag
    active: AtomicBool,
    /// Pending messages
    pending: AtomicU64,
    /// Total received
    received: AtomicU64,
}

impl AnycastChannel {
    /// Create new anycast channel
    pub fn new(policy: AnycastPolicy) -> Self {
        Self {
            receivers: Mutex::new(Vec::new()),
            id: crate::ipc::alloc_channel_id(),
            policy,
            next_receiver: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            total_sent: AtomicU64::new(0),
        }
    }
    
    /// Add receiver
    pub fn add_receiver(&self, capacity: usize) -> Arc<AnycastReceiverState> {
        let receiver = Arc::new(AnycastReceiverState {
            id: crate::ipc::alloc_channel_id(),
            active: AtomicBool::new(true),
            pending: AtomicU64::new(0),
            received: AtomicU64::new(0),
        });
        
        let ring = UltraFastRing::new(capacity);
        self.receivers.lock().push((Arc::clone(&receiver), ring));
        
        receiver
    }
    
    /// Send to one receiver (load balanced)
    pub fn send(&self, data: &[u8], priority: PriorityClass) -> MemoryResult<u64> {
        if self.closed.load(Ordering::Acquire) {
            return Err(MemoryError::NotFound);
        }
        
        let receivers = self.receivers.lock();
        if receivers.is_empty() {
            return Err(MemoryError::NotFound);
        }
        
        // Select receiver based on policy
        let idx = self.select_receiver(&receivers)?;
        let (ref receiver, ref ring) = receivers[idx];
        
        // Send to selected receiver's ring
        ring.send_fast(data, priority)?;
        receiver.pending.fetch_add(1, Ordering::Relaxed);
        self.total_sent.fetch_add(1, Ordering::Relaxed);
        
        Ok(receiver.id)
    }
    
    /// Select receiver based on policy
    fn select_receiver(&self, receivers: &[(Arc<AnycastReceiverState>, Box<UltraFastRing>)]) -> MemoryResult<usize> {
        let active: Vec<usize> = receivers.iter()
            .enumerate()
            .filter(|(_, (r, _))| r.active.load(Ordering::Relaxed))
            .map(|(i, _)| i)
            .collect();
        
        if active.is_empty() {
            return Err(MemoryError::NotFound);
        }
        
        let idx = match self.policy {
            AnycastPolicy::RoundRobin => {
                let next = self.next_receiver.fetch_add(1, Ordering::Relaxed);
                active[(next as usize) % active.len()]
            }
            AnycastPolicy::LeastLoaded => {
                let mut min_load = u64::MAX;
                let mut min_idx = active[0];
                
                for &i in &active {
                    let load = receivers[i].0.pending.load(Ordering::Relaxed);
                    if load < min_load {
                        min_load = load;
                        min_idx = i;
                    }
                }
                
                min_idx
            }
            AnycastPolicy::Random => {
                let seed = rdtsc();
                active[(seed as usize) % active.len()]
            }
            AnycastPolicy::AffinityFirst => {
                // TODO: NUMA awareness
                active[0]
            }
        };
        
        Ok(idx)
    }
    
    /// Receive for specific receiver
    pub fn recv(&self, receiver: &AnycastReceiverState, buffer: &mut [u8]) -> MemoryResult<usize> {
        let receivers = self.receivers.lock();
        
        // Find receiver's ring
        for (recv, ring) in receivers.iter() {
            if recv.id == receiver.id {
                let (size, _, _) = ring.recv_fast(buffer)?;
                receiver.pending.fetch_sub(1, Ordering::Relaxed);
                receiver.received.fetch_add(1, Ordering::Relaxed);
                return Ok(size);
            }
        }
        
        Err(MemoryError::NotFound)
    }
    
    /// Remove receiver
    pub fn remove_receiver(&self, receiver: &AnycastReceiverState) {
        receiver.active.store(false, Ordering::Release);
        
        let mut receivers = self.receivers.lock();
        receivers.retain(|(r, _)| r.active.load(Ordering::Relaxed));
    }
    
    /// Close channel
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
    
    /// Get receiver count
    pub fn receiver_count(&self) -> usize {
        self.receivers.lock()
            .iter()
            .filter(|(r, _)| r.active.load(Ordering::Relaxed))
            .count()
    }
}

// =============================================================================
// REQUEST-REPLY CHANNEL
// =============================================================================

/// Request-reply channel with correlated responses
pub struct RequestReplyChannel {
    /// Request ring
    request_ring: Box<UltraFastRing>,
    
    /// Response ring
    response_ring: Box<UltraFastRing>,
    
    /// Pending requests (correlation ID -> timestamp)
    pending: Mutex<alloc::collections::BTreeMap<u64, u64>>,
    
    /// Next correlation ID
    next_corr_id: AtomicU64,
    
    /// Channel ID
    id: u64,
    
    /// Closed flag
    closed: AtomicBool,
}

impl RequestReplyChannel {
    /// Create new request-reply channel
    pub fn new(capacity: usize) -> Self {
        Self {
            request_ring: UltraFastRing::new(capacity),
            response_ring: UltraFastRing::new(capacity),
            pending: Mutex::new(alloc::collections::BTreeMap::new()),
            next_corr_id: AtomicU64::new(1),
            id: crate::ipc::alloc_channel_id(),
            closed: AtomicBool::new(false),
        }
    }
    
    /// Send request and get correlation ID
    pub fn send_request(&self, data: &[u8]) -> MemoryResult<u64> {
        let corr_id = self.next_corr_id.fetch_add(1, Ordering::Relaxed);
        
        // Prepend correlation ID to message
        let mut msg = Vec::with_capacity(8 + data.len());
        msg.extend_from_slice(&corr_id.to_le_bytes());
        msg.extend_from_slice(data);
        
        self.request_ring.send_fast(&msg, PriorityClass::Normal)?;
        
        // Track pending request
        self.pending.lock().insert(corr_id, rdtsc());
        
        Ok(corr_id)
    }
    
    /// Receive request (server side)
    pub fn recv_request(&self, buffer: &mut [u8]) -> MemoryResult<(u64, usize)> {
        let mut temp = [0u8; 48]; // Max inline size
        let (size, _, _) = self.request_ring.recv_fast(&mut temp)?;
        
        if size < 8 {
            return Err(MemoryError::InvalidSize);
        }
        
        let corr_id = u64::from_le_bytes(temp[..8].try_into().unwrap());
        let data_size = size - 8;
        
        buffer[..data_size].copy_from_slice(&temp[8..size]);
        
        Ok((corr_id, data_size))
    }
    
    /// Send response with correlation ID
    pub fn send_response(&self, corr_id: u64, data: &[u8]) -> MemoryResult<()> {
        let mut msg = Vec::with_capacity(8 + data.len());
        msg.extend_from_slice(&corr_id.to_le_bytes());
        msg.extend_from_slice(data);
        
        self.response_ring.send_fast(&msg, PriorityClass::Normal)
    }
    
    /// Receive response for specific correlation ID
    /// Returns (data_size, latency_cycles)
    pub fn recv_response(&self, expected_corr_id: u64, buffer: &mut [u8]) -> MemoryResult<(usize, u64)> {
        let mut temp = [0u8; 48];
        let (size, _, _) = self.response_ring.recv_fast(&mut temp)?;
        
        if size < 8 {
            return Err(MemoryError::InvalidSize);
        }
        
        let corr_id = u64::from_le_bytes(temp[..8].try_into().unwrap());
        
        if corr_id != expected_corr_id {
            // TODO: Queue for later / allow out-of-order
            return Err(MemoryError::NotFound);
        }
        
        let data_size = size - 8;
        buffer[..data_size].copy_from_slice(&temp[8..size]);
        
        // Calculate latency
        let latency = if let Some(start_tsc) = self.pending.lock().remove(&corr_id) {
            rdtsc().saturating_sub(start_tsc)
        } else {
            0
        };
        
        Ok((data_size, latency))
    }
    
    /// Close channel
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
}
