//! Endpoint - IPC Communication Endpoints
//!
//! An endpoint represents one side of an IPC channel. It provides:
//! - Type-safe message passing
//! - Capability-based security
//! - Efficient polling and blocking
//! - Priority-based message delivery
//!
//! ## Architecture:
//! ```text
//! ┌─────────────┐     ┌──────────────┐     ┌─────────────┐
//! │  Process A  │────▶│  MPMC Ring   │◀────│  Process B  │
//! │  Endpoint   │     │  (shared)    │     │  Endpoint   │
//! └─────────────┘     └──────────────┘     └─────────────┘
//! ```

use core::sync::atomic::{AtomicU64, AtomicU32, AtomicBool, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::string::String;
use spin::RwLock;

use super::mpmc_ring::MpmcRing;
use super::wait_queue::{WaitQueue, WakeReason, BlockingWait};
use super::transfer::{TransferMode, TransferResult};
use super::{ChannelHandle, ChannelStats};
use crate::memory::{MemoryResult, MemoryError};

/// Unique endpoint identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EndpointId(pub u64);

impl EndpointId {
    /// Generate new unique endpoint ID
    pub fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// Endpoint flags/capabilities
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointFlags(u32);

impl EndpointFlags {
    pub const NONE: Self = Self(0);
    /// Can send messages
    pub const SEND: Self = Self(1 << 0);
    /// Can receive messages
    pub const RECV: Self = Self(1 << 1);
    /// Can transfer endpoint to other process
    pub const TRANSFER: Self = Self(1 << 2);
    /// Has priority delivery
    pub const PRIORITY: Self = Self(1 << 3);
    /// Non-blocking only
    pub const NONBLOCK: Self = Self(1 << 4);
    /// Close on exec
    pub const CLOEXEC: Self = Self(1 << 5);
    /// Bidirectional (both send and recv)
    pub const BIDIRECTIONAL: Self = Self(Self::SEND.0 | Self::RECV.0);
    
    #[inline(always)]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
    
    #[inline(always)]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    
    #[inline(always)]
    pub fn can_send(self) -> bool {
        self.contains(Self::SEND)
    }
    
    #[inline(always)]
    pub fn can_recv(self) -> bool {
        self.contains(Self::RECV)
    }
}

/// Endpoint priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Priority {
    /// Background/bulk transfers
    Low = 0,
    /// Normal priority
    Normal = 128,
    /// High priority (interactive)
    High = 192,
    /// Real-time/critical
    Realtime = 255,
}

impl Default for Priority {
    fn default() -> Self {
        Priority::Normal
    }
}

/// Endpoint state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EndpointState {
    /// Active and usable
    Active = 0,
    /// Temporarily suspended
    Suspended = 1,
    /// Half-closed (can only recv or send)
    HalfClosed = 2,
    /// Fully closed
    Closed = 3,
}

/// IPC Endpoint
pub struct Endpoint {
    /// Unique identifier
    id: EndpointId,
    /// Channel this endpoint belongs to
    channel: ChannelHandle,
    /// Ring buffer (shared with peer)
    ring: Arc<MpmcRing>,
    /// Wait queue for blocking operations
    wait_queue: Arc<WaitQueue>,
    /// Endpoint flags/capabilities
    flags: AtomicU32,
    /// Current state
    state: AtomicU32,
    /// Priority
    priority: Priority,
    /// Owner process ID
    owner_pid: AtomicU64,
    /// Statistics
    stats: EndpointStats,
    /// Optional name for debugging
    name: Option<String>,
    /// Peer endpoint (if known)
    peer: RwLock<Option<EndpointId>>,
}

/// Per-endpoint statistics
#[derive(Debug, Default)]
pub struct EndpointStats {
    pub messages_sent: AtomicU64,
    pub messages_recv: AtomicU64,
    pub bytes_sent: AtomicU64,
    pub bytes_recv: AtomicU64,
    pub send_blocks: AtomicU64,
    pub recv_blocks: AtomicU64,
    pub errors: AtomicU64,
}

impl Endpoint {
    /// Create new endpoint
    pub fn new(
        channel: ChannelHandle,
        ring: Arc<MpmcRing>,
        wait_queue: Arc<WaitQueue>,
        flags: EndpointFlags,
    ) -> Self {
        Self {
            id: EndpointId::new(),
            channel,
            ring,
            wait_queue,
            flags: AtomicU32::new(flags.0),
            state: AtomicU32::new(EndpointState::Active as u32),
            priority: Priority::Normal,
            owner_pid: AtomicU64::new(0),
            stats: EndpointStats::default(),
            name: None,
            peer: RwLock::new(None),
        }
    }
    
    /// Create endpoint with name
    pub fn with_name(mut self, name: String) -> Self {
        self.name = Some(name);
        self
    }
    
    /// Create endpoint with priority
    pub fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }
    
    /// Get endpoint ID
    #[inline]
    pub fn id(&self) -> EndpointId {
        self.id
    }
    
    /// Get channel handle
    #[inline]
    pub fn channel(&self) -> ChannelHandle {
        self.channel
    }
    
    /// Get flags
    #[inline]
    pub fn flags(&self) -> EndpointFlags {
        EndpointFlags(self.flags.load(Ordering::Relaxed))
    }
    
    /// Get state
    #[inline]
    pub fn state(&self) -> EndpointState {
        match self.state.load(Ordering::Acquire) {
            0 => EndpointState::Active,
            1 => EndpointState::Suspended,
            2 => EndpointState::HalfClosed,
            _ => EndpointState::Closed,
        }
    }
    
    /// Check if endpoint is active
    #[inline]
    pub fn is_active(&self) -> bool {
        self.state() == EndpointState::Active
    }
    
    /// Set owner process
    pub fn set_owner(&self, pid: u64) {
        self.owner_pid.store(pid, Ordering::Release);
    }
    
    /// Get owner process
    #[inline]
    pub fn owner(&self) -> u64 {
        self.owner_pid.load(Ordering::Acquire)
    }
    
    // =========================================================================
    // SEND OPERATIONS
    // =========================================================================
    
    /// Send message (non-blocking)
    #[inline]
    pub fn try_send(&self, data: &[u8]) -> MemoryResult<()> {
        self.check_can_send()?;
        
        let result = self.ring.try_send_inline(data);
        
        if result.is_ok() {
            self.stats.messages_sent.fetch_add(1, Ordering::Relaxed);
            self.stats.bytes_sent.fetch_add(data.len() as u64, Ordering::Relaxed);
            
            // Wake waiting receiver
            self.wait_queue.wake_one_receiver();
        }
        
        result
    }
    
    /// Send message (blocking)
    pub fn send(&self, data: &[u8]) -> MemoryResult<()> {
        self.check_can_send()?;
        
        // Try non-blocking first
        if self.try_send(data).is_ok() {
            return Ok(());
        }
        
        // Check if non-blocking mode
        if self.flags().contains(EndpointFlags::NONBLOCK) {
            return Err(MemoryError::WouldBlock);
        }
        
        // Blocking wait
        self.stats.send_blocks.fetch_add(1, Ordering::Relaxed);
        
        loop {
            let wait = BlockingWait::new(&self.wait_queue, true);
            
            match wait.wait_with_spin(100) {
                WakeReason::Ready => {
                    if self.try_send(data).is_ok() {
                        return Ok(());
                    }
                    // Spurious wake, retry
                }
                WakeReason::Closed => return Err(MemoryError::NotFound),
                WakeReason::Interrupted => return Err(MemoryError::Interrupted),
                WakeReason::Timeout => return Err(MemoryError::Timeout),
                WakeReason::Spurious => continue,
            }
        }
    }
    
    /// Send with timeout (microseconds)
    pub fn send_timeout(&self, data: &[u8], _timeout_us: u64) -> MemoryResult<()> {
        self.check_can_send()?;
        
        // Try non-blocking first
        if self.try_send(data).is_ok() {
            return Ok(());
        }
        
        // TODO: Implement proper timeout with timer integration
        // For now, just do a limited spin
        for _ in 0..1000 {
            core::hint::spin_loop();
            if self.try_send(data).is_ok() {
                return Ok(());
            }
        }
        
        Err(MemoryError::Timeout)
    }
    
    // =========================================================================
    // RECEIVE OPERATIONS
    // =========================================================================
    
    /// Receive message (non-blocking)
    #[inline]
    pub fn try_recv(&self, buffer: &mut [u8]) -> MemoryResult<usize> {
        self.check_can_recv()?;
        
        let result = self.ring.try_recv(buffer);
        
        if let Ok(size) = &result {
            self.stats.messages_recv.fetch_add(1, Ordering::Relaxed);
            self.stats.bytes_recv.fetch_add(*size as u64, Ordering::Relaxed);
            
            // Wake waiting sender
            self.wait_queue.wake_one_sender();
        }
        
        result
    }
    
    /// Receive message (blocking)
    pub fn recv(&self, buffer: &mut [u8]) -> MemoryResult<usize> {
        self.check_can_recv()?;
        
        // Try non-blocking first
        if let Ok(size) = self.try_recv(buffer) {
            return Ok(size);
        }
        
        // Check if non-blocking mode
        if self.flags().contains(EndpointFlags::NONBLOCK) {
            return Err(MemoryError::WouldBlock);
        }
        
        // Blocking wait
        self.stats.recv_blocks.fetch_add(1, Ordering::Relaxed);
        
        loop {
            let wait = BlockingWait::new(&self.wait_queue, false);
            
            match wait.wait_with_spin(100) {
                WakeReason::Ready => {
                    if let Ok(size) = self.try_recv(buffer) {
                        return Ok(size);
                    }
                    // Spurious wake, retry
                }
                WakeReason::Closed => return Err(MemoryError::NotFound),
                WakeReason::Interrupted => return Err(MemoryError::Interrupted),
                WakeReason::Timeout => return Err(MemoryError::Timeout),
                WakeReason::Spurious => continue,
            }
        }
    }
    
    /// Receive with timeout
    pub fn recv_timeout(&self, buffer: &mut [u8], _timeout_us: u64) -> MemoryResult<usize> {
        self.check_can_recv()?;
        
        // Try non-blocking first
        if let Ok(size) = self.try_recv(buffer) {
            return Ok(size);
        }
        
        // TODO: Implement proper timeout
        for _ in 0..1000 {
            core::hint::spin_loop();
            if let Ok(size) = self.try_recv(buffer) {
                return Ok(size);
            }
        }
        
        Err(MemoryError::Timeout)
    }
    
    // =========================================================================
    // BATCH OPERATIONS
    // =========================================================================
    
    /// Send batch of messages
    pub fn send_batch(&self, messages: &[&[u8]]) -> MemoryResult<usize> {
        self.check_can_send()?;
        
        let sent = self.ring.send_batch(messages);
        
        if sent > 0 {
            self.stats.messages_sent.fetch_add(sent as u64, Ordering::Relaxed);
            let bytes: usize = messages[..sent].iter().map(|m| m.len()).sum();
            self.stats.bytes_sent.fetch_add(bytes as u64, Ordering::Relaxed);
            
            // Wake receivers
            self.wait_queue.wake_one_receiver();
        }
        
        Ok(sent)
    }
    
    /// Receive batch of messages
    pub fn recv_batch(&self, max_count: usize, buffer: &mut [u8]) -> MemoryResult<Vec<(usize, usize)>> {
        self.check_can_recv()?;
        
        let results = self.ring.recv_batch(max_count, buffer);
        
        if !results.is_empty() {
            self.stats.messages_recv.fetch_add(results.len() as u64, Ordering::Relaxed);
            let bytes: usize = results.iter().map(|(_, size)| size).sum();
            self.stats.bytes_recv.fetch_add(bytes as u64, Ordering::Relaxed);
            
            // Wake senders
            self.wait_queue.wake_one_sender();
        }
        
        Ok(results)
    }
    
    // =========================================================================
    // LIFECYCLE
    // =========================================================================
    
    /// Close endpoint
    pub fn close(&self) {
        self.state.store(EndpointState::Closed as u32, Ordering::Release);
        self.wait_queue.close();
    }
    
    /// Half-close (disable send or recv)
    pub fn shutdown_send(&self) {
        let flags = self.flags.load(Ordering::Relaxed);
        self.flags.store(flags & !EndpointFlags::SEND.0, Ordering::Release);
        
        if flags & EndpointFlags::RECV.0 == 0 {
            self.close();
        } else {
            self.state.store(EndpointState::HalfClosed as u32, Ordering::Release);
        }
    }
    
    pub fn shutdown_recv(&self) {
        let flags = self.flags.load(Ordering::Relaxed);
        self.flags.store(flags & !EndpointFlags::RECV.0, Ordering::Release);
        
        if flags & EndpointFlags::SEND.0 == 0 {
            self.close();
        } else {
            self.state.store(EndpointState::HalfClosed as u32, Ordering::Release);
        }
    }
    
    /// Suspend endpoint temporarily
    pub fn suspend(&self) {
        self.state.store(EndpointState::Suspended as u32, Ordering::Release);
    }
    
    /// Resume suspended endpoint
    pub fn resume(&self) {
        let state = self.state.load(Ordering::Acquire);
        if state == EndpointState::Suspended as u32 {
            self.state.store(EndpointState::Active as u32, Ordering::Release);
            // Wake any waiters
            self.wait_queue.wake_one_sender();
            self.wait_queue.wake_one_receiver();
        }
    }
    
    // =========================================================================
    // HELPERS
    // =========================================================================
    
    fn check_can_send(&self) -> MemoryResult<()> {
        if !self.is_active() {
            return Err(MemoryError::NotFound);
        }
        if !self.flags().can_send() {
            return Err(MemoryError::PermissionDenied);
        }
        Ok(())
    }
    
    fn check_can_recv(&self) -> MemoryResult<()> {
        if !self.is_active() {
            return Err(MemoryError::NotFound);
        }
        if !self.flags().can_recv() {
            return Err(MemoryError::PermissionDenied);
        }
        Ok(())
    }
    
    /// Get statistics
    pub fn stats(&self) -> &EndpointStats {
        &self.stats
    }
    
    /// Check if ring has data
    #[inline]
    pub fn readable(&self) -> bool {
        !self.ring.is_empty()
    }
    
    /// Check if ring has space
    #[inline]
    pub fn writable(&self) -> bool {
        !self.ring.is_full()
    }
    
    /// Get ring length
    #[inline]
    pub fn pending(&self) -> usize {
        self.ring.len()
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.close();
    }
}

// =============================================================================
// ENDPOINT PAIR CREATION
// =============================================================================

/// Create a pair of connected endpoints
pub fn create_endpoint_pair(capacity: usize) -> (Endpoint, Endpoint) {
    let ring = Arc::new(MpmcRing::new(capacity));
    let wait_queue = Arc::new(WaitQueue::new());
    let channel = ChannelHandle(crate::ipc::alloc_channel_id());
    
    let send_ep = Endpoint::new(
        channel,
        ring.clone(),
        wait_queue.clone(),
        EndpointFlags::SEND,
    );
    
    let recv_ep = Endpoint::new(
        channel,
        ring,
        wait_queue,
        EndpointFlags::RECV,
    );
    
    // Link peers
    *send_ep.peer.write() = Some(recv_ep.id());
    *recv_ep.peer.write() = Some(send_ep.id());
    
    (send_ep, recv_ep)
}

/// Create bidirectional endpoint pair
pub fn create_bidirectional_pair(capacity: usize) -> (Endpoint, Endpoint) {
    let ring = Arc::new(MpmcRing::new(capacity));
    let wait_queue = Arc::new(WaitQueue::new());
    let channel = ChannelHandle(crate::ipc::alloc_channel_id());
    
    let ep1 = Endpoint::new(
        channel,
        ring.clone(),
        wait_queue.clone(),
        EndpointFlags::BIDIRECTIONAL,
    );
    
    let ep2 = Endpoint::new(
        channel,
        ring,
        wait_queue,
        EndpointFlags::BIDIRECTIONAL,
    );
    
    *ep1.peer.write() = Some(ep2.id());
    *ep2.peer.write() = Some(ep1.id());
    
    (ep1, ep2)
}
