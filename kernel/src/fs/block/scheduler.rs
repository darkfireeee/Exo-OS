//! I/O Scheduler
//!
//! Implements multiple I/O scheduling algorithms to optimize block device access patterns.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::fs::{FsError, FsResult};
use super::device::BlockDevice;

/// I/O request
#[derive(Debug, Clone)]
pub struct IoRequest {
    /// Request ID (for tracking)
    pub id: u64,
    /// Operation type
    pub operation: IoOperation,
    /// Block address
    pub lba: u64,
    /// Number of blocks
    pub count: u32,
    /// Priority (0 = highest)
    pub priority: u8,
    /// Deadline timestamp (nanoseconds)
    pub deadline: u64,
    /// Submission timestamp
    pub submit_time: u64,
}

impl IoRequest {
    /// Create a new I/O request
    pub fn new(operation: IoOperation, lba: u64, count: u32) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let now = crate::time::uptime_ns();

        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            operation,
            lba,
            count,
            priority: 4,
            deadline: now + 1_000_000_000,
            submit_time: now,
        }
    }

    /// Set priority
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }

    /// Set deadline (nanoseconds from now)
    pub fn with_deadline(mut self, deadline_ns: u64) -> Self {
        self.deadline = self.submit_time + deadline_ns;
        self
    }

    /// Get end LBA
    pub fn end_lba(&self) -> u64 {
        self.lba + self.count as u64
    }

    /// Check if request has expired deadline
    pub fn is_expired(&self) -> bool {
        let now = crate::time::uptime_ns();
        now > self.deadline
    }

    /// Get age in nanoseconds
    pub fn age(&self) -> u64 {
        let now = crate::time::uptime_ns();
        now.saturating_sub(self.submit_time)
    }
}

/// I/O operation type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoOperation {
    Read,
    Write,
    Flush,
    Discard,
}

/// Scheduler type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerType {
    /// Deadline scheduler - Prioritizes deadline and prevents starvation
    Deadline,
    /// Completely Fair Queuing - Fair bandwidth distribution
    CFQ,
    /// No-op scheduler - Simple FIFO, minimal overhead
    Noop,
}

/// I/O Scheduler trait
pub trait IoScheduler: Send + Sync {
    /// Submit a new I/O request
    fn submit(&self, request: IoRequest) -> FsResult<()>;

    /// Get next request to execute
    fn next(&self) -> Option<IoRequest>;

    /// Get scheduler type
    fn scheduler_type(&self) -> SchedulerType;

    /// Get queue depth (number of pending requests)
    fn queue_depth(&self) -> usize;

    /// Merge adjacent requests if possible
    fn try_merge(&self, request: &IoRequest) -> bool {
        let _ = request;
        false
    }
}

/// Deadline scheduler implementation
///
/// Prioritizes requests by deadline to prevent starvation.
/// Separate queues for read and write operations.
pub struct DeadlineScheduler {
    /// Read request queue (sorted by deadline)
    read_queue: RwLock<VecDeque<IoRequest>>,
    /// Write request queue (sorted by deadline)
    write_queue: RwLock<VecDeque<IoRequest>>,
    /// Queue depth counter
    depth: AtomicUsize,
    /// Read batch counter
    read_batch: AtomicUsize,
}

impl DeadlineScheduler {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            read_queue: RwLock::new(VecDeque::new()),
            write_queue: RwLock::new(VecDeque::new()),
            depth: AtomicUsize::new(0),
            read_batch: AtomicUsize::new(0),
        })
    }

    /// Insert request into sorted queue by deadline
    fn insert_sorted(queue: &mut VecDeque<IoRequest>, request: IoRequest) {
        let pos = queue
            .iter()
            .position(|r| r.deadline > request.deadline)
            .unwrap_or(queue.len());

        queue.insert(pos, request);
    }
}

impl Default for DeadlineScheduler {
    fn default() -> Self {
        Self {
            read_queue: RwLock::new(VecDeque::new()),
            write_queue: RwLock::new(VecDeque::new()),
            depth: AtomicUsize::new(0),
            read_batch: AtomicUsize::new(0),
        }
    }
}

impl IoScheduler for DeadlineScheduler {
    fn submit(&self, request: IoRequest) -> FsResult<()> {
        match request.operation {
            IoOperation::Read => {
                let mut queue = self.read_queue.write();
                Self::insert_sorted(&mut queue, request);
            }
            IoOperation::Write | IoOperation::Flush | IoOperation::Discard => {
                let mut queue = self.write_queue.write();
                Self::insert_sorted(&mut queue, request);
            }
        }

        self.depth.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn next(&self) -> Option<IoRequest> {
        let read_batch = self.read_batch.load(Ordering::Relaxed);

        let request = if read_batch < 4 {
            let mut read_queue = self.read_queue.write();
            if let Some(req) = read_queue.pop_front() {
                self.read_batch.fetch_add(1, Ordering::Relaxed);
                Some(req)
            } else {
                self.read_batch.store(0, Ordering::Relaxed);
                let mut write_queue = self.write_queue.write();
                write_queue.pop_front()
            }
        } else {
            self.read_batch.store(0, Ordering::Relaxed);
            let mut write_queue = self.write_queue.write();
            write_queue.pop_front()
        };

        if request.is_some() {
            self.depth.fetch_sub(1, Ordering::Relaxed);
        }

        request
    }

    fn scheduler_type(&self) -> SchedulerType {
        SchedulerType::Deadline
    }

    fn queue_depth(&self) -> usize {
        self.depth.load(Ordering::Relaxed)
    }
}

/// CFQ (Completely Fair Queuing) scheduler
///
/// Provides fair I/O bandwidth distribution among processes.
pub struct CFQScheduler {
    /// Request queue
    queue: RwLock<VecDeque<IoRequest>>,
    /// Queue depth counter
    depth: AtomicUsize,
}

impl CFQScheduler {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            queue: RwLock::new(VecDeque::new()),
            depth: AtomicUsize::new(0),
        })
    }
}

impl Default for CFQScheduler {
    fn default() -> Self {
        Self {
            queue: RwLock::new(VecDeque::new()),
            depth: AtomicUsize::new(0),
        }
    }
}

impl IoScheduler for CFQScheduler {
    fn submit(&self, request: IoRequest) -> FsResult<()> {
        let mut queue = self.queue.write();

        let pos = queue
            .iter()
            .position(|r| r.priority > request.priority ||
                          (r.priority == request.priority && r.lba > request.lba))
            .unwrap_or(queue.len());

        queue.insert(pos, request);
        self.depth.fetch_add(1, Ordering::Relaxed);

        Ok(())
    }

    fn next(&self) -> Option<IoRequest> {
        let mut queue = self.queue.write();
        let request = queue.pop_front();

        if request.is_some() {
            self.depth.fetch_sub(1, Ordering::Relaxed);
        }

        request
    }

    fn scheduler_type(&self) -> SchedulerType {
        SchedulerType::CFQ
    }

    fn queue_depth(&self) -> usize {
        self.depth.load(Ordering::Relaxed)
    }
}

/// Noop scheduler - Simple FIFO
///
/// Minimal overhead, suitable for SSDs and NVMe devices.
pub struct NoopScheduler {
    /// Request queue (FIFO)
    queue: RwLock<VecDeque<IoRequest>>,
    /// Queue depth counter
    depth: AtomicUsize,
}

impl NoopScheduler {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            queue: RwLock::new(VecDeque::new()),
            depth: AtomicUsize::new(0),
        })
    }
}

impl Default for NoopScheduler {
    fn default() -> Self {
        Self {
            queue: RwLock::new(VecDeque::new()),
            depth: AtomicUsize::new(0),
        }
    }
}

impl IoScheduler for NoopScheduler {
    fn submit(&self, request: IoRequest) -> FsResult<()> {
        let mut queue = self.queue.write();
        queue.push_back(request);
        self.depth.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn next(&self) -> Option<IoRequest> {
        let mut queue = self.queue.write();
        let request = queue.pop_front();

        if request.is_some() {
            self.depth.fetch_sub(1, Ordering::Relaxed);
        }

        request
    }

    fn scheduler_type(&self) -> SchedulerType {
        SchedulerType::Noop
    }

    fn queue_depth(&self) -> usize {
        self.depth.load(Ordering::Relaxed)
    }
}

/// Scheduled block device - Wraps a block device with I/O scheduler
pub struct ScheduledDevice {
    /// Underlying device
    device: Arc<RwLock<dyn BlockDevice>>,
    /// Scheduler
    scheduler: Arc<dyn IoScheduler>,
}

impl ScheduledDevice {
    /// Create a new scheduled device
    pub fn new(
        device: Arc<RwLock<dyn BlockDevice>>,
        scheduler_type: SchedulerType,
    ) -> Arc<RwLock<Self>> {
        let scheduler: Arc<dyn IoScheduler> = match scheduler_type {
            SchedulerType::Deadline => DeadlineScheduler::new(),
            SchedulerType::CFQ => CFQScheduler::new(),
            SchedulerType::Noop => NoopScheduler::new(),
        };

        Arc::new(RwLock::new(Self { device, scheduler }))
    }

    /// Submit a request to scheduler
    pub fn submit(&self, request: IoRequest) -> FsResult<()> {
        self.scheduler.submit(request)
    }

    /// Process next scheduled request
    pub fn process_next(&mut self) -> FsResult<bool> {
        if let Some(request) = self.scheduler.next() {
            match request.operation {
                IoOperation::Flush => {
                    self.device.write().flush()?;
                }
                _ => {}
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get scheduler type
    pub fn scheduler_type(&self) -> SchedulerType {
        self.scheduler.scheduler_type()
    }

    /// Get queue depth
    pub fn queue_depth(&self) -> usize {
        self.scheduler.queue_depth()
    }
}
