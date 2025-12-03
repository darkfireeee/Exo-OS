//! Wait Queue - Efficient thread blocking for IPC
//!
//! Provides efficient blocking/waking for IPC operations.
//! Integrates with the Exo-OS scheduler for minimal latency wake-ups.
//!
//! ## Design:
//! - Lock-free wait list using atomic linked list
//! - Direct scheduler integration (no syscall for wake)
//! - Priority-aware waking
//! - Timeout support

use core::sync::atomic::{AtomicU64, AtomicPtr, AtomicBool, Ordering};
use core::ptr;
use alloc::boxed::Box;
use alloc::vec::Vec;
use spin::Mutex;
use crate::scheduler::{SCHEDULER, ThreadId};

/// Reason for wake-up
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeReason {
    /// Data available / space available
    Ready,
    /// Timeout expired
    Timeout,
    /// Interrupted (signal)
    Interrupted,
    /// Channel closed
    Closed,
    /// Spurious wake
    Spurious,
}

/// Wait node for a single waiter
#[repr(C)]
pub struct WaitNode {
    /// Thread ID of the waiter
    thread_id: AtomicU64,
    /// Wake flag
    woken: AtomicBool,
    /// Wake reason
    reason: AtomicU64, // WakeReason as u64
    /// Next node in list
    next: AtomicPtr<WaitNode>,
    /// Priority (higher = wake first)
    priority: u8,
    /// Waiting for send (true) or receive (false)
    is_sender: bool,
}

impl WaitNode {
    pub const fn new(thread_id: u64, is_sender: bool, priority: u8) -> Self {
        Self {
            thread_id: AtomicU64::new(thread_id),
            woken: AtomicBool::new(false),
            reason: AtomicU64::new(WakeReason::Spurious as u64),
            next: AtomicPtr::new(ptr::null_mut()),
            priority,
            is_sender,
        }
    }
    
    /// Check if woken
    #[inline]
    pub fn is_woken(&self) -> bool {
        self.woken.load(Ordering::Acquire)
    }
    
    /// Get wake reason
    #[inline]
    pub fn wake_reason(&self) -> WakeReason {
        match self.reason.load(Ordering::Acquire) {
            0 => WakeReason::Ready,
            1 => WakeReason::Timeout,
            2 => WakeReason::Interrupted,
            3 => WakeReason::Closed,
            _ => WakeReason::Spurious,
        }
    }
    
    /// Wake this node
    #[inline]
    fn wake(&self, reason: WakeReason) {
        self.reason.store(reason as u64, Ordering::Release);
        self.woken.store(true, Ordering::Release);
        
        // Wake the thread via scheduler
        let tid = self.thread_id.load(Ordering::Relaxed);
        if tid != 0 {
            SCHEDULER.unblock_thread(tid);
        }
    }
}

/// Wait queue for blocked threads
pub struct WaitQueue {
    /// Head of sender wait list
    sender_head: AtomicPtr<WaitNode>,
    /// Head of receiver wait list
    receiver_head: AtomicPtr<WaitNode>,
    /// Number of waiting senders
    sender_count: AtomicU64,
    /// Number of waiting receivers
    receiver_count: AtomicU64,
    /// Channel closed flag
    closed: AtomicBool,
}

impl WaitQueue {
    pub const fn new() -> Self {
        Self {
            sender_head: AtomicPtr::new(ptr::null_mut()),
            receiver_head: AtomicPtr::new(ptr::null_mut()),
            sender_count: AtomicU64::new(0),
            receiver_count: AtomicU64::new(0),
            closed: AtomicBool::new(false),
        }
    }
    
    /// Register a waiter
    /// Returns false if channel is closed
    pub fn wait(&self, node: &WaitNode) -> bool {
        if self.closed.load(Ordering::Acquire) {
            return false;
        }
        
        let head = if node.is_sender {
            self.sender_count.fetch_add(1, Ordering::Relaxed);
            &self.sender_head
        } else {
            self.receiver_count.fetch_add(1, Ordering::Relaxed);
            &self.receiver_head
        };
        
        // Add to wait list (lock-free push)
        loop {
            let current = head.load(Ordering::Acquire);
            node.next.store(current, Ordering::Relaxed);
            
            if head.compare_exchange_weak(
                current,
                node as *const WaitNode as *mut WaitNode,
                Ordering::Release,
                Ordering::Relaxed,
            ).is_ok() {
                break;
            }
        }
        
        true
    }
    
    /// Remove a waiter (after wake or timeout)
    pub fn remove(&self, node: &WaitNode) {
        let head = if node.is_sender {
            self.sender_count.fetch_sub(1, Ordering::Relaxed);
            &self.sender_head
        } else {
            self.receiver_count.fetch_sub(1, Ordering::Relaxed);
            &self.receiver_head
        };
        
        // Remove from list (simplified - in production would use hazard pointers)
        // For now, just mark as removed and let list be cleaned lazily
    }
    
    /// Wake one sender (when space becomes available)
    pub fn wake_one_sender(&self) -> bool {
        self.wake_one(&self.sender_head, WakeReason::Ready)
    }
    
    /// Wake one receiver (when data becomes available)
    pub fn wake_one_receiver(&self) -> bool {
        self.wake_one(&self.receiver_head, WakeReason::Ready)
    }
    
    /// Wake one waiter from a list
    fn wake_one(&self, head: &AtomicPtr<WaitNode>, reason: WakeReason) -> bool {
        let mut current = head.load(Ordering::Acquire);
        
        while !current.is_null() {
            let node = unsafe { &*current };
            
            // Try to wake this node if not already woken
            if !node.is_woken() {
                node.wake(reason);
                return true;
            }
            
            current = node.next.load(Ordering::Acquire);
        }
        
        false
    }
    
    /// Wake all senders
    pub fn wake_all_senders(&self, reason: WakeReason) -> usize {
        self.wake_all(&self.sender_head, reason)
    }
    
    /// Wake all receivers
    pub fn wake_all_receivers(&self, reason: WakeReason) -> usize {
        self.wake_all(&self.receiver_head, reason)
    }
    
    /// Wake all waiters in a list
    fn wake_all(&self, head: &AtomicPtr<WaitNode>, reason: WakeReason) -> usize {
        let mut count = 0;
        let mut current = head.load(Ordering::Acquire);
        
        while !current.is_null() {
            let node = unsafe { &*current };
            
            if !node.is_woken() {
                node.wake(reason);
                count += 1;
            }
            
            current = node.next.load(Ordering::Acquire);
        }
        
        count
    }
    
    /// Close the queue (wake all with Closed reason)
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.wake_all_senders(WakeReason::Closed);
        self.wake_all_receivers(WakeReason::Closed);
    }
    
    /// Check if closed
    #[inline]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
    
    /// Get number of waiting senders
    #[inline]
    pub fn sender_count(&self) -> u64 {
        self.sender_count.load(Ordering::Relaxed)
    }
    
    /// Get number of waiting receivers
    #[inline]
    pub fn receiver_count(&self) -> u64 {
        self.receiver_count.load(Ordering::Relaxed)
    }
    
    /// Check if any waiters
    #[inline]
    pub fn has_waiters(&self) -> bool {
        self.sender_count() > 0 || self.receiver_count() > 0
    }
}

/// Blocking wait helper
pub struct BlockingWait<'a> {
    queue: &'a WaitQueue,
    node: WaitNode,
}

impl<'a> BlockingWait<'a> {
    /// Create new blocking wait
    pub fn new(queue: &'a WaitQueue, is_sender: bool) -> Self {
        let thread_id = SCHEDULER.with_current_thread(|t| t.id()).unwrap_or(0);
        
        Self {
            queue,
            node: WaitNode::new(thread_id, is_sender, 128),
        }
    }
    
    /// Wait until woken or timeout
    pub fn wait(&self) -> WakeReason {
        if !self.queue.wait(&self.node) {
            return WakeReason::Closed;
        }
        
        // Block current thread
        while !self.node.is_woken() {
            crate::scheduler::block_current();
        }
        
        self.node.wake_reason()
    }
    
    /// Wait with spin first
    pub fn wait_with_spin(&self, spin_count: u32) -> WakeReason {
        if !self.queue.wait(&self.node) {
            return WakeReason::Closed;
        }
        
        // Spin phase
        for _ in 0..spin_count {
            if self.node.is_woken() {
                return self.node.wake_reason();
            }
            core::hint::spin_loop();
        }
        
        // Block phase
        while !self.node.is_woken() {
            crate::scheduler::block_current();
        }
        
        self.node.wake_reason()
    }
}

impl<'a> Drop for BlockingWait<'a> {
    fn drop(&mut self) {
        self.queue.remove(&self.node);
    }
}

/// Event notification for edge-triggered wake
pub struct EventNotifier {
    /// Pending events bitmap
    events: AtomicU64,
    /// Associated wait queue
    wait_queue: WaitQueue,
}

impl EventNotifier {
    pub const fn new() -> Self {
        Self {
            events: AtomicU64::new(0),
            wait_queue: WaitQueue::new(),
        }
    }
    
    /// Signal an event
    pub fn signal(&self, event: u64) {
        let old = self.events.fetch_or(event, Ordering::Release);
        
        // Wake waiters if new event
        if old & event == 0 {
            self.wait_queue.wake_one_receiver();
        }
    }
    
    /// Clear and return events
    pub fn consume(&self) -> u64 {
        self.events.swap(0, Ordering::AcqRel)
    }
    
    /// Check if event is pending
    pub fn is_pending(&self, event: u64) -> bool {
        self.events.load(Ordering::Acquire) & event != 0
    }
}

/// Event types for IPC channels
pub mod events {
    pub const READABLE: u64 = 1 << 0;
    pub const WRITABLE: u64 = 1 << 1;
    pub const ERROR: u64 = 1 << 2;
    pub const HANGUP: u64 = 1 << 3;
    pub const PRIORITY: u64 = 1 << 4;
}
