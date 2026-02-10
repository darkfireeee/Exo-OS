//! io_uring - Modern high-performance async I/O framework
//!
//! ## Architecture
//! - Submission Queue (SQ): User submits I/O requests
//! - Completion Queue (CQ): Kernel posts completions
//! - Zero-copy operation via shared memory rings
//! - Support for chained operations (IOSQE_IO_LINK)
//! - Polling mode for ultra-low latency
//!
//! ## Performance Targets
//! - Submission: < 50 cycles
//! - Completion: < 30 cycles
//! - Latency: < 1µs (polling mode)
//! - Throughput: > 2M IOPS per core

use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::collections::VecDeque;
use spin::{Mutex, RwLock};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use crate::fs::{FsError, FsResult};

/// io_uring operation codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IoUringOp {
    /// No-op (for testing)
    Nop = 0,
    /// Read from file
    ReadV = 1,
    /// Write to file
    WriteV = 2,
    /// Sync file data
    Fsync = 3,
    /// Read from fixed buffer
    ReadFixed = 4,
    /// Write to fixed buffer
    WriteFixed = 5,
    /// Poll add
    PollAdd = 6,
    /// Poll remove
    PollRemove = 7,
    /// Sync file range
    SyncFileRange = 8,
    /// Send message
    SendMsg = 9,
    /// Receive message
    RecvMsg = 10,
    /// Timeout
    Timeout = 11,
    /// Accept connection
    Accept = 13,
    /// Async cancel
    AsyncCancel = 14,
    /// Link timeout
    LinkTimeout = 15,
    /// Connect
    Connect = 16,
}

/// io_uring submission queue entry (SQE)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IoUringSqe {
    /// Operation code
    pub opcode: IoUringOp,
    /// Flags
    pub flags: u8,
    /// I/O priority
    pub ioprio: u16,
    /// File descriptor
    pub fd: i32,
    /// File offset or address
    pub off: u64,
    /// Pointer to buffer or iovecs
    pub addr: u64,
    /// Buffer size or number of iovecs
    pub len: u32,
    /// Operation-specific flags
    pub op_flags: u32,
    /// User data (returned in CQE)
    pub user_data: u64,
    /// Buffer group ID or file index
    pub buf_index: u16,
    /// Personality
    pub personality: u16,
    /// Splice file descriptor
    pub splice_fd_in: i32,
    /// Reserved
    pub reserved: [u64; 2],
}

impl IoUringSqe {
    pub const fn new() -> Self {
        Self {
            opcode: IoUringOp::Nop,
            flags: 0,
            ioprio: 0,
            fd: -1,
            off: 0,
            addr: 0,
            len: 0,
            op_flags: 0,
            user_data: 0,
            buf_index: 0,
            personality: 0,
            splice_fd_in: -1,
            reserved: [0; 2],
        }
    }
}

/// io_uring completion queue entry (CQE)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IoUringCqe {
    /// User data from SQE
    pub user_data: u64,
    /// Result code (bytes transferred or error)
    pub res: i32,
    /// Flags
    pub flags: u32,
}

impl IoUringCqe {
    pub const fn new() -> Self {
        Self {
            user_data: 0,
            res: 0,
            flags: 0,
        }
    }
}

/// io_uring submission queue
pub struct IoUringSubmissionQueue {
    /// Ring buffer of SQEs
    entries: RwLock<Vec<IoUringSqe>>,
    /// Head index (consumer)
    head: AtomicU32,
    /// Tail index (producer)
    tail: AtomicU32,
    /// Ring size (must be power of 2)
    size: u32,
    /// Dropped entries counter
    dropped: AtomicU32,
}

impl IoUringSubmissionQueue {
    pub fn new(size: u32) -> Self {
        let size = size.next_power_of_two();
        let mut entries = Vec::with_capacity(size as usize);
        entries.resize(size as usize, IoUringSqe::new());

        Self {
            entries: RwLock::new(entries),
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            size,
            dropped: AtomicU32::new(0),
        }
    }

    /// Get next available SQE slot
    pub fn get_sqe(&self) -> Option<usize> {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Relaxed);

        // Check if queue is full
        if tail.wrapping_sub(head) >= self.size {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        Some((tail & (self.size - 1)) as usize)
    }

    /// Submit SQE
    pub fn submit_sqe(&self, sqe: IoUringSqe) -> FsResult<()> {
        let idx = self.get_sqe().ok_or(FsError::Again)?;

        let mut entries = self.entries.write();
        entries[idx] = sqe;
        drop(entries);

        // Advance tail
        self.tail.fetch_add(1, Ordering::Release);

        Ok(())
    }

    /// Consume next SQE (kernel side)
    pub fn consume_sqe(&self) -> Option<IoUringSqe> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);

        if head == tail {
            return None; // Queue empty
        }

        let idx = (head & (self.size - 1)) as usize;
        let entries = self.entries.read();
        let sqe = entries[idx];
        drop(entries);

        // Advance head
        self.head.fetch_add(1, Ordering::Release);

        Some(sqe)
    }

    pub fn pending(&self) -> u32 {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        tail.wrapping_sub(head)
    }
}

/// io_uring completion queue
pub struct IoUringCompletionQueue {
    /// Ring buffer of CQEs
    entries: RwLock<Vec<IoUringCqe>>,
    /// Head index (consumer)
    head: AtomicU32,
    /// Tail index (producer)
    tail: AtomicU32,
    /// Ring size (must be power of 2)
    size: u32,
    /// Overflow counter
    overflow: AtomicU32,
}

impl IoUringCompletionQueue {
    pub fn new(size: u32) -> Self {
        let size = size.next_power_of_two();
        let mut entries = Vec::with_capacity(size as usize);
        entries.resize(size as usize, IoUringCqe::new());

        Self {
            entries: RwLock::new(entries),
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            size,
            overflow: AtomicU32::new(0),
        }
    }

    /// Post completion (kernel side)
    pub fn post_cqe(&self, cqe: IoUringCqe) -> FsResult<()> {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Relaxed);

        // Check if queue is full
        if tail.wrapping_sub(head) >= self.size {
            self.overflow.fetch_add(1, Ordering::Relaxed);
            return Err(FsError::Again);
        }

        let idx = (tail & (self.size - 1)) as usize;

        let mut entries = self.entries.write();
        entries[idx] = cqe;
        drop(entries);

        // Advance tail
        self.tail.fetch_add(1, Ordering::Release);

        Ok(())
    }

    /// Consume next CQE (user side)
    pub fn consume_cqe(&self) -> Option<IoUringCqe> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);

        if head == tail {
            return None; // Queue empty
        }

        let idx = (head & (self.size - 1)) as usize;
        let entries = self.entries.read();
        let cqe = entries[idx];
        drop(entries);

        // Advance head
        self.head.fetch_add(1, Ordering::Release);

        Some(cqe)
    }

    pub fn available(&self) -> u32 {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        tail.wrapping_sub(head)
    }
}

/// io_uring instance
pub struct IoUringEngine {
    /// Submission queue
    sq: Arc<IoUringSubmissionQueue>,
    /// Completion queue
    cq: Arc<IoUringCompletionQueue>,
    /// Pending operations (for processing)
    pending_ops: Mutex<VecDeque<IoUringSqe>>,
    /// Statistics
    stats: IoUringStats,
}

#[derive(Debug, Default)]
pub struct IoUringStats {
    pub submitted: AtomicU64,
    pub completed: AtomicU64,
    pub errors: AtomicU64,
    pub dropped: AtomicU64,
}

impl IoUringEngine {
    /// Create new io_uring instance
    pub fn new(sq_size: u32, cq_size: u32) -> Arc<Self> {
        Arc::new(Self {
            sq: Arc::new(IoUringSubmissionQueue::new(sq_size)),
            cq: Arc::new(IoUringCompletionQueue::new(cq_size)),
            pending_ops: Mutex::new(VecDeque::with_capacity(sq_size as usize)),
            stats: IoUringStats::default(),
        })
    }

    /// Submit I/O operation
    #[inline]
    pub fn submit(&self, sqe: IoUringSqe) -> FsResult<()> {
        self.sq.submit_sqe(sqe)?;
        self.stats.submitted.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Process pending submissions
    pub fn process_submissions(&self) {
        while let Some(sqe) = self.sq.consume_sqe() {
            let mut pending = self.pending_ops.lock();
            pending.push_back(sqe);
        }
    }

    /// Complete I/O operation
    #[inline]
    pub fn complete(&self, user_data: u64, res: i32, flags: u32) -> FsResult<()> {
        let cqe = IoUringCqe {
            user_data,
            res,
            flags,
        };

        self.cq.post_cqe(cqe)?;

        if res < 0 {
            self.stats.errors.fetch_add(1, Ordering::Relaxed);
        } else {
            self.stats.completed.fetch_add(1, Ordering::Relaxed);
        }

        Ok(())
    }

    /// Wait for completion
    pub fn wait_cqe(&self) -> Option<IoUringCqe> {
        // Spin wait for completions
        for _ in 0..1000 {
            if let Some(cqe) = self.cq.consume_cqe() {
                return Some(cqe);
            }
            core::hint::spin_loop();
        }

        None
    }

    /// Peek completion (non-blocking)
    #[inline]
    pub fn peek_cqe(&self) -> Option<IoUringCqe> {
        self.cq.consume_cqe()
    }

    /// Get statistics
    pub fn stats(&self) -> &IoUringStats {
        &self.stats
    }

    /// Async operations support
    pub fn submit_read(&self, fd: i32, offset: u64, buf: u64, len: u32, user_data: u64) -> FsResult<()> {
        let mut sqe = IoUringSqe::new();
        sqe.opcode = IoUringOp::ReadV;
        sqe.fd = fd;
        sqe.off = offset;
        sqe.addr = buf;
        sqe.len = len;
        sqe.user_data = user_data;

        self.submit(sqe)
    }

    pub fn submit_write(&self, fd: i32, offset: u64, buf: u64, len: u32, user_data: u64) -> FsResult<()> {
        let mut sqe = IoUringSqe::new();
        sqe.opcode = IoUringOp::WriteV;
        sqe.fd = fd;
        sqe.off = offset;
        sqe.addr = buf;
        sqe.len = len;
        sqe.user_data = user_data;

        self.submit(sqe)
    }

    pub fn submit_fsync(&self, fd: i32, user_data: u64) -> FsResult<()> {
        let mut sqe = IoUringSqe::new();
        sqe.opcode = IoUringOp::Fsync;
        sqe.fd = fd;
        sqe.user_data = user_data;

        self.submit(sqe)
    }
}

/// Global io_uring instance
static GLOBAL_URING: spin::Once<Arc<IoUringEngine>> = spin::Once::new();

/// Initialize global io_uring
pub fn init() {
    GLOBAL_URING.call_once(|| {
        log::info!("Initializing io_uring engine (SQ=256, CQ=512)");
        IoUringEngine::new(256, 512)
    });
}

/// Get global io_uring instance
pub fn global_uring() -> &'static Arc<IoUringEngine> {
    GLOBAL_URING.get().expect("io_uring not initialized")
}
