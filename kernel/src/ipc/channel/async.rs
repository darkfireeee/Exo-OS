// ipc/channel/async.rs — Asynchronous channel (notifications without async runtime) for Exo-OS
//
// This module implements an asynchronous channel without a full async/await runtime.
// Threads can register wake-up callbacks (Waker-like) that will be
// invoked when a message becomes available.
//
// Architecture:
//   - Message queue (SpscRing or MpmcRing depending on mode)
//   - Static waker table (MAX_ASYNC_WAKERS = 32)
//   - Notification via AtomicBool "ready" per registered waker
//   - Wake-up hook into scheduler (optional call if configured)
//   - No std Future/Poll/Waker — no_std kernel
//
// Usage patterns:
//   1. Active polling: `try_recv()` + short spin
//   2. Event-driven: register waker -> get notified -> call `try_recv()`
//   3. Completion callback: register `fn(*mut ()) -> ()` called on reception

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::ipc::core::constants::MAX_MSG_SIZE;
use crate::ipc::core::types::{
    alloc_channel_id, alloc_message_id, ChannelId, IpcError, MessageId, MsgFlags,
};
use crate::ipc::ring::spsc::SpscRing;
use crate::ipc::stats::counters::{StatEvent, IPC_STATS};
use crate::scheduler::sync::spinlock::SpinLock;

// ---------------------------------------------------------------------------
// Simplified kernel waker
// ---------------------------------------------------------------------------

/// Maximum number of registered wakers per asynchronous channel
pub const MAX_ASYNC_WAKERS: usize = 32;

/// Wake-up function pointer type.
/// `data` = opaque pointer passed during registration.
pub type WakeFn = unsafe fn(data: *mut ());

/// Waker registered on an asynchronous channel
#[repr(C, align(32))]
pub struct AsyncWaker {
    /// Wake-up function (None = free waker)
    wake_fn: Option<WakeFn>,
    /// Opaque data passed to wake_fn
    data: *mut (),
    /// This waker was fired but not yet processed
    fired: AtomicBool,
    /// Active waker (registered)
    active: AtomicBool,
    _pad: [u8; 6],
}

// SAFETY: wake_fn and data are defined by registrant who guarantees their validity
// for the entire registration duration. AtomicBool is Sync.
unsafe impl Sync for AsyncWaker {}
unsafe impl Send for AsyncWaker {}

impl AsyncWaker {
    pub const fn new() -> Self {
        Self {
            wake_fn: None,
            data: core::ptr::null_mut(),
            fired: AtomicBool::new(false),
            active: AtomicBool::new(false),
            _pad: [0u8; 6],
        }
    }

    pub fn register(&mut self, f: WakeFn, data: *mut ()) {
        self.wake_fn = Some(f);
        self.data = data;
        self.fired.store(false, Ordering::Relaxed);
        self.active.store(true, Ordering::Release);
    }

    pub fn deactivate(&mut self) {
        self.active.store(false, Ordering::Release);
        self.wake_fn = None;
        self.data = core::ptr::null_mut();
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub fn has_fired(&self) -> bool {
        self.fired.load(Ordering::Acquire)
    }

    /// Fires waker: calls wake_fn(data) if active.
    ///
    /// # SAFETY
    /// Registrant guarantees that `data` remains valid until deregistration.
    pub unsafe fn fire(&self) {
        if self.active.load(Ordering::Acquire) {
            if let Some(f) = self.wake_fn {
                self.fired.store(true, Ordering::Release);
                // SAFETY: guaranteed by registrant contract
                f(self.data);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Waker table
// ---------------------------------------------------------------------------

pub struct WakerTable {
    entries: [AsyncWaker; MAX_ASYNC_WAKERS],
    count: usize,
}

// SAFETY: access protected by SpinLock
unsafe impl Send for WakerTable {}

impl WakerTable {
    pub const fn new() -> Self {
        Self {
            entries: [const { AsyncWaker::new() }; MAX_ASYNC_WAKERS],
            count: 0,
        }
    }

    /// Registers a waker. Returns index or `None` if full.
    pub fn register(&mut self, f: WakeFn, data: *mut ()) -> Option<usize> {
        for i in 0..MAX_ASYNC_WAKERS {
            if !self.entries[i].is_active() {
                self.entries[i].register(f, data);
                self.count += 1;
                return Some(i);
            }
        }
        None
    }

    /// Unregisters waker at index `idx`.
    pub fn unregister(&mut self, idx: usize) -> bool {
        if idx < MAX_ASYNC_WAKERS && self.entries[idx].is_active() {
            self.entries[idx].deactivate();
            self.count -= 1;
            true
        } else {
            false
        }
    }

    /// Fires all active wakers (internal, caller holds lock).
    pub fn fire_all(&self) {
        for i in 0..MAX_ASYNC_WAKERS {
            if self.entries[i].is_active() {
                // SAFETY: registrant contract (data valid until deregistration)
                unsafe { self.entries[i].fire() };
            }
        }
    }

    /// Collects callbacks of active wakers without calling them under lock,
    /// avoiding deadlocks if callback interacts with table.
    pub fn collect_active_wakers(&mut self) -> ([(Option<WakeFn>, *mut ()); MAX_ASYNC_WAKERS], usize) {
        let mut list = [(None, core::ptr::null_mut()); MAX_ASYNC_WAKERS];
        let mut n = 0;
        for i in 0..MAX_ASYNC_WAKERS {
            if self.entries[i].is_active() {
                self.entries[i].fired.store(true, Ordering::Release);
                list[n] = (self.entries[i].wake_fn, self.entries[i].data);
                n += 1;
            }
        }
        (list, n)
    }

    pub fn count(&self) -> usize {
        self.count
    }
}

// ---------------------------------------------------------------------------
// Asynchronous channel statistics
// ---------------------------------------------------------------------------

#[repr(C, align(64))]
pub struct AsyncChannelStats {
    pub sends_ok: AtomicU64,
    pub sends_full: AtomicU64,
    pub recvs_ok: AtomicU64,
    pub recvs_empty: AtomicU64,
    pub waker_fires: AtomicU64,
    pub bytes_transferred: AtomicU64,
    _pad: [u8; 16],
}

impl AsyncChannelStats {
    pub const fn new() -> Self {
        Self {
            sends_ok: AtomicU64::new(0),
            sends_full: AtomicU64::new(0),
            recvs_ok: AtomicU64::new(0),
            recvs_empty: AtomicU64::new(0),
            waker_fires: AtomicU64::new(0),
            bytes_transferred: AtomicU64::new(0),
            _pad: [0u8; 16],
        }
    }

    pub fn snapshot(&self) -> AsyncChannelStatsSnapshot {
        AsyncChannelStatsSnapshot {
            sends_ok: self.sends_ok.load(Ordering::Relaxed),
            sends_full: self.sends_full.load(Ordering::Relaxed),
            recvs_ok: self.recvs_ok.load(Ordering::Relaxed),
            recvs_empty: self.recvs_empty.load(Ordering::Relaxed),
            waker_fires: self.waker_fires.load(Ordering::Relaxed),
            bytes_transferred: self.bytes_transferred.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AsyncChannelStatsSnapshot {
    pub sends_ok: u64,
    pub sends_full: u64,
    pub recvs_ok: u64,
    pub recvs_empty: u64,
    pub waker_fires: u64,
    pub bytes_transferred: u64,
}

// ---------------------------------------------------------------------------
// AsyncChannel — main structure
// ---------------------------------------------------------------------------

/// Asynchronous channel with waker notification.
///
/// Mechanism:
///   - `send()` -> push into SpscRing -> notifies registered wakers
///   - `try_recv()` -> pop from SpscRing (non-blocking)
///   - `register_waker()` -> will be called on each send()
///   - `pending_count()` -> approximate count of pending messages
#[repr(C, align(64))]
pub struct AsyncChannel {
    pub id: ChannelId,
    /// Message ring
    ring: SpscRing,
    /// Waker table (protected by SpinLock)
    wakers: SpinLock<WakerTable>,
    /// Local statistics
    pub stats: AsyncChannelStats,
    /// Active concurrent operations counter (drain before freeing/reusing).
    pub active_ops: AtomicU32,
    /// Number of pending messages (approximate)
    pending: AtomicUsize,
    /// Closed channel
    closed: AtomicU32,
    _pad: [u8; 16],
}

// SAFETY: SpscRing is Sync, SpinLock<WakerTable> is Sync
unsafe impl Sync for AsyncChannel {}
unsafe impl Send for AsyncChannel {}

impl AsyncChannel {
    pub fn new() -> Self {
        let s = Self {
            id: alloc_channel_id(),
            ring: SpscRing::new(),
            wakers: SpinLock::new(WakerTable::new()),
            stats: AsyncChannelStats::new(),
            active_ops: AtomicU32::new(0),
            pending: AtomicUsize::new(0),
            closed: AtomicU32::new(0),
            _pad: [0u8; 16],
        };
        s.ring.init();
        s
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) != 0
    }

    pub fn close(&self) {
        self.closed.store(1, Ordering::Release);
        // Notify wakers that channel is closed (outside lock to avoid deadlock)
        let (callbacks, n) = {
            let mut tbl = self.wakers.lock();
            tbl.collect_active_wakers()
        };
        for i in 0..n {
            if let Some(f) = callbacks[i].0 {
                unsafe { f(callbacks[i].1) };
            }
        }
    }

    // -----------------------------------------------------------------------
    // Waker management
    // -----------------------------------------------------------------------

    /// Registers a waker that will be called on each available message.
    /// Returns waker index for deregistration.
    pub fn register_waker(&self, f: WakeFn, data: *mut ()) -> Option<usize> {
        let mut tbl = self.wakers.lock();
        tbl.register(f, data)
    }

    /// Deregisters waker at index `waker_idx`.
    pub fn unregister_waker(&self, waker_idx: usize) -> bool {
        let mut tbl = self.wakers.lock();
        tbl.unregister(waker_idx)
    }

    /// Returns number of active wakers.
    pub fn waker_count(&self) -> usize {
        self.wakers.lock().count()
    }

    // -----------------------------------------------------------------------
    // Send
    // -----------------------------------------------------------------------

    /// Sends `data` into asynchronous channel and notifies wakers.
    pub fn send(&self, data: &[u8], flags: MsgFlags) -> Result<MessageId, IpcError> {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }
        if data.len() > MAX_MSG_SIZE {
            return Err(IpcError::MessageTooLarge);
        }

        let mid = alloc_message_id();

        match self.ring.push_copy(data, flags) {
            Ok(_) => {
                self.stats.sends_ok.fetch_add(1, Ordering::Relaxed);
                self.stats
                    .bytes_transferred
                    .fetch_add(data.len() as u64, Ordering::Relaxed);
                self.pending.fetch_add(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageSent);

                // Notify registered wakers (outside lock)
                let (callbacks, count) = {
                    let mut tbl = self.wakers.lock();
                    tbl.collect_active_wakers()
                };
                if count > 0 {
                    self.stats
                        .waker_fires
                        .fetch_add(count as u64, Ordering::Relaxed);
                    for i in 0..count {
                        if let Some(f) = callbacks[i].0 {
                            unsafe { f(callbacks[i].1) };
                        }
                    }
                }

                Ok(mid)
            }
            Err(IpcError::QueueFull) => {
                self.stats.sends_full.fetch_add(1, Ordering::Relaxed);
                Err(IpcError::QueueFull)
            }
            Err(e) => Err(e),
        }
    }

    // -----------------------------------------------------------------------
    // Receive
    // -----------------------------------------------------------------------

    /// Attempts to receive a message without blocking.
    /// Returns `IpcError::WouldBlock` if no message available.
    pub fn try_recv(&self, buf: &mut [u8]) -> Result<(usize, MsgFlags), IpcError> {
        if self.is_closed() && self.ring.is_empty() {
            return Err(IpcError::Closed);
        }

        match self.ring.pop_into(buf) {
            Ok((len, flags)) => {
                self.stats.recvs_ok.fetch_add(1, Ordering::Relaxed);
                self.pending.fetch_sub(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageReceived);
                Ok((len, flags))
            }
            Err(IpcError::QueueEmpty) => {
                self.stats.recvs_empty.fetch_add(1, Ordering::Relaxed);
                Err(IpcError::WouldBlock)
            }
            Err(e) => Err(e),
        }
    }

    /// Blocking receive (bounded spin-wait).
    pub fn recv(&self, buf: &mut [u8], timeout_spins: u32) -> Result<(usize, MsgFlags), IpcError> {
        let mut spins = 0u32;
        loop {
            match self.try_recv(buf) {
                Ok(r) => return Ok(r),
                Err(IpcError::WouldBlock) => {
                    core::hint::spin_loop();
                    spins += 1;
                    if self.is_closed() {
                        return Err(IpcError::Closed);
                    }
                    if timeout_spins > 0 && spins >= timeout_spins {
                        return Err(IpcError::Timeout);
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    // -----------------------------------------------------------------------
    // Utilities
    // -----------------------------------------------------------------------

    pub fn pending_count(&self) -> usize {
        self.pending.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    pub fn snapshot_stats(&self) -> AsyncChannelStatsSnapshot {
        self.stats.snapshot()
    }
}

// ---------------------------------------------------------------------------
// Global static asynchronous channel table
// ---------------------------------------------------------------------------

pub const ASYNC_CHANNEL_TABLE_SIZE: usize = 256;

struct AsyncChannelTable {
    slots: [MaybeUninit<AsyncChannel>; ASYNC_CHANNEL_TABLE_SIZE],
    used: [bool; ASYNC_CHANNEL_TABLE_SIZE],
    count: usize,
}

// SAFETY: access protected by SpinLock
unsafe impl Send for AsyncChannelTable {}

impl AsyncChannelTable {
    #[allow(dead_code)]
    const fn new() -> Self {
        // SAFETY: all-zero MaybeUninit valid + AtomicBool false = empty table; never read before init.
        unsafe { core::mem::zeroed() }
    }

    fn alloc(&mut self) -> Option<usize> {
        for i in 0..ASYNC_CHANNEL_TABLE_SIZE {
            if !self.used[i] {
                self.slots[i].write(AsyncChannel::new());
                self.used[i] = true;
                self.count += 1;
                return Some(i);
            }
        }
        None
    }

    fn free(&mut self, idx: usize) -> bool {
        if idx < ASYNC_CHANNEL_TABLE_SIZE && self.used[idx] {
            // SAFETY: used[idx] guarantees slots[idx] is initialized; used -> false prevents double-drop.
            unsafe { self.slots[idx].assume_init_drop() };
            self.used[idx] = false;
            self.count -= 1;
            true
        } else {
            false
        }
    }

    unsafe fn get(&self, idx: usize) -> Option<&AsyncChannel> {
        if idx < ASYNC_CHANNEL_TABLE_SIZE && self.used[idx] {
            Some(self.slots[idx].assume_init_ref())
        } else {
            None
        }
    }
}

static ASYNC_CHANNEL_TABLE: SpinLock<AsyncChannelTable> =
    // SAFETY: all-zero SpinLock<AsyncChannelTable> valid: AtomicBool false = unlocked, empty table.
    unsafe { core::mem::zeroed() };

// ---------------------------------------------------------------------------
// High-level public API
// ---------------------------------------------------------------------------

/// Creates an asynchronous channel. Returns its index.
pub fn async_channel_create() -> Result<usize, IpcError> {
    let mut tbl = ASYNC_CHANNEL_TABLE.lock();
    tbl.alloc().ok_or(IpcError::OutOfResources)
}

/// Registers a waker on channel `idx`.
pub fn async_channel_register_waker(
    idx: usize,
    f: WakeFn,
    data: *mut (),
) -> Result<usize, IpcError> {
    let tbl = ASYNC_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static AsyncChannel = unsafe { &*(chan as *const AsyncChannel) };
    if chan_ref.is_closed() {
        return Err(IpcError::Closed);
    }
    chan_ref.active_ops.fetch_add(1, Ordering::SeqCst);
    drop(tbl);
    let res = chan_ref
        .register_waker(f, data)
        .ok_or(IpcError::OutOfResources);
    chan_ref.active_ops.fetch_sub(1, Ordering::SeqCst);
    res
}

/// Deregisters a waker.
pub fn async_channel_unregister_waker(chan_idx: usize, waker_idx: usize) -> Result<(), IpcError> {
    let tbl = ASYNC_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static AsyncChannel = unsafe { &*(chan as *const AsyncChannel) };
    if chan_ref.is_closed() {
        return Err(IpcError::Closed);
    }
    chan_ref.active_ops.fetch_add(1, Ordering::SeqCst);
    drop(tbl);
    let res = if chan_ref.unregister_waker(waker_idx) {
        Ok(())
    } else {
        Err(IpcError::InvalidHandle)
    };
    chan_ref.active_ops.fetch_sub(1, Ordering::SeqCst);
    res
}

/// Sends `data` on asynchronous channel `idx`.
pub fn async_channel_send(idx: usize, data: &[u8], flags: MsgFlags) -> Result<MessageId, IpcError> {
    let tbl = ASYNC_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static AsyncChannel = unsafe { &*(chan as *const AsyncChannel) };
    if chan_ref.is_closed() {
        return Err(IpcError::Closed);
    }
    chan_ref.active_ops.fetch_add(1, Ordering::SeqCst);
    drop(tbl);
    let res = chan_ref.send(data, flags);
    chan_ref.active_ops.fetch_sub(1, Ordering::SeqCst);
    res
}

/// Receives a message without blocking from channel `idx`.
pub fn async_channel_try_recv(idx: usize, buf: &mut [u8]) -> Result<(usize, MsgFlags), IpcError> {
    let tbl = ASYNC_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static AsyncChannel = unsafe { &*(chan as *const AsyncChannel) };
    if chan_ref.is_closed() {
        return Err(IpcError::Closed);
    }
    chan_ref.active_ops.fetch_add(1, Ordering::SeqCst);
    drop(tbl);
    let res = chan_ref.try_recv(buf);
    chan_ref.active_ops.fetch_sub(1, Ordering::SeqCst);
    res
}

/// Closes and destroys asynchronous channel `idx`.
pub fn async_channel_destroy(idx: usize) -> Result<(), IpcError> {
    let tbl = ASYNC_CHANNEL_TABLE.lock();
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    chan.close();
    let chan_ref: &'static AsyncChannel = unsafe { &*(chan as *const AsyncChannel) };
    drop(tbl);

    // Cooperative drain of concurrent in-flight operations before freeing slot
    let mut spins = 0u32;
    while chan_ref.active_ops.load(Ordering::SeqCst) > 0 {
        core::hint::spin_loop();
        spins = spins.wrapping_add(1);
        if spins % 64 == 0 {
            // SAFETY: cooperative_reschedule() yields CPU to scheduler if threads are ready.
            unsafe {
                let _ = crate::scheduler::core::switch::cooperative_reschedule();
            }
        }
    }

    let mut tbl = ASYNC_CHANNEL_TABLE.lock();
    if !tbl.free(idx) {
        return Err(IpcError::InvalidHandle);
    }
    Ok(())
}

/// Number of active asynchronous channels.
pub fn async_channel_count() -> usize {
    ASYNC_CHANNEL_TABLE.lock().count
}
