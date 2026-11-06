// ipc/channel/streaming.rs — Continuous streaming channel for Exo-OS
//
// Designed for transfers of large amounts of data (video/audio streams,
// DMA buffers). Uses ZeroCopyRing to share physical pages between
// producer and consumer with zero copies.
//
// Architecture:
//   - Internal ZeroCopyRing (512 slots of ZeroCopyRef)
//   - Each ZeroCopyRef references a pre-allocated physical page
//   - Producer: acquires a free buffer, fills it, sends via push()
//   - Consumer: receives via pop(), consumes, releases via release()
//   - Flow control: producer cannot overtake consumer
//     (full ring = backpressure, no drops)
//
// Granularity: configurable from 4 KiB (PAGE_SIZE) to 2 MiB (HUGE_PAGE)

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::ipc::core::constants::SHM_POOL_PAGES;
use crate::ipc::core::types::{alloc_channel_id, alloc_message_id, ChannelId, IpcError, MessageId};
use crate::ipc::ring::zerocopy::{ZeroCopyBuffer, ZeroCopyRef, ZeroCopyRing};
use crate::ipc::stats::counters::{StatEvent, IPC_STATS};
use crate::scheduler::sync::spinlock::SpinLock;

// ---------------------------------------------------------------------------
// Streaming buffer granularity
// ---------------------------------------------------------------------------

/// Granule size of a streaming buffer (bytes)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum StreamGranule {
    /// 4 KiB — standard page
    Page4K = 4096,
    /// 16 KiB — 4 consecutive pages
    Page16K = 16384,
    /// 64 KiB
    Page64K = 65536,
    /// 256 KiB
    Page256K = 262144,
    /// 2 MiB — huge page
    HugePage2M = 2097152,
}

impl StreamGranule {
    pub fn bytes(self) -> usize {
        self as usize
    }
}

// ---------------------------------------------------------------------------
// Pre-allocated streaming buffer pool
// ---------------------------------------------------------------------------

/// Maximum number of buffers in the pool of a streaming channel
pub const STREAM_POOL_SIZE: usize = SHM_POOL_PAGES; // 256

/// Structure of an in-pool streaming buffer.
/// Buffers are not genuinely allocated here (no_std kernel without general
/// allocator) — monotonic increasing physical addresses are simulated.
/// Actual physical memory is assumed to be reserved by the memory manager.
#[repr(C, align(64))]
pub struct StreamBuffer {
    /// Zero-copy reference to physical page
    pub zc: ZeroCopyBuffer,
    /// Current useful size (filled by producer)
    pub data_len: AtomicUsize,
    /// Buffer available in pool (not acquired)
    pub available: AtomicU32,
    _pad: [u8; 28],
}

// SAFETY: ZeroCopyBuffer is Sync via atomic refcount
unsafe impl Sync for StreamBuffer {}
unsafe impl Send for StreamBuffer {}

impl StreamBuffer {
    pub const fn new_uninit() -> Self {
        Self {
            zc: ZeroCopyBuffer::new_uninit(),
            data_len: AtomicUsize::new(0),
            available: AtomicU32::new(1), // available upon creation
            _pad: [0u8; 28],
        }
    }

    /// Initializes buffer with simulated physical address.
    pub fn init(&mut self, phys_addr: u64, capacity: usize) {
        self.zc = ZeroCopyBuffer::init(phys_addr, capacity as u32, 0);
        self.available.store(1, Ordering::Release);
    }

    /// Acquires buffer for producer.
    /// Returns `true` if acquired, `false` if already in use.
    pub fn acquire(&self) -> bool {
        self.available
            .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Releases buffer after consumption.
    pub fn release(&self) {
        self.data_len.store(0, Ordering::Relaxed);
        self.available.store(1, Ordering::Release);
    }

    pub fn is_available(&self) -> bool {
        self.available.load(Ordering::Acquire) != 0
    }
}

// ---------------------------------------------------------------------------
// Streaming channel buffer pool
// ---------------------------------------------------------------------------

/// Static streaming buffer pool
#[repr(C, align(64))]
pub struct StreamPool {
    buffers: [StreamBuffer; STREAM_POOL_SIZE],
    total: usize,
    base_phys: AtomicU64,
}

// SAFETY: access protected by StreamBuffer atomics
unsafe impl Sync for StreamPool {}
unsafe impl Send for StreamPool {}

impl StreamPool {
    pub const fn new() -> Self {
        Self {
            buffers: [const { StreamBuffer::new_uninit() }; STREAM_POOL_SIZE],
            total: 0,
            base_phys: AtomicU64::new(0),
        }
    }

    /// Initializes pool with base physical address.
    /// Each buffer occupies `granule` bytes of contiguous physical space.
    pub fn init(&mut self, base_phys: u64, granule: StreamGranule, count: usize) {
        let count = count.min(STREAM_POOL_SIZE);
        let granule_bytes = granule.bytes();
        let base = base_phys;
        self.base_phys.store(base, Ordering::Relaxed);

        for i in 0..count {
            let phys = base + (i * granule_bytes) as u64;
            self.buffers[i].init(phys, granule_bytes);
        }
        self.total = count;
    }

    /// Allocates a free buffer. Returns its index or `None`.
    pub fn alloc_buffer(&self) -> Option<usize> {
        (0..self.total).find(|&i| self.buffers[i].acquire())
    }

    /// Releases buffer at index `idx`.
    pub fn free_buffer(&self, idx: usize) -> bool {
        if idx < self.total {
            self.buffers[idx].release();
            true
        } else {
            false
        }
    }

    /// Returns a ZeroCopyRef to buffer `idx`.
    pub fn make_ref(&self, idx: usize, data_len: usize) -> Option<ZeroCopyRef> {
        if idx < self.total {
            let buf = &self.buffers[idx];
            // Check that buffer was acquired and data_len does not exceed capacity
            if buf.is_available() || data_len > buf.zc.capacity as usize {
                return None;
            }
            buf.data_len.store(data_len, Ordering::Relaxed);
            Some(buf.zc.to_ref())
        } else {
            None
        }
    }

    pub fn buffer_count(&self) -> usize {
        self.total
    }

    pub fn available_count(&self) -> usize {
        (0..self.total)
            .filter(|&i| self.buffers[i].is_available())
            .count()
    }
}

// ---------------------------------------------------------------------------
// Streaming channel statistics
// ---------------------------------------------------------------------------

#[repr(C, align(64))]
pub struct StreamStats {
    pub pushes_ok: AtomicU64,
    pub pushes_full: AtomicU64,
    pub pops_ok: AtomicU64,
    pub pops_empty: AtomicU64,
    pub bytes_transferred: AtomicU64,
    pub pool_allocs: AtomicU64,
    pub pool_frees: AtomicU64,
    pub backpressure_events: AtomicU64,
    _pad: [u8; 0],
}

impl StreamStats {
    pub const fn new() -> Self {
        Self {
            pushes_ok: AtomicU64::new(0),
            pushes_full: AtomicU64::new(0),
            pops_ok: AtomicU64::new(0),
            pops_empty: AtomicU64::new(0),
            bytes_transferred: AtomicU64::new(0),
            pool_allocs: AtomicU64::new(0),
            pool_frees: AtomicU64::new(0),
            backpressure_events: AtomicU64::new(0),
            _pad: [],
        }
    }

    pub fn snapshot(&self) -> StreamStatsSnapshot {
        StreamStatsSnapshot {
            pushes_ok: self.pushes_ok.load(Ordering::Relaxed),
            pushes_full: self.pushes_full.load(Ordering::Relaxed),
            pops_ok: self.pops_ok.load(Ordering::Relaxed),
            pops_empty: self.pops_empty.load(Ordering::Relaxed),
            bytes_transferred: self.bytes_transferred.load(Ordering::Relaxed),
            pool_allocs: self.pool_allocs.load(Ordering::Relaxed),
            pool_frees: self.pool_frees.load(Ordering::Relaxed),
            backpressure_events: self.backpressure_events.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StreamStatsSnapshot {
    pub pushes_ok: u64,
    pub pushes_full: u64,
    pub pops_ok: u64,
    pub pops_empty: u64,
    pub bytes_transferred: u64,
    pub pool_allocs: u64,
    pub pool_frees: u64,
    pub backpressure_events: u64,
}

// ---------------------------------------------------------------------------
// StreamChannel — main structure
// ---------------------------------------------------------------------------

/// Zero-copy streaming channel based on ZeroCopyRing + buffer pool.
#[repr(C, align(64))]
pub struct StreamChannel {
    pub id: ChannelId,
    /// Zero-copy ring for transfers
    ring: ZeroCopyRing,
    /// Physical buffer pool
    pool: StreamPool,
    /// Local statistics
    pub stats: StreamStats,
    /// Closed channel
    closed: AtomicU32,
    /// Configured granule
    granule: AtomicU32,
    /// In-flight operations counter (anti-UAF on destroy)
    pub active_ops: AtomicU32,
    _pad: [u8; 20],
}

// SAFETY: ZeroCopyRing is Sync, StreamPool is Sync
unsafe impl Sync for StreamChannel {}
unsafe impl Send for StreamChannel {}

impl StreamChannel {
    pub const fn new_uninit() -> Self {
        Self {
            id: ChannelId::DANGLING,
            ring: ZeroCopyRing::new(),
            pool: StreamPool::new(),
            stats: StreamStats::new(),
            closed: AtomicU32::new(0),
            granule: AtomicU32::new(StreamGranule::Page4K as u32),
            active_ops: AtomicU32::new(0),
            _pad: [0u8; 20],
        }
    }

    /// Initializes channel with physical base and granularity.
    pub fn init(&mut self, base_phys: u64, granule: StreamGranule, buffer_count: usize) {
        self.id = alloc_channel_id();
        self.pool.init(base_phys, granule, buffer_count);
        self.granule.store(granule as u32, Ordering::Relaxed);
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) != 0
    }

    pub fn close(&self) {
        self.closed.store(1, Ordering::Release);
    }

    // -----------------------------------------------------------------------
    // Producer side
    // -----------------------------------------------------------------------

    /// Allocates a buffer from pool for producer.
    /// Returns allocated buffer index.
    pub fn alloc_buffer(&self) -> Result<usize, IpcError> {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }
        let idx = self.pool.alloc_buffer().ok_or(IpcError::OutOfResources)?;
        self.stats.pool_allocs.fetch_add(1, Ordering::Relaxed);
        Ok(idx)
    }

    /// Sends buffer `buf_idx` (filled with `data_len` useful bytes).
    pub fn push(&self, buf_idx: usize, data_len: usize) -> Result<MessageId, IpcError> {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }

        let zc_ref = self
            .pool
            .make_ref(buf_idx, data_len)
            .ok_or(IpcError::InvalidHandle)?;

        match self.ring.push(zc_ref) {
            Ok(()) => {
                let mid = alloc_message_id();
                self.stats.pushes_ok.fetch_add(1, Ordering::Relaxed);
                self.stats
                    .bytes_transferred
                    .fetch_add(data_len as u64, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageSent);
                Ok(mid)
            }
            Err(_) => {
                // Backpressure: full ring, release buffer
                self.pool.free_buffer(buf_idx);
                self.stats.pushes_full.fetch_add(1, Ordering::Relaxed);
                self.stats
                    .backpressure_events
                    .fetch_add(1, Ordering::Relaxed);
                Err(IpcError::QueueFull)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Consumer side
    // -----------------------------------------------------------------------

    /// Receives next ZeroCopyRef from ring.
    /// Consumer must call `release_buffer()` after processing.
    pub fn pop(&self) -> Result<ZeroCopyRef, IpcError> {
        if self.is_closed() && self.ring.is_empty() {
            return Err(IpcError::Closed);
        }

        match self.ring.pop() {
            Ok(zc_ref) => {
                self.stats.pops_ok.fetch_add(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageReceived);
                Ok(zc_ref)
            }
            Err(_) => {
                self.stats.pops_empty.fetch_add(1, Ordering::Relaxed);
                Err(IpcError::WouldBlock)
            }
        }
    }

    /// Releases buffer identified by its pool_idx after consumption.
    pub fn release_buffer(&self, pool_idx: usize) -> bool {
        let freed = self.pool.free_buffer(pool_idx);
        if freed {
            self.stats.pool_frees.fetch_add(1, Ordering::Relaxed);
        }
        freed
    }

    // -----------------------------------------------------------------------
    // Utilities
    // -----------------------------------------------------------------------

    pub fn pool_available(&self) -> usize {
        self.pool.available_count()
    }

    pub fn ring_is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    pub fn snapshot_stats(&self) -> StreamStatsSnapshot {
        self.stats.snapshot()
    }
}

// ---------------------------------------------------------------------------
// Global static streaming channel table
// ---------------------------------------------------------------------------

pub const STREAM_CHANNEL_TABLE_SIZE: usize = 64;

struct StreamChannelTable {
    slots: [MaybeUninit<StreamChannel>; STREAM_CHANNEL_TABLE_SIZE],
    used: [bool; STREAM_CHANNEL_TABLE_SIZE],
    count: usize,
}

// SAFETY: access protected by SpinLock
unsafe impl Send for StreamChannelTable {}

impl StreamChannelTable {
    const fn new() -> Self {
        // SAFETY: mem::zeroed() avoids const-eval memory limits for large arrays.
        unsafe { core::mem::zeroed() }
    }

    fn alloc(
        &mut self,
        base_phys: u64,
        granule: StreamGranule,
        buffer_count: usize,
    ) -> Option<usize> {
        for i in 0..STREAM_CHANNEL_TABLE_SIZE {
            if !self.used[i] {
                let mut chan = StreamChannel::new_uninit();
                chan.init(base_phys, granule, buffer_count);
                self.slots[i].write(chan);
                self.used[i] = true;
                self.count += 1;
                return Some(i);
            }
        }
        None
    }

    fn free(&mut self, idx: usize) -> bool {
        if idx < STREAM_CHANNEL_TABLE_SIZE && self.used[idx] {
            // SAFETY: used[idx] guarantees slots[idx] is initialized; used -> false prevents double-drop.
            unsafe { self.slots[idx].assume_init_drop() };
            self.used[idx] = false;
            self.count -= 1;
            true
        } else {
            false
        }
    }

    unsafe fn get(&self, idx: usize) -> Option<&StreamChannel> {
        if idx < STREAM_CHANNEL_TABLE_SIZE && self.used[idx] {
            Some(self.slots[idx].assume_init_ref())
        } else {
            None
        }
    }
}

static STREAM_CHANNEL_TABLE: SpinLock<StreamChannelTable> =
    SpinLock::new(StreamChannelTable::new());

// ---------------------------------------------------------------------------
// High-level public API
// ---------------------------------------------------------------------------

/// Creates a streaming channel with `buffer_count` buffers of size `granule`.
/// `base_phys` = base physical address of pre-allocated memory region.
pub fn stream_channel_create(
    base_phys: u64,
    granule: StreamGranule,
    buffer_count: usize,
) -> Result<usize, IpcError> {
    let mut tbl = STREAM_CHANNEL_TABLE.lock();
    tbl.alloc(base_phys, granule, buffer_count)
        .ok_or(IpcError::OutOfResources)
}

/// Allocates a producer buffer on channel `idx`.
pub fn stream_alloc_buffer(idx: usize) -> Result<usize, IpcError> {
    let tbl = STREAM_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static StreamChannel = unsafe { &*(chan as *const StreamChannel) };
    chan_ref.active_ops.fetch_add(1, Ordering::Acquire);
    drop(tbl);
    let res = chan_ref.alloc_buffer();
    chan_ref.active_ops.fetch_sub(1, Ordering::Release);
    res
}

/// Sends buffer `buf_idx` (with `data_len` useful bytes) on channel `idx`.
pub fn stream_push(idx: usize, buf_idx: usize, data_len: usize) -> Result<MessageId, IpcError> {
    let tbl = STREAM_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static StreamChannel = unsafe { &*(chan as *const StreamChannel) };
    chan_ref.active_ops.fetch_add(1, Ordering::Acquire);
    drop(tbl);
    let res = chan_ref.push(buf_idx, data_len);
    chan_ref.active_ops.fetch_sub(1, Ordering::Release);
    res
}

/// Receives next ZeroCopyRef from channel `idx`.
pub fn stream_pop(idx: usize) -> Result<ZeroCopyRef, IpcError> {
    let tbl = STREAM_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static StreamChannel = unsafe { &*(chan as *const StreamChannel) };
    chan_ref.active_ops.fetch_add(1, Ordering::Acquire);
    drop(tbl);
    let res = chan_ref.pop();
    chan_ref.active_ops.fetch_sub(1, Ordering::Release);
    res
}

/// Releases buffer `pool_idx` after consumption on channel `idx`.
pub fn stream_release_buffer(idx: usize, pool_idx: usize) -> Result<(), IpcError> {
    let tbl = STREAM_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static StreamChannel = unsafe { &*(chan as *const StreamChannel) };
    chan_ref.active_ops.fetch_add(1, Ordering::Acquire);
    drop(tbl);
    let freed = chan_ref.release_buffer(pool_idx);
    chan_ref.active_ops.fetch_sub(1, Ordering::Release);
    if freed {
        Ok(())
    } else {
        Err(IpcError::InvalidHandle)
    }
}

/// Closes and destroys streaming channel `idx`.
pub fn stream_channel_destroy(idx: usize) -> Result<(), IpcError> {
    let tbl = STREAM_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static StreamChannel = unsafe { &*(chan as *const StreamChannel) };
    chan_ref.close();
    drop(tbl);

    let mut spins = 0u32;
    while chan_ref.active_ops.load(Ordering::Acquire) != 0 {
        if spins >= 256 {
            return Err(IpcError::Retry);
        }
        core::hint::spin_loop();
        spins = spins.wrapping_add(1);
        if spins % 32 == 0 {
            unsafe {
                let _ = crate::scheduler::core::switch::cooperative_reschedule();
            }
        }
    }

    let mut tbl = STREAM_CHANNEL_TABLE.lock();
    if !tbl.free(idx) {
        return Err(IpcError::InvalidHandle);
    }
    Ok(())
}
