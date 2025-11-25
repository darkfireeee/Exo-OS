//! Predictive Scheduler - 3-Queue EMA System
//!
//! Uses Exponential Moving Average to predict thread behavior
//! Three queues: Interactive, Batch, System

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::collections::VecDeque;
use crate::scheduler::thread::Thread;
use spin::Mutex;

/// Queue types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueType {
    Interactive,  // High priority, short quantum
    Batch,        // Medium priority, longer quantum
    System,       // Highest priority, kernel threads
}

/// EMA prediction parameters
const EMA_ALPHA: f32 = 0.5;  // Smoothing factor (0.0-1.0)
const INTERACTIVE_THRESHOLD: u64 = 10_000;  // 10us threshold
const BATCH_THRESHOLD: u64 = 100_000;       // 100us threshold

/// Thread statistics for prediction
pub struct ThreadStats {
    /// Average execution time (EMA)
    pub avg_exec_time: f32,
    
    /// Average wait time (EMA)
    pub avg_wait_time: f32,
    
    /// Total context switches
    pub context_switches: u64,
    
    /// Last execution time (cycles)
    pub last_exec_time: u64,
    
    /// Last scheduled timestamp
    pub last_scheduled: u64,
}

impl ThreadStats {
    pub fn new() -> Self {
        Self {
            avg_exec_time: 0.0,
            avg_wait_time: 0.0,
            context_switches: 0,
            last_exec_time: 0,
            last_scheduled: 0,
        }
    }
    
    /// Update EMA with new execution time
    pub fn update_exec_time(&mut self, cycles: u64) {
        let new_time = cycles as f32;
        self.avg_exec_time = EMA_ALPHA * new_time + (1.0 - EMA_ALPHA) * self.avg_exec_time;
        self.last_exec_time = cycles;
        self.context_switches += 1;
    }
    
    /// Update EMA with new wait time
    pub fn update_wait_time(&mut self, cycles: u64) {
        let new_time = cycles as f32;
        self.avg_wait_time = EMA_ALPHA * new_time + (1.0 - EMA_ALPHA) * self.avg_wait_time;
    }
    
    /// Predict queue type based on EMA
    pub fn predict_queue(&self) -> QueueType {
        if self.avg_exec_time < INTERACTIVE_THRESHOLD as f32 {
            QueueType::Interactive
        } else if self.avg_exec_time < BATCH_THRESHOLD as f32 {
            QueueType::Batch
        } else {
            QueueType::Batch
        }
    }
}

/// Priority queue
pub struct PriorityQueue {
    queue_type: QueueType,
    threads: VecDeque<usize>,  // Thread IDs
    quantum: u64,              // Time quantum (cycles)
}

impl PriorityQueue {
    pub fn new(queue_type: QueueType) -> Self {
        let quantum = match queue_type {
            QueueType::Interactive => 1_000_000,   // 1ms
            QueueType::Batch => 10_000_000,        // 10ms
            QueueType::System => 500_000,          // 0.5ms
        };
        
        Self {
            queue_type,
            threads: VecDeque::new(),
            quantum,
        }
    }
    
    pub fn enqueue(&mut self, thread_id: usize) {
        self.threads.push_back(thread_id);
    }
    
    pub fn dequeue(&mut self) -> Option<usize> {
        self.threads.pop_front()
    }
    
    pub fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }
    
    pub fn len(&self) -> usize {
        self.threads.len()
    }
    
    pub fn quantum(&self) -> u64 {
        self.quantum
    }
}

/// Predictive scheduler
pub struct PredictiveScheduler {
    /// System queue (highest priority)
    system_queue: Mutex<PriorityQueue>,
    
    /// Interactive queue (high priority)
    interactive_queue: Mutex<PriorityQueue>,
    
    /// Batch queue (normal priority)
    batch_queue: Mutex<PriorityQueue>,
    
    /// Total scheduled threads
    total_scheduled: AtomicU64,
}

impl PredictiveScheduler {
    pub fn new() -> Self {
        Self {
            system_queue: Mutex::new(PriorityQueue::new(QueueType::System)),
            interactive_queue: Mutex::new(PriorityQueue::new(QueueType::Interactive)),
            batch_queue: Mutex::new(PriorityQueue::new(QueueType::Batch)),
            total_scheduled: AtomicU64::new(0),
        }
    }
    
    /// Add thread to appropriate queue
    pub fn enqueue(&self, thread_id: usize, queue_type: QueueType) {
        match queue_type {
            QueueType::System => {
                self.system_queue.lock().enqueue(thread_id);
            }
            QueueType::Interactive => {
                self.interactive_queue.lock().enqueue(thread_id);
            }
            QueueType::Batch => {
                self.batch_queue.lock().enqueue(thread_id);
            }
        }
    }
    
    /// Pick next thread to run (<100 cycles target)
    pub fn pick_next(&self) -> Option<(usize, u64)> {
        // Try system queue first
        if let Some(tid) = self.system_queue.lock().dequeue() {
            let quantum = QueueType::System as u64;
            self.total_scheduled.fetch_add(1, Ordering::Relaxed);
            return Some((tid, quantum));
        }
        
        // Try interactive queue
        if let Some(tid) = self.interactive_queue.lock().dequeue() {
            let quantum = 1_000_000;
            self.total_scheduled.fetch_add(1, Ordering::Relaxed);
            return Some((tid, quantum));
        }
        
        // Try batch queue
        if let Some(tid) = self.batch_queue.lock().dequeue() {
            let quantum = 10_000_000;
            self.total_scheduled.fetch_add(1, Ordering::Relaxed);
            return Some((tid, quantum));
        }
        
        None
    }
    
    /// Get total scheduled count
    pub fn total_scheduled(&self) -> u64 {
        self.total_scheduled.load(Ordering::Relaxed)
    }
}
