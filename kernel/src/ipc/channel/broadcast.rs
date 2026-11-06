// ipc/channel/broadcast.rs — Broadcast channel (one-to-many) for Exo-OS
//
// Implements publish/subscribe pattern: a publish sends ONCE and ALL
// subscribers receive an independent copy of the message.
//
// Architecture:
//   - Static list of subscribers (MAX_BROADCAST_SUBSCRIBERS = 16)
//   - Each subscriber has a private SpscRing
//   - Publishing = copy message into each subscriber ring
//   - No dynamic allocation, no Vec/Box
//   - Per-subscriber + global AtomicU64 statistics
//
// Rules observed:
//   - RULE NO-ALLOC: no Vec/Box/Arc/Rc
//   - Subscriber isolation: a full ring does not affect others (DROP_SUBSCRIBER)
//   - No imports from process/ or fs/

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::ipc::core::constants::MAX_MSG_SIZE;
use crate::ipc::core::types::{
    alloc_channel_id, alloc_message_id, ChannelId, IpcError, MessageId, MsgFlags,
};
use crate::ipc::ring::spsc::SpscRing;
use crate::ipc::stats::counters::{StatEvent, IPC_STATS};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::security::capability::{CapTable, CapToken, Rights};

// ---------------------------------------------------------------------------
// Broadcast constants
// ---------------------------------------------------------------------------

/// Maximum number of subscribers per broadcast channel
/// (bounded to limit static memory budget — SpscRing per subscriber).
pub const MAX_BROADCAST_SUBSCRIBERS: usize = 16;

/// Subscriber identifier: index in subscribers array
pub type SubscriberId = u32;

/// Sentinel value — invalid / unregistered subscriber
pub const SUBSCRIBER_INVALID: SubscriberId = u32::MAX;

// ---------------------------------------------------------------------------
// Subscriber slot
// ---------------------------------------------------------------------------

/// Private ring of a subscriber with its performance counters
#[repr(C, align(64))]
pub struct SubscriberSlot {
    /// Dedicated SPSC ring for this subscriber
    ring: SpscRing,
    /// Active subscriber
    active: AtomicBool,
    /// Opaque identifier (e.g. consumer ThreadId)
    owner_id: AtomicU64,
    /// Count of received messages
    msgs_received: AtomicU64,
    /// Count of dropped messages (ring full)
    msgs_dropped: AtomicU64,
    _pad: [u8; 14],
}

// SAFETY: SpscRing is Sync (internal AtomicU64 barriers), all other
// fields are atomic.
unsafe impl Sync for SubscriberSlot {}
unsafe impl Send for SubscriberSlot {}

impl SubscriberSlot {
    pub const fn new() -> Self {
        Self {
            ring: SpscRing::new(),
            active: AtomicBool::new(false),
            owner_id: AtomicU64::new(0),
            msgs_received: AtomicU64::new(0),
            msgs_dropped: AtomicU64::new(0),
            _pad: [0u8; 14],
        }
    }

    /// Initializes and activates this subscriber.
    pub fn activate(&self, owner_id: u64) {
        self.owner_id.store(owner_id, Ordering::Relaxed);
        // SAFETY: initialize ring before marking active
        self.ring.init();
        self.active.store(true, Ordering::Release);
    }

    /// Attempts to activate subscriber atomically (CAS).
    pub fn try_activate(&self, owner_id: u64) -> bool {
        if self
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.owner_id.store(owner_id, Ordering::Relaxed);
            self.ring.init();
            true
        } else {
            false
        }
    }

    /// Deactivates subscriber (no longer consumes future messages).
    pub fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
        self.owner_id.store(0, Ordering::Relaxed);
    }

    /// Attempts to deactivate subscriber atomically (CAS).
    pub fn try_deactivate(&self) -> bool {
        if self
            .active
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.owner_id.store(0, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Writes a message into this subscriber's ring.
    /// Returns `true` if successful, `false` if ring is full (message dropped).
    pub fn deliver(&self, data: &[u8], flags: MsgFlags) -> bool {
        if !self.is_active() {
            return false;
        }
        match self.ring.push_copy(data, flags) {
            Ok(_) => {
                self.msgs_received.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(_) => {
                self.msgs_dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Reads a message from this subscriber's ring.
    pub fn consume(&self, buf: &mut [u8]) -> Result<(usize, MsgFlags), IpcError> {
        if !self.is_active() {
            return Err(IpcError::Closed);
        }
        self.ring.pop_into(buf)
    }

    pub fn msgs_received(&self) -> u64 {
        self.msgs_received.load(Ordering::Relaxed)
    }

    pub fn msgs_dropped(&self) -> u64 {
        self.msgs_dropped.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Global broadcast channel statistics
// ---------------------------------------------------------------------------

#[repr(C, align(64))]
pub struct BroadcastStats {
    pub publishes: AtomicU64,
    pub total_deliveries: AtomicU64,
    pub total_drops: AtomicU64,
    pub subscriber_joins: AtomicU64,
    pub subscriber_leaves: AtomicU64,
    pub bytes_published: AtomicU64,
    _pad: [u8; 16],
}

impl BroadcastStats {
    pub const fn new() -> Self {
        Self {
            publishes: AtomicU64::new(0),
            total_deliveries: AtomicU64::new(0),
            total_drops: AtomicU64::new(0),
            subscriber_joins: AtomicU64::new(0),
            subscriber_leaves: AtomicU64::new(0),
            bytes_published: AtomicU64::new(0),
            _pad: [0u8; 16],
        }
    }

    pub fn snapshot(&self) -> BroadcastStatsSnapshot {
        BroadcastStatsSnapshot {
            publishes: self.publishes.load(Ordering::Relaxed),
            total_deliveries: self.total_deliveries.load(Ordering::Relaxed),
            total_drops: self.total_drops.load(Ordering::Relaxed),
            subscriber_joins: self.subscriber_joins.load(Ordering::Relaxed),
            subscriber_leaves: self.subscriber_leaves.load(Ordering::Relaxed),
            bytes_published: self.bytes_published.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BroadcastStatsSnapshot {
    pub publishes: u64,
    pub total_deliveries: u64,
    pub total_drops: u64,
    pub subscriber_joins: u64,
    pub subscriber_leaves: u64,
    pub bytes_published: u64,
}

// ---------------------------------------------------------------------------
// BroadcastChannel — main structure
// ---------------------------------------------------------------------------

/// Broadcast channel: one producer, N independent consumers.
///
/// Each subscriber has a private SpscRing. The `publish()` method iterates
/// over all active subscribers and copies message into each.
/// Delivery is best-effort: if a subscriber's ring is full, the message
/// is dropped for that subscriber only.
#[repr(C, align(64))]
pub struct BroadcastChannel {
    /// Unique channel identifier
    pub id: ChannelId,
    /// Static array of subscriber slots
    subscribers: [SubscriberSlot; MAX_BROADCAST_SUBSCRIBERS],
    /// Subscriber presence bitmap (protection of table under SpinLock)
    subscriber_bitmap: AtomicU64,
    /// Number of active subscribers
    subscriber_count: AtomicU32,
    /// Global statistics
    pub stats: BroadcastStats,
    /// Closed channel
    closed: AtomicU32,
    /// In-flight operations counter (anti-UAF on destroy)
    pub active_ops: AtomicU32,
    _pad: [u8; 20],
}

// SAFETY: SubscriberSlot is Sync, BroadcastStats is Sync (AtomicU64)
unsafe impl Sync for BroadcastChannel {}
unsafe impl Send for BroadcastChannel {}

impl BroadcastChannel {
    pub const fn new_uninit() -> Self {
        Self {
            id: ChannelId::DANGLING,
            subscribers: [const { SubscriberSlot::new() }; MAX_BROADCAST_SUBSCRIBERS],
            subscriber_bitmap: AtomicU64::new(0),
            subscriber_count: AtomicU32::new(0),
            stats: BroadcastStats::new(),
            closed: AtomicU32::new(0),
            active_ops: AtomicU32::new(0),
            _pad: [0u8; 20],
        }
    }

    pub fn init(&mut self) {
        self.id = alloc_channel_id();
    }

    #[inline]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) != 0
    }

    pub fn close(&self) {
        self.closed.store(1, Ordering::Release);
        // Deactivate all subscribers atomically
        for i in 0..MAX_BROADCAST_SUBSCRIBERS {
            if self.subscribers[i].try_deactivate() {
                let mask = 1u64 << (i % 64);
                self.subscriber_bitmap.fetch_and(!mask, Ordering::AcqRel);
                self.subscriber_count.fetch_sub(1, Ordering::Relaxed);
                self.stats.subscriber_leaves.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Subscriber management
    // -----------------------------------------------------------------------

    /// Subscribes to channel.
    /// Returns subscriber identifier (index) or `None` if full.
    pub fn subscribe(&self, owner_id: u64) -> Option<SubscriberId> {
        if self.is_closed() {
            return None;
        }
        // Find a free slot and activate it atomically (CAS)
        for i in 0..MAX_BROADCAST_SUBSCRIBERS {
            if self.subscribers[i].try_activate(owner_id) {
                let mask = 1u64 << (i % 64);
                self.subscriber_bitmap.fetch_or(mask, Ordering::AcqRel);
                self.subscriber_count.fetch_add(1, Ordering::Relaxed);
                self.stats.subscriber_joins.fetch_add(1, Ordering::Relaxed);
                return Some(i as SubscriberId);
            }
        }
        None
    }

    /// Unsubscribes from channel.
    pub fn unsubscribe(&self, sub_id: SubscriberId) -> bool {
        let idx = sub_id as usize;
        if idx >= MAX_BROADCAST_SUBSCRIBERS {
            return false;
        }
        if self.subscribers[idx].try_deactivate() {
            let mask = 1u64 << (idx % 64);
            self.subscriber_bitmap.fetch_and(!mask, Ordering::AcqRel);
            self.subscriber_count.fetch_sub(1, Ordering::Relaxed);
            self.stats.subscriber_leaves.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    pub fn subscriber_count(&self) -> u32 {
        self.subscriber_count.load(Ordering::Relaxed)
    }

    // -----------------------------------------------------------------------
    // Publishing
    // -----------------------------------------------------------------------

    /// Publishes `data` to ALL active subscribers.
    ///
    /// Returns `(MessageId, delivered, dropped)`:
    ///   - `delivered`: number of subscribers that received message
    ///   - `dropped`: number of subscribers whose ring was full
    pub fn publish(&self, data: &[u8], flags: MsgFlags) -> Result<(MessageId, u32, u32), IpcError> {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }
        if data.len() > MAX_MSG_SIZE {
            return Err(IpcError::MessageTooLarge);
        }

        let mid = alloc_message_id();
        let mut delivered: u32 = 0;
        let mut dropped: u32 = 0;

        // Iterate over active subscribers via bitmap
        let mut bitmap = self.subscriber_bitmap.load(Ordering::Acquire);
        while bitmap != 0 {
            let bit = bitmap.trailing_zeros() as usize;
            bitmap &= bitmap - 1; // clear lowest set bit

            if bit < MAX_BROADCAST_SUBSCRIBERS && self.subscribers[bit].is_active() {
                if self.subscribers[bit].deliver(data, flags) {
                    delivered += 1;
                } else {
                    dropped += 1;
                }
            }
        }

        self.stats.publishes.fetch_add(1, Ordering::Relaxed);
        self.stats
            .total_deliveries
            .fetch_add(delivered as u64, Ordering::Relaxed);
        self.stats
            .total_drops
            .fetch_add(dropped as u64, Ordering::Relaxed);
        self.stats
            .bytes_published
            .fetch_add(data.len() as u64, Ordering::Relaxed);

        if delivered > 0 {
            IPC_STATS.record(StatEvent::MessageSent);
        }

        Ok((mid, delivered, dropped))
    }

    // -----------------------------------------------------------------------
    // Consumption (per subscriber)
    // -----------------------------------------------------------------------

    /// Subscriber `sub_id` reads its next message.
    pub fn recv(
        &self,
        sub_id: SubscriberId,
        buf: &mut [u8],
    ) -> Result<(usize, MsgFlags), IpcError> {
        let idx = sub_id as usize;
        if idx >= MAX_BROADCAST_SUBSCRIBERS {
            return Err(IpcError::InvalidHandle);
        }
        let result = self.subscribers[idx].consume(buf)?;
        IPC_STATS.record(StatEvent::MessageReceived);
        Ok(result)
    }

    /// Snapshot of global statistics.
    pub fn snapshot_stats(&self) -> BroadcastStatsSnapshot {
        self.stats.snapshot()
    }

    /// Per-subscriber: number of received / dropped messages.
    pub fn subscriber_stats(&self, sub_id: SubscriberId) -> Option<(u64, u64)> {
        let idx = sub_id as usize;
        if idx >= MAX_BROADCAST_SUBSCRIBERS || !self.subscribers[idx].is_active() {
            return None;
        }
        Some((
            self.subscribers[idx].msgs_received(),
            self.subscribers[idx].msgs_dropped(),
        ))
    }

    // -----------------------------------------------------------------------
    // CAP-CHECKED PUBLISHING/SUBSCRIPTION — IPC-04 (v6)
    // -----------------------------------------------------------------------

    /// Publishes to all subscribers with capability check (RULE IPC-04 v6).
    ///
    /// # Required rights: `Rights::IPC_SEND`
    #[inline]
    pub fn publish_checked(
        &self,
        data: &[u8],
        flags: MsgFlags,
        table: &CapTable,
        token: CapToken,
    ) -> Result<(MessageId, u32, u32), IpcError> {
        crate::ipc::capability_bridge::check_channel_access(table, token, Rights::IPC_SEND)?;
        self.publish(data, flags)
    }

    /// Subscribes to channel with capability check (RULE IPC-04 v6).
    ///
    /// # Required rights: `Rights::IPC_RECV`
    #[inline]
    pub fn subscribe_checked(
        &self,
        owner_id: u64,
        table: &CapTable,
        token: CapToken,
    ) -> Result<Option<SubscriberId>, IpcError> {
        crate::ipc::capability_bridge::check_channel_access(table, token, Rights::IPC_RECV)?;
        Ok(self.subscribe(owner_id))
    }

    /// Subscriber `sub_id` reads its next message with capability check.
    ///
    /// # Required rights: `Rights::IPC_RECV`
    #[inline]
    pub fn recv_checked(
        &self,
        sub_id: SubscriberId,
        buf: &mut [u8],
        table: &CapTable,
        token: CapToken,
    ) -> Result<(usize, MsgFlags), IpcError> {
        crate::ipc::capability_bridge::check_channel_access(table, token, Rights::IPC_RECV)?;
        self.recv(sub_id, buf)
    }
}

// ---------------------------------------------------------------------------
// Global static broadcast channel table
// ---------------------------------------------------------------------------

pub const BROADCAST_CHANNEL_TABLE_SIZE: usize = 16;

struct BroadcastChannelTable {
    slots: [MaybeUninit<BroadcastChannel>; BROADCAST_CHANNEL_TABLE_SIZE],
    used: [bool; BROADCAST_CHANNEL_TABLE_SIZE],
    count: usize,
}

// SAFETY: access protected by SpinLock
unsafe impl Send for BroadcastChannelTable {}

impl BroadcastChannelTable {
    #[allow(dead_code)]
    const fn new() -> Self {
        // SAFETY: mem::zeroed() avoids const-eval memory limits for large arrays.
        unsafe { core::mem::zeroed() }
    }

    fn alloc(&mut self) -> Option<usize> {
        for i in 0..BROADCAST_CHANNEL_TABLE_SIZE {
            if !self.used[i] {
                let mut chan = BroadcastChannel::new_uninit();
                chan.init();
                self.slots[i].write(chan);
                self.used[i] = true;
                self.count += 1;
                return Some(i);
            }
        }
        None
    }

    fn free(&mut self, idx: usize) -> bool {
        if idx < BROADCAST_CHANNEL_TABLE_SIZE && self.used[idx] {
            // SAFETY: used[idx] guarantees init
            unsafe { self.slots[idx].assume_init_drop() };
            self.used[idx] = false;
            self.count -= 1;
            true
        } else {
            false
        }
    }

    unsafe fn get(&self, idx: usize) -> Option<&BroadcastChannel> {
        if idx < BROADCAST_CHANNEL_TABLE_SIZE && self.used[idx] {
            Some(self.slots[idx].assume_init_ref())
        } else {
            None
        }
    }
}

// SAFETY: .bss section is zero-initialized by bootloader.
// SpinLock<BroadcastChannelTable> all-zero = valid (unlocked, empty table).
// MaybeUninit::uninit() = 0 const-eval bytes, memory comes from .bss.
static BROADCAST_TABLE: core::mem::MaybeUninit<SpinLock<BroadcastChannelTable>> =
    core::mem::MaybeUninit::uninit();

#[inline(always)]
fn broadcast_table() -> &'static SpinLock<BroadcastChannelTable> {
    // SAFETY: .bss is zero-initialized at boot. SpinLock all-zeros = unlocked.
    unsafe { BROADCAST_TABLE.assume_init_ref() }
}

// ---------------------------------------------------------------------------
// High-level public API
// ---------------------------------------------------------------------------

/// Creates a broadcast channel. Returns its index.
pub fn broadcast_create() -> Result<usize, IpcError> {
    let mut tbl = broadcast_table().lock();
    tbl.alloc().ok_or(IpcError::OutOfResources)
}

/// Subscribes to broadcast channel `chan_idx`. Returns SubscriberId.
pub fn broadcast_subscribe(chan_idx: usize, owner_id: u64) -> Result<SubscriberId, IpcError> {
    let tbl = broadcast_table().lock();
    // SAFETY: tbl.get() verifies used[chan_idx] before returning.
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static BroadcastChannel = unsafe { &*(chan as *const BroadcastChannel) };
    chan_ref.active_ops.fetch_add(1, Ordering::Acquire);
    drop(tbl);
    let res = chan_ref.subscribe(owner_id).ok_or(IpcError::OutOfResources);
    chan_ref.active_ops.fetch_sub(1, Ordering::Release);
    res
}

/// Unsubscribes from channel `chan_idx`.
pub fn broadcast_unsubscribe(chan_idx: usize, sub_id: SubscriberId) -> Result<(), IpcError> {
    let tbl = broadcast_table().lock();
    // SAFETY: tbl.get() verifies used[chan_idx] before returning.
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static BroadcastChannel = unsafe { &*(chan as *const BroadcastChannel) };
    chan_ref.active_ops.fetch_add(1, Ordering::Acquire);
    drop(tbl);
    let ok = chan_ref.unsubscribe(sub_id);
    chan_ref.active_ops.fetch_sub(1, Ordering::Release);
    if ok {
        Ok(())
    } else {
        Err(IpcError::InvalidHandle)
    }
}

/// Publishes a message to all subscribers of channel `chan_idx`.
pub fn broadcast_publish(
    chan_idx: usize,
    data: &[u8],
    flags: MsgFlags,
) -> Result<(MessageId, u32, u32), IpcError> {
    let tbl = broadcast_table().lock();
    // SAFETY: tbl.get() verifies used[chan_idx] before returning.
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static BroadcastChannel = unsafe { &*(chan as *const BroadcastChannel) };
    chan_ref.active_ops.fetch_add(1, Ordering::Acquire);
    drop(tbl);
    let res = chan_ref.publish(data, flags);
    chan_ref.active_ops.fetch_sub(1, Ordering::Release);
    res
}

/// Receives a message for subscriber `sub_id` on channel `chan_idx`.
pub fn broadcast_recv(
    chan_idx: usize,
    sub_id: SubscriberId,
    buf: &mut [u8],
) -> Result<(usize, MsgFlags), IpcError> {
    let tbl = broadcast_table().lock();
    // SAFETY: tbl.get() verifies used[chan_idx] before returning.
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static BroadcastChannel = unsafe { &*(chan as *const BroadcastChannel) };
    chan_ref.active_ops.fetch_add(1, Ordering::Acquire);
    drop(tbl);
    let res = chan_ref.recv(sub_id, buf);
    chan_ref.active_ops.fetch_sub(1, Ordering::Release);
    res
}

/// Closes and destroys broadcast channel.
pub fn broadcast_destroy(chan_idx: usize) -> Result<(), IpcError> {
    let tbl = broadcast_table().lock();
    // SAFETY: tbl.get() verifies used[chan_idx] before returning.
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static BroadcastChannel = unsafe { &*(chan as *const BroadcastChannel) };
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

    let mut tbl = broadcast_table().lock();
    tbl.free(chan_idx);
    Ok(())
}

// ---------------------------------------------------------------------------
// Cap-checked API — IPC-04 (v6)
// ---------------------------------------------------------------------------

/// Publishes with capability check (IPC-04 v6).
pub fn broadcast_publish_checked(
    chan_idx: usize,
    data: &[u8],
    flags: MsgFlags,
    table: &CapTable,
    token: CapToken,
) -> Result<(MessageId, u32, u32), IpcError> {
    let tbl = broadcast_table().lock();
    // SAFETY: tbl.get() verifies used[chan_idx] before returning.
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static BroadcastChannel = unsafe { &*(chan as *const BroadcastChannel) };
    chan_ref.active_ops.fetch_add(1, Ordering::Acquire);
    drop(tbl);
    let res = chan_ref.publish_checked(data, flags, table, token);
    chan_ref.active_ops.fetch_sub(1, Ordering::Release);
    res
}

/// Subscribes with capability check (IPC-04 v6).
pub fn broadcast_subscribe_checked(
    chan_idx: usize,
    owner_id: u64,
    table: &CapTable,
    token: CapToken,
) -> Result<SubscriberId, IpcError> {
    let tbl = broadcast_table().lock();
    // SAFETY: tbl.get() verifies used[chan_idx] before returning.
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static BroadcastChannel = unsafe { &*(chan as *const BroadcastChannel) };
    chan_ref.active_ops.fetch_add(1, Ordering::Acquire);
    drop(tbl);
    let res = chan_ref
        .subscribe_checked(owner_id, table, token)
        .and_then(|opt| opt.ok_or(IpcError::OutOfResources));
    chan_ref.active_ops.fetch_sub(1, Ordering::Release);
    res
}

/// Receives with capability check (IPC-04 v6).
pub fn broadcast_recv_checked(
    chan_idx: usize,
    sub_id: SubscriberId,
    buf: &mut [u8],
    table: &CapTable,
    token: CapToken,
) -> Result<(usize, MsgFlags), IpcError> {
    let tbl = broadcast_table().lock();
    // SAFETY: tbl.get() verifies used[chan_idx] before returning.
    let chan = unsafe { tbl.get(chan_idx) }.ok_or(IpcError::InvalidHandle)?;
    let chan_ref: &'static BroadcastChannel = unsafe { &*(chan as *const BroadcastChannel) };
    chan_ref.active_ops.fetch_add(1, Ordering::Acquire);
    drop(tbl);
    let res = chan_ref.recv_checked(sub_id, buf, table, token);
    chan_ref.active_ops.fetch_sub(1, Ordering::Release);
    res
}
