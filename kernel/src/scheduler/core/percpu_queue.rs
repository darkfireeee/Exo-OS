//! Per-CPU Run Queues for SMP Scheduling
//!
//! Each CPU has its own run queue to minimize lock contention.
//! The load balancer can steal threads from other CPUs when needed.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use crate::scheduler::thread::Thread;
use crate::arch::x86_64::smp::MAX_CPUS;
use core::sync::atomic::{AtomicU64, AtomicPtr, Ordering};
use spin::Mutex;

/// Per-CPU run queue
pub struct PerCpuQueue {
    /// CPU ID this queue belongs to
    cpu_id: usize,
    /// Ready threads waiting to run
    ready_threads: Mutex<VecDeque<Arc<Thread>>>,
    /// Currently running thread (null if idle)
    current_thread: AtomicPtr<Thread>,
    /// Time spent idle (nanoseconds)
    idle_time_ns: AtomicU64,
    /// Time spent busy (nanoseconds)
    busy_time_ns: AtomicU64,
    /// Total number of context switches
    context_switches: AtomicU64,
}

impl PerCpuQueue {
    /// Create a new per-CPU queue
    pub const fn new(cpu_id: usize) -> Self {
        Self {
            cpu_id,
            ready_threads: Mutex::new(VecDeque::new()),
            current_thread: AtomicPtr::new(core::ptr::null_mut()),
            idle_time_ns: AtomicU64::new(0),
            busy_time_ns: AtomicU64::new(0),
            context_switches: AtomicU64::new(0),
        }
    }
    
    /// Enqueue a thread to the ready queue
    pub fn enqueue(&self, thread: Arc<Thread>) {
        let mut queue = self.ready_threads.lock();
        queue.push_back(thread);
    }
    
    /// Enqueue a thread at the front (for high priority or preempted threads)
    pub fn enqueue_front(&self, thread: Arc<Thread>) {
        let mut queue = self.ready_threads.lock();
        queue.push_front(thread);
    }
    
    /// Dequeue the next thread to run
    pub fn dequeue(&self) -> Option<Arc<Thread>> {
        let mut queue = self.ready_threads.lock();
        queue.pop_front()
    }
    
    /// Get the number of threads in the ready queue
    pub fn len(&self) -> usize {
        let queue = self.ready_threads.lock();
        queue.len()
    }
    
    /// Check if the queue is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    
    /// Steal half of the threads from this queue (for load balancing)
    ///
    /// Returns a vector of stolen threads
    pub fn steal_half(&self) -> alloc::vec::Vec<Arc<Thread>> {
        let mut queue = self.ready_threads.lock();
        let steal_count = queue.len() / 2;
        
        if steal_count == 0 {
            return alloc::vec::Vec::new();
        }
        
        // Drain first half of queue
        queue.drain(..steal_count).collect()
    }
    
    /// Get the current running thread
    pub fn current_thread(&self) -> Option<Arc<Thread>> {
        let ptr = self.current_thread.load(Ordering::Acquire);
        if ptr.is_null() {
            None
        } else {
            // SAFETY: We maintain Arc reference count properly
            unsafe {
                let arc = Arc::from_raw(ptr);
                let cloned = arc.clone();
                core::mem::forget(arc); // Don't drop the Arc
                Some(cloned)
            }
        }
    }
    
    /// Set the current running thread
    pub fn set_current_thread(&self, thread: Option<Arc<Thread>>) {
        let new_ptr = match thread {
            Some(arc) => Arc::into_raw(arc) as *mut Thread,
            None => core::ptr::null_mut(),
        };
        
        let old_ptr = self.current_thread.swap(new_ptr, Ordering::AcqRel);
        
        // Drop the old Arc
        if !old_ptr.is_null() {
            unsafe {
                drop(Arc::from_raw(old_ptr));
            }
        }
    }
    
    /// Record idle time
    pub fn add_idle_time(&self, ns: u64) {
        self.idle_time_ns.fetch_add(ns, Ordering::Relaxed);
    }
    
    /// Record busy time
    pub fn add_busy_time(&self, ns: u64) {
        self.busy_time_ns.fetch_add(ns, Ordering::Relaxed);
    }
    
    /// Increment context switch counter
    pub fn inc_context_switches(&self) {
        self.context_switches.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Get CPU load (0-100%)
    pub fn load_percentage(&self) -> u8 {
        let idle = self.idle_time_ns.load(Ordering::Relaxed);
        let busy = self.busy_time_ns.load(Ordering::Relaxed);
        let total = idle + busy;
        
        if total == 0 {
            return 0;
        }
        
        ((busy * 100) / total).min(100) as u8
    }
    
    /// Get statistics
    pub fn stats(&self) -> PerCpuQueueStats {
        PerCpuQueueStats {
            cpu_id: self.cpu_id,
            queue_length: self.len(),
            idle_time_ns: self.idle_time_ns.load(Ordering::Relaxed),
            busy_time_ns: self.busy_time_ns.load(Ordering::Relaxed),
            context_switches: self.context_switches.load(Ordering::Relaxed),
            load_percentage: self.load_percentage(),
        }
    }
}

/// Per-CPU queue statistics
#[derive(Debug, Clone, Copy)]
pub struct PerCpuQueueStats {
    pub cpu_id: usize,
    pub queue_length: usize,
    pub idle_time_ns: u64,
    pub busy_time_ns: u64,
    pub context_switches: u64,
    pub load_percentage: u8,
}

/// Global array of per-CPU queues
pub static PER_CPU_QUEUES: PerCpuQueues = PerCpuQueues::new();

pub struct PerCpuQueues {
    queues: [PerCpuQueue; MAX_CPUS],
}

impl PerCpuQueues {
    const fn new() -> Self {
        // Create array with dummy init values, CPU IDs are just indices
        const INIT: PerCpuQueue = PerCpuQueue::new(0);
        Self { queues: [INIT; MAX_CPUS] }
    }
    
    /// Get queue for a specific CPU
    pub fn get(&self, cpu_id: usize) -> Option<&PerCpuQueue> {
        if cpu_id < MAX_CPUS {
            Some(&self.queues[cpu_id])
        } else {
            None
        }
    }
    
    /// Get all queues
    pub fn all(&self) -> &[PerCpuQueue; MAX_CPUS] {
        &self.queues
    }
}

/// Initialize per-CPU queues
pub fn init() {
    log::info!("Per-CPU queues initialized for {} CPUs", MAX_CPUS);
}
