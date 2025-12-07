//! # QoS Traffic Shaping - Advanced Algorithms
//! 
//! Algorithmes de contrôle de congestion et shaping :
//! - CBQ (Class-Based Queueing)
//! - RED (Random Early Detection)
//! - BLUE (improved RED)
//! - CoDel (Controlled Delay)

use alloc::vec::Vec;
use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use crate::sync::SpinLock;

/// CBQ (Class-Based Queueing) - Hierarchical scheduling
pub struct ClassBasedQueue {
    classes: SpinLock<Vec<TrafficClass>>,
    root_rate: u64, // bits/sec
}

/// Traffic class for CBQ
pub struct TrafficClass {
    pub id: u32,
    pub parent_id: Option<u32>,
    pub rate: u64,      // bits/sec (guaranteed)
    pub ceil: u64,      // bits/sec (maximum)
    pub priority: u32,
    pub quantum: u32,   // bytes per round
    
    // Token bucket
    tokens: AtomicU64,
    last_update: AtomicU64,
    
    // Queue
    queue: VecDeque<Vec<u8>>,
    stats: ClassStats,
}

#[derive(Debug, Clone, Default)]
pub struct ClassStats {
    pub packets: u64,
    pub bytes: u64,
    pub dropped: u64,
    pub overlimits: u64,
}

impl ClassBasedQueue {
    pub fn new(root_rate: u64) -> Self {
        Self {
            classes: SpinLock::new(Vec::new()),
            root_rate,
        }
    }
    
    /// Add traffic class
    pub fn add_class(&self, parent_id: Option<u32>, rate: u64, ceil: u64, priority: u32) -> u32 {
        let mut classes = self.classes.lock();
        let id = classes.len() as u32;
        
        let class = TrafficClass {
            id,
            parent_id,
            rate,
            ceil,
            priority,
            quantum: 1500, // Default MTU
            tokens: AtomicU64::new(rate),
            last_update: AtomicU64::new(current_time_ns()),
            queue: VecDeque::new(),
            stats: ClassStats::default(),
        };
        
        classes.push(class);
        id
    }
    
    /// Enqueue packet
    pub fn enqueue(&self, class_id: u32, packet: Vec<u8>) -> Result<(), QosError> {
        let mut classes = self.classes.lock();
        
        if let Some(class) = classes.get_mut(class_id as usize) {
            class.queue.push_back(packet);
            Ok(())
        } else {
            Err(QosError::ClassNotFound)
        }
    }
    
    /// Dequeue packet (weighted round-robin with borrowing)
    pub fn dequeue(&self) -> Option<Vec<u8>> {
        let mut classes = self.classes.lock();
        
        // Sort by priority
        let mut indices: Vec<_> = (0..classes.len()).collect();
        indices.sort_by_key(|&i| classes[i].priority);
        
        for &idx in &indices {
            let class = &mut classes[idx];
            
            // Update tokens
            let now = current_time_ns();
            let elapsed = now - class.last_update.load(Ordering::Relaxed);
            let new_tokens = (class.rate * elapsed / 1_000_000_000).min(class.ceil);
            class.tokens.store(new_tokens, Ordering::Relaxed);
            class.last_update.store(now, Ordering::Relaxed);
            
            // Check if can dequeue
            if !class.queue.is_empty() {
                let packet_size = class.queue[0].len() as u64;
                
                if class.tokens.load(Ordering::Relaxed) >= packet_size * 8 {
                    // Dequeue
                    let packet = class.queue.pop_front().unwrap();
                    class.tokens.fetch_sub(packet_size * 8, Ordering::Relaxed);
                    class.stats.packets += 1;
                    class.stats.bytes += packet_size;
                    return Some(packet);
                } else {
                    class.stats.overlimits += 1;
                }
            }
        }
        
        None
    }
}

/// RED (Random Early Detection) - Congestion avoidance
pub struct RandomEarlyDetection {
    min_threshold: u32,  // packets
    max_threshold: u32,  // packets
    max_probability: f32,
    
    // Statistics
    avg_queue_size: AtomicU32,
    count: AtomicU64,
    stats: RedStats,
}

#[derive(Debug, Clone, Default)]
pub struct RedStats {
    pub packets: u64,
    pub bytes: u64,
    pub dropped_early: u64,
    pub dropped_forced: u64,
    pub marked: u64, // ECN marked
}

impl RandomEarlyDetection {
    pub fn new(min_threshold: u32, max_threshold: u32, max_probability: f32) -> Self {
        Self {
            min_threshold,
            max_threshold,
            max_probability,
            avg_queue_size: AtomicU32::new(0),
            count: AtomicU64::new(0),
            stats: RedStats::default(),
        }
    }
    
    /// Decide if packet should be dropped
    pub fn should_drop(&self, current_queue_size: u32) -> bool {
        // Update average queue size (EWMA)
        let avg = self.avg_queue_size.load(Ordering::Relaxed);
        let new_avg = ((avg as u64 * 15 + current_queue_size as u64) / 16) as u32;
        self.avg_queue_size.store(new_avg, Ordering::Relaxed);
        
        if new_avg < self.min_threshold {
            // No drop
            self.count.store(0, Ordering::Relaxed);
            false
        } else if new_avg >= self.max_threshold {
            // Forced drop
            self.count.store(0, Ordering::Relaxed);
            true
        } else {
            // Probabilistic drop
            let count = self.count.fetch_add(1, Ordering::Relaxed);
            let pb = self.max_probability * 
                     (new_avg - self.min_threshold) as f32 / 
                     (self.max_threshold - self.min_threshold) as f32;
            
            let drop_prob = pb / (1.0 - count as f32 * pb);
            
            if random_f32() < drop_prob {
                self.count.store(0, Ordering::Relaxed);
                true
            } else {
                false
            }
        }
    }
}

/// BLUE - Improved RED using queue occupancy and packet loss
pub struct BlueQueue {
    target_queue_size: u32,
    freeze_time: u64,  // ns
    increment: f32,
    decrement: f32,
    
    // State
    drop_probability: AtomicU32, // Fixed point (0-65535 = 0.0-1.0)
    last_update: AtomicU64,
    
    stats: BlueStats,
}

#[derive(Debug, Clone, Default)]
pub struct BlueStats {
    pub packets: u64,
    pub bytes: u64,
    pub dropped: u64,
    pub marked: u64,
}

impl BlueQueue {
    pub fn new(target_queue_size: u32) -> Self {
        Self {
            target_queue_size,
            freeze_time: 100_000_000, // 100ms
            increment: 0.01,
            decrement: 0.001,
            drop_probability: AtomicU32::new(0),
            last_update: AtomicU64::new(0),
            stats: BlueStats::default(),
        }
    }
    
    /// Update drop probability based on queue events
    pub fn on_packet_enqueue(&self, queue_size: u32) {
        let now = current_time_ns();
        let last = self.last_update.load(Ordering::Relaxed);
        
        if now - last < self.freeze_time {
            return;
        }
        
        if queue_size > self.target_queue_size {
            // Increase drop probability
            let p = self.drop_probability.load(Ordering::Relaxed);
            let new_p = ((p as f32 / 65535.0 + self.increment) * 65535.0).min(65535.0) as u32;
            self.drop_probability.store(new_p, Ordering::Relaxed);
            self.last_update.store(now, Ordering::Relaxed);
        }
    }
    
    pub fn on_packet_drop(&self) {
        let now = current_time_ns();
        let last = self.last_update.load(Ordering::Relaxed);
        
        if now - last < self.freeze_time {
            return;
        }
        
        // Increase drop probability
        let p = self.drop_probability.load(Ordering::Relaxed);
        let new_p = ((p as f32 / 65535.0 + self.increment) * 65535.0).min(65535.0) as u32;
        self.drop_probability.store(new_p, Ordering::Relaxed);
        self.last_update.store(now, Ordering::Relaxed);
    }
    
    pub fn on_link_idle(&self) {
        let now = current_time_ns();
        let last = self.last_update.load(Ordering::Relaxed);
        
        if now - last < self.freeze_time {
            return;
        }
        
        // Decrease drop probability
        let p = self.drop_probability.load(Ordering::Relaxed);
        let new_p = ((p as f32 / 65535.0 - self.decrement) * 65535.0).max(0.0) as u32;
        self.drop_probability.store(new_p, Ordering::Relaxed);
        self.last_update.store(now, Ordering::Relaxed);
    }
    
    /// Decide if packet should be dropped
    pub fn should_drop(&self) -> bool {
        let p = self.drop_probability.load(Ordering::Relaxed);
        let threshold = (random_u32() >> 16) as u32;
        threshold < p
    }
}

/// CoDel (Controlled Delay) - Modern AQM
pub struct CoDelQueue {
    target: u64,    // target delay (ns)
    interval: u64,  // interval (ns)
    
    // State
    first_above_time: AtomicU64,
    drop_next: AtomicU64,
    count: AtomicU32,
    dropping: core::sync::atomic::AtomicBool,
    
    stats: CoDelStats,
}

#[derive(Debug, Clone, Default)]
pub struct CoDelStats {
    pub packets: u64,
    pub bytes: u64,
    pub dropped: u64,
    pub sojourn_time_avg: u64, // ns
}

impl CoDelQueue {
    pub fn new() -> Self {
        Self {
            target: 5_000_000,   // 5ms
            interval: 100_000_000, // 100ms
            first_above_time: AtomicU64::new(0),
            drop_next: AtomicU64::new(0),
            count: AtomicU32::new(0),
            dropping: core::sync::atomic::AtomicBool::new(false),
            stats: CoDelStats::default(),
        }
    }
    
    /// Process dequeue event
    pub fn dequeue(&self, sojourn_time: u64) -> bool {
        let now = current_time_ns();
        
        if sojourn_time < self.target {
            // Below target, exit dropping state
            self.first_above_time.store(0, Ordering::Relaxed);
            return false;
        }
        
        if self.first_above_time.load(Ordering::Relaxed) == 0 {
            // First time above target
            self.first_above_time.store(now, Ordering::Relaxed);
            return false;
        }
        
        if now - self.first_above_time.load(Ordering::Relaxed) < self.interval {
            // Not yet time to drop
            return false;
        }
        
        // Should drop
        true
    }
}

/// QoS errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QosError {
    ClassNotFound,
    QueueFull,
    InvalidRate,
}

// Helper functions (mock)
fn current_time_ns() -> u64 {
    0
}

fn random_f32() -> f32 {
    0.5
}

fn random_u32() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_cbq() {
        let cbq = ClassBasedQueue::new(1_000_000_000); // 1 Gbps
        
        // Add classes
        let root = cbq.add_class(None, 1_000_000_000, 1_000_000_000, 0);
        let high = cbq.add_class(Some(root), 500_000_000, 800_000_000, 1);
        let low = cbq.add_class(Some(root), 300_000_000, 500_000_000, 2);
        
        assert_eq!(root, 0);
        assert_eq!(high, 1);
        assert_eq!(low, 2);
    }
}
