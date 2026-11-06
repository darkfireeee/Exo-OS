// ipc/channel/mpmc.rs — Multi-Producer / Multi-Consumer Channel for Exo-OS
//
// This channel wraps ring::MpmcRing (Dmitry Vyukov CAS-based algorithm)
// to provide a high-level interface with rights management, statistics,
// and configurable overflow policy.
//
// Features:
//   - N concurrent senders — lock-free CAS on queue head
//   - M concurrent receivers — lock-free CAS on queue tail
//   - Overflow policy: DROP_OLDEST, DROP_NEWEST, BLOCK
//   - Effective capacity: RING_SIZE = 16 slots (power of 2).
//     PATCH-IPC-01: previous docs mistakenly stated "4096 slots".
//     To change capacity, modify RING_SIZE in ipc/core/constants.rs.
//   - Real-time statistics (AtomicU64, no mutex)

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::ipc::core::constants::MAX_MSG_SIZE;
use crate::ipc::core::types::{
    alloc_channel_id, alloc_message_id, ChannelId, IpcError, MessageId, MsgFlags,
};
use crate::ipc::ring::mpmc::MpmcRing;
use crate::ipc::stats::counters::{StatEvent, IPC_STATS};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::security::capability::{CapTable, CapToken, Rights};

// ---------------------------------------------------------------------------
// Overflow policy
// ---------------------------------------------------------------------------

/// Behavior when queue is full
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OverflowPolicy {
    /// Reject incoming message (default)
    DropNewest = 0,
    /// Evict oldest message to make room
    DropOldest = 1,
    /// Block sender (spin-wait until room is available)
    Block = 2,
}

impl OverflowPolicy {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::DropOldest,
            2 => Self::Block,
            _ => Self::DropNewest,
        }
    }
}

// ---------------------------------------------------------------------------
// MPMC statistics
// ---------------------------------------------------------------------------

/// Performance counters of an MPMC channel
#[repr(C, align(64))]
pub struct MpmcStats {
    pub sends_ok: AtomicU64,
    pub sends_dropped: AtomicU64,
    pub sends_blocked: AtomicU64,
    pub recvs_ok: AtomicU64,
    pub recvs_empty: AtomicU64,
    pub total_bytes_sent: AtomicU64,
    pub total_bytes_recv: AtomicU64,
    pub overflow_drops: AtomicU64,
    _pad: [u8; 0],
}

impl MpmcStats {
    pub const fn new() -> Self {
        Self {
            sends_ok: AtomicU64::new(0),
            sends_dropped: AtomicU64::new(0),
            sends_blocked: AtomicU64::new(0),
            recvs_ok: AtomicU64::new(0),
            recvs_empty: AtomicU64::new(0),
            total_bytes_sent: AtomicU64::new(0),
            total_bytes_recv: AtomicU64::new(0),
            overflow_drops: AtomicU64::new(0),
            _pad: [],
        }
    }

    /// Consistent snapshot (relaxed sequentiality read)
    pub fn snapshot(&self) -> MpmcStatsSnapshot {
        MpmcStatsSnapshot {
            sends_ok: self.sends_ok.load(Ordering::Relaxed),
            sends_dropped: self.sends_dropped.load(Ordering::Relaxed),
            sends_blocked: self.sends_blocked.load(Ordering::Relaxed),
            recvs_ok: self.recvs_ok.load(Ordering::Relaxed),
            recvs_empty: self.recvs_empty.load(Ordering::Relaxed),
            total_bytes_sent: self.total_bytes_sent.load(Ordering::Relaxed),
            total_bytes_recv: self.total_bytes_recv.load(Ordering::Relaxed),
            overflow_drops: self.overflow_drops.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MpmcStatsSnapshot {
    pub sends_ok: u64,
    pub sends_dropped: u64,
    pub sends_blocked: u64,
    pub recvs_ok: u64,
    pub recvs_empty: u64,
    pub total_bytes_sent: u64,
    pub total_bytes_recv: u64,
    pub overflow_drops: u64,
}

// ---------------------------------------------------------------------------
// MpmcChannel — main structure
// ---------------------------------------------------------------------------

/// MPMC channel using Vyukov's lock-free CAS ring.
///
/// Effective capacity is `MPMC_RING_SIZE` (= `RING_SIZE` = 16 slots).
/// PATCH-IPC-01: canonical reference -> ipc/core/constants.rs::RING_SIZE.
/// Multiple producers and consumers can operate concurrently
/// without mutexes.
#[repr(C, align(64))]
pub struct MpmcChannel {
    /// Unique channel identifier
    pub id: ChannelId,
    /// Underlying data ring
    ring: MpmcRing,
    /// Local statistics
    pub stats: MpmcStats,
    /// Active concurrent operations counter (drain before freeing/reusing).
    pub active_ops: AtomicU32,
    /// Closed channel
    closed: AtomicU32,
    /// Overflow policy (OverflowPolicy encoded as u8)
    overflow_policy: AtomicU32,
    /// Number of registered producers
    producer_count: AtomicU32,
    /// Number of registered consumers
    consumer_count: AtomicU32,
    /// Number of messages en attente (approximatif)
    pending: AtomicUsize,
    _pad: [u8; 20],
}

// SAFETY: MpmcRing is Sync (internal atomic CAS), AtomicU64 is Sync.
unsafe impl Sync for MpmcChannel {}
unsafe impl Send for MpmcChannel {}

impl MpmcChannel {
    /// Creates a new MPMC channel.
    pub fn new(policy: OverflowPolicy) -> Self {
        let c = Self {
            id: alloc_channel_id(),
            ring: MpmcRing::new_uninit(),
            stats: MpmcStats::new(),
            active_ops: AtomicU32::new(0),
            closed: AtomicU32::new(0),
            overflow_policy: AtomicU32::new(policy as u32),
            producer_count: AtomicU32::new(0),
            consumer_count: AtomicU32::new(0),
            pending: AtomicUsize::new(0),
            _pad: [0u8; 20],
        };
        c.ring.init();
        c
    }

    #[inline]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) != 0
    }

    pub fn close(&self) {
        self.closed.store(1, Ordering::Release);
    }

    pub fn register_producer(&self) -> u32 {
        self.producer_count.fetch_add(1, Ordering::Relaxed)
    }

    pub fn unregister_producer(&self) -> u32 {
        self.producer_count.fetch_sub(1, Ordering::Relaxed)
    }

    pub fn register_consumer(&self) -> u32 {
        self.consumer_count.fetch_add(1, Ordering::Relaxed)
    }

    pub fn unregister_consumer(&self) -> u32 {
        self.consumer_count.fetch_sub(1, Ordering::Relaxed)
    }

    pub fn producer_count(&self) -> u32 {
        self.producer_count.load(Ordering::Relaxed)
    }

    pub fn consumer_count(&self) -> u32 {
        self.consumer_count.load(Ordering::Relaxed)
    }

    // -----------------------------------------------------------------------
    // SEND
    // -----------------------------------------------------------------------

    /// Sends `data` into MPMC channel.
    ///
    /// Configurable behavior when queue is full (see OverflowPolicy).
    pub fn send(&self, data: &[u8], flags: MsgFlags) -> Result<MessageId, IpcError> {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }
        if data.len() > MAX_MSG_SIZE {
            return Err(IpcError::MessageTooLarge);
        }

        let mid = alloc_message_id();
        let policy = OverflowPolicy::from_u8(self.overflow_policy.load(Ordering::Relaxed) as u8);

        match self.ring.push_copy(data, flags) {
            Ok(_seq) => {
                self.stats.sends_ok.fetch_add(1, Ordering::Relaxed);
                self.stats
                    .total_bytes_sent
                    .fetch_add(data.len() as u64, Ordering::Relaxed);
                self.pending.fetch_add(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageSent);
                Ok(mid)
            }
            Err(IpcError::QueueFull) => {
                match policy {
                    OverflowPolicy::DropNewest => {
                        self.stats.sends_dropped.fetch_add(1, Ordering::Relaxed);
                        self.stats.overflow_drops.fetch_add(1, Ordering::Relaxed);
                        IPC_STATS.record(StatEvent::MessageDropped);
                        Err(IpcError::QueueFull)
                    }
                    OverflowPolicy::DropOldest => {
                        // Consume (and discard) oldest message
                        let mut discard = [0u8; MAX_MSG_SIZE];
                        if self.ring.pop_into(&mut discard).is_ok() {
                            self.pending.fetch_sub(1, Ordering::Relaxed);
                            self.stats.overflow_drops.fetch_add(1, Ordering::Relaxed);
                        }
                        // Retry
                        match self.ring.push_copy(data, flags) {
                            Ok(_) => {
                                self.pending.fetch_add(1, Ordering::Relaxed);
                                self.stats.sends_ok.fetch_add(1, Ordering::Relaxed);
                                IPC_STATS.record(StatEvent::MessageSent);
                                Ok(mid)
                            }
                            Err(e) => Err(e),
                        }
                    }
                    OverflowPolicy::Block => {
                        // Spin-wait until room is available
                        self.stats.sends_blocked.fetch_add(1, Ordering::Relaxed);
                        let mut spins: u32 = 0;
                        loop {
                            core::hint::spin_loop();
                            spins += 1;
                            if spins % 128 == 0 {
                                // SAFETY: cooperative reschedule without holding locks.
                                unsafe {
                                    crate::scheduler::core::switch::cooperative_reschedule();
                                }
                            }
                            if self.is_closed() {
                                return Err(IpcError::Closed);
                            }
                            if spins > 500_000 {
                                return Err(IpcError::Timeout);
                            }
                            match self.ring.push_copy(data, flags) {
                                Ok(_) => {
                                    self.pending.fetch_add(1, Ordering::Relaxed);
                                    self.stats.sends_ok.fetch_add(1, Ordering::Relaxed);
                                    IPC_STATS.record(StatEvent::MessageSent);
                                    return Ok(mid);
                                }
                                Err(IpcError::QueueFull) => continue,
                                Err(e) => return Err(e),
                            }
                        }
                    }
                }
            }
            Err(e) => Err(e),
        }
    }

    // -----------------------------------------------------------------------
    // RECEIVE
    // -----------------------------------------------------------------------

    /// Receives a message from MPMC channel (consumption).
    ///
    /// Thread-safe — multiple consumers can call recv() concurrently.
    pub fn recv(&self, buf: &mut [u8], flags: MsgFlags) -> Result<(usize, MsgFlags), IpcError> {
        if self.is_closed() && self.ring.is_empty_approx() {
            return Err(IpcError::Closed);
        }

        if flags.contains(MsgFlags::NOWAIT) {
            match self.ring.pop_into(buf) {
                Ok((len, msg_flags)) => {
                    self.stats.recvs_ok.fetch_add(1, Ordering::Relaxed);
                    self.stats
                        .total_bytes_recv
                        .fetch_add(len as u64, Ordering::Relaxed);
                    self.pending.fetch_sub(1, Ordering::Relaxed);
                    IPC_STATS.record(StatEvent::MessageReceived);
                    Ok((len, msg_flags))
                }
                Err(IpcError::QueueEmpty) => {
                    self.stats.recvs_empty.fetch_add(1, Ordering::Relaxed);
                    Err(IpcError::WouldBlock)
                }
                Err(e) => Err(e),
            }
        } else {
            // Blocking spin-wait
            let mut spins: u32 = 0;
            loop {
                match self.ring.pop_into(buf) {
                    Ok((len, msg_flags)) => {
                        self.stats.recvs_ok.fetch_add(1, Ordering::Relaxed);
                        self.stats
                            .total_bytes_recv
                            .fetch_add(len as u64, Ordering::Relaxed);
                        self.pending.fetch_sub(1, Ordering::Relaxed);
                        IPC_STATS.record(StatEvent::MessageReceived);
                        return Ok((len, msg_flags));
                    }
                    Err(IpcError::QueueEmpty) => {
                        core::hint::spin_loop();
                        spins += 1;
                        if spins % 128 == 0 {
                            // SAFETY: cooperative reschedule without holding locks.
                            unsafe {
                                crate::scheduler::core::switch::cooperative_reschedule();
                            }
                        }
                        if self.is_closed() && self.pending.load(Ordering::Relaxed) == 0 {
                            return Err(IpcError::Closed);
                        }
                        if spins > 2_000_000 {
                            return Err(IpcError::Timeout);
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }

    /// Attempts to read up to `max` messages in a single operation (batch).
    /// Returns number of messages actually read.
    pub fn recv_batch(
        &self,
        bufs: &mut [[u8; MAX_MSG_SIZE]],
        max: usize,
    ) -> Result<usize, IpcError> {
        let n = max.min(bufs.len());
        let mut count = 0usize;

        for i in 0..n {
            match self.ring.pop_into(&mut bufs[i]) {
                Ok((len, _flags)) => {
                    self.stats.recvs_ok.fetch_add(1, Ordering::Relaxed);
                    self.stats
                        .total_bytes_recv
                        .fetch_add(len as u64, Ordering::Relaxed);
                    count += 1;
                }
                Err(IpcError::QueueEmpty) => break,
                Err(e) => {
                    if count == 0 {
                        return Err(e);
                    }
                    break;
                }
            }
        }

        if count > 0 {
            self.pending.fetch_sub(count, Ordering::Relaxed);
        }
        Ok(count)
    }

    // -----------------------------------------------------------------------
    // Utilities
    // -----------------------------------------------------------------------

    /// Approximate size of queue (may be inaccurate under concurrency).
    pub fn len_approx(&self) -> usize {
        self.pending.load(Ordering::Relaxed)
    }

    pub fn is_empty_approx(&self) -> bool {
        self.ring.is_empty_approx()
    }

    pub fn snapshot_stats(&self) -> MpmcStatsSnapshot {
        self.stats.snapshot()
    }

    // -----------------------------------------------------------------------
    // CAP-CHECKED SEND/RECV — IPC-04 (v6)
    // -----------------------------------------------------------------------

    /// Sends with capability check (RULE IPC-04 v6).
    ///
    /// Calls `security::access_control::check_access()` before actual send.
    /// # Required rights: `Rights::IPC_SEND`
    #[inline]
    pub fn send_checked(
        &self,
        data: &[u8],
        flags: MsgFlags,
        table: &CapTable,
        token: CapToken,
    ) -> Result<MessageId, IpcError> {
        crate::ipc::capability_bridge::check_channel_access(table, token, Rights::IPC_SEND)?;
        self.send(data, flags)
    }

    /// Receives with capability check (RULE IPC-04 v6).
    /// # Required rights: `Rights::IPC_RECV`
    #[inline]
    pub fn recv_checked(
        &self,
        buf: &mut [u8],
        flags: MsgFlags,
        table: &CapTable,
        token: CapToken,
    ) -> Result<(usize, MsgFlags), IpcError> {
        crate::ipc::capability_bridge::check_channel_access(table, token, Rights::IPC_RECV)?;
        self.recv(buf, flags)
    }
}

// ---------------------------------------------------------------------------
// Global static MPMC channel table
// ---------------------------------------------------------------------------

/// Size of global MPMC channel table
pub const MPMC_CHANNEL_TABLE_SIZE: usize = 256;

struct MpmcChannelTable {
    slots: [MaybeUninit<MpmcChannel>; MPMC_CHANNEL_TABLE_SIZE],
    used: [bool; MPMC_CHANNEL_TABLE_SIZE],
    count: usize,
}

// SAFETY: access protected by SpinLock<MpmcChannelTable>
unsafe impl Send for MpmcChannelTable {}

impl MpmcChannelTable {
    #[allow(dead_code)]
    const fn new() -> Self {
        // SAFETY: mem::zeroed() avoids const-eval memory limits for large arrays.
        unsafe { core::mem::zeroed() }
    }

    fn alloc(&mut self, policy: OverflowPolicy) -> Option<usize> {
        for i in 0..MPMC_CHANNEL_TABLE_SIZE {
            if !self.used[i] {
                self.slots[i].write(MpmcChannel::new(policy));
                self.used[i] = true;
                self.count += 1;
                return Some(i);
            }
        }
        None
    }

    fn free(&mut self, idx: usize) -> bool {
        if idx < MPMC_CHANNEL_TABLE_SIZE && self.used[idx] {
            // SAFETY: used[idx] guarantees initialization
            unsafe { self.slots[idx].assume_init_drop() };
            self.used[idx] = false;
            self.count -= 1;
            true
        } else {
            false
        }
    }

    unsafe fn get(&self, idx: usize) -> Option<&MpmcChannel> {
        if idx < MPMC_CHANNEL_TABLE_SIZE && self.used[idx] {
            Some(self.slots[idx].assume_init_ref())
        } else {
            None
        }
    }
}

static MPMC_CHANNEL_TABLE: SpinLock<MpmcChannelTable> =
    // SAFETY: all-zero SpinLock<MpmcChannelTable> valid: AtomicBool false = unlocked, empty table.
    unsafe { core::mem::zeroed() };

// ---------------------------------------------------------------------------
// High-level public API
// ---------------------------------------------------------------------------

/// Creates an MPMC channel with specified overflow policy.
pub fn mpmc_channel_create(policy: OverflowPolicy) -> Result<usize, IpcError> {
    let mut tbl = MPMC_CHANNEL_TABLE.lock();
    tbl.alloc(policy).ok_or(IpcError::OutOfResources)
}

/// Sends a message into MPMC channel identified by `idx`.
pub fn mpmc_channel_send(idx: usize, data: &[u8], flags: MsgFlags) -> Result<MessageId, IpcError> {
    let tbl = MPMC_CHANNEL_TABLE.lock();
    // SAFETY: internal tbl.get() verifies used[idx] < TABLE_SIZE before returning.
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static MpmcChannel = unsafe { &*(chan as *const MpmcChannel) };
    if chan_ref.is_closed() {
        return Err(IpcError::Closed);
    }
    chan_ref.active_ops.fetch_add(1, Ordering::SeqCst);
    drop(tbl);
    let res = chan_ref.send(data, flags);
    chan_ref.active_ops.fetch_sub(1, Ordering::SeqCst);
    res
}

/// Receives a message from MPMC channel identified by `idx`.
pub fn mpmc_channel_recv(
    idx: usize,
    buf: &mut [u8],
    flags: MsgFlags,
) -> Result<(usize, MsgFlags), IpcError> {
    let tbl = MPMC_CHANNEL_TABLE.lock();
    // SAFETY: tbl.get() verifies used[idx] before returning.
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static MpmcChannel = unsafe { &*(chan as *const MpmcChannel) };
    if chan_ref.is_closed() {
        return Err(IpcError::Closed);
    }
    chan_ref.active_ops.fetch_add(1, Ordering::SeqCst);
    drop(tbl);
    let res = chan_ref.recv(buf, flags);
    chan_ref.active_ops.fetch_sub(1, Ordering::SeqCst);
    res
}

/// Closes and destroys MPMC channel identified by `idx`.
pub fn mpmc_channel_destroy(idx: usize) -> Result<(), IpcError> {
    let tbl = MPMC_CHANNEL_TABLE.lock();
    // SAFETY: tbl.get() verifies used[idx].
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    chan.close();
    let chan_ref: &'static MpmcChannel = unsafe { &*(chan as *const MpmcChannel) };
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

    let mut tbl = MPMC_CHANNEL_TABLE.lock();
    if !tbl.free(idx) {
        return Err(IpcError::InvalidHandle);
    }
    Ok(())
}

/// Returns number of active MPMC channels.
pub fn mpmc_channel_count() -> usize {
    MPMC_CHANNEL_TABLE.lock().count
}

/// Sends into MPMC channel `idx` with capability check (IPC-04 v6).
pub fn mpmc_channel_send_checked(
    idx: usize,
    data: &[u8],
    flags: MsgFlags,
    table: &CapTable,
    token: CapToken,
) -> Result<MessageId, IpcError> {
    let tbl = MPMC_CHANNEL_TABLE.lock();
    // SAFETY: tbl.get() verifies used[idx] before returning.
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static MpmcChannel = unsafe { &*(chan as *const MpmcChannel) };
    if chan_ref.is_closed() {
        return Err(IpcError::Closed);
    }
    chan_ref.active_ops.fetch_add(1, Ordering::SeqCst);
    drop(tbl);
    let res = chan_ref.send_checked(data, flags, table, token);
    chan_ref.active_ops.fetch_sub(1, Ordering::SeqCst);
    res
}

/// Receives from MPMC channel `idx` with capability check (IPC-04 v6).
pub fn mpmc_channel_recv_checked(
    idx: usize,
    buf: &mut [u8],
    flags: MsgFlags,
    table: &CapTable,
    token: CapToken,
) -> Result<(usize, MsgFlags), IpcError> {
    let tbl = MPMC_CHANNEL_TABLE.lock();
    // SAFETY: tbl.get() verifies used[idx] before returning.
    let chan = unsafe { tbl.get(idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static MpmcChannel = unsafe { &*(chan as *const MpmcChannel) };
    if chan_ref.is_closed() {
        return Err(IpcError::Closed);
    }
    chan_ref.active_ops.fetch_add(1, Ordering::SeqCst);
    drop(tbl);
    let res = chan_ref.recv_checked(buf, flags, table, token);
    chan_ref.active_ops.fetch_sub(1, Ordering::SeqCst);
    res
}
