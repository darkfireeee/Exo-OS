// ipc/message/priority.rs — IPC priority queue for Exo-OS
//
// This module implements a two-level IPC priority queue:
//   - REALTIME level (priority > PRIORITY_RT_THRESHOLD): dedicated SpscRing
//   - NORMAL level (priority <= PRIORITY_RT_THRESHOLD): standard SpscRing
//
// Dequeue always reads REALTIME first, then NORMAL.
// If the RT queue is full, RT messages are downgraded to NORMAL
// with the `DEGRADED` flag set.
//
// A priority queue can also be weighted: the weight determines
// how many normal messages are read between two RT messages (to prevent
// starvation).
//
// RULE PRIO-01: two static rings per PriorityQueue, no O(N) sorting.
// RULE PRIO-02: RT level is ALWAYS consumed first.
// RULE PRIO-03: no starvation -> configurable RT:NORMAL ratio.

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::ipc::core::types::IpcError;
use crate::ipc::message::builder::IpcMessage;
use crate::ipc::stats::counters::{StatEvent, IPC_STATS};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Priority threshold for REALTIME queue (>= -> RT)
pub const PRIORITY_RT_THRESHOLD: u8 = 192;

/// RT ring capacity (smaller: RT messages are rare)
pub const PRIORITY_RT_RING_CAP: usize = 64;

/// NORMAL ring capacity
pub const PRIORITY_NORMAL_RING_CAP: usize = 256;

/// RT:NORMAL ratio for anti-starvation (consume N_NORMAL between each RT)
pub const PRIORITY_ANTI_STARVATION_RATIO: u32 = 8;

/// Maximum number of PriorityQueues in global table
pub const MAX_PRIORITY_QUEUES: usize = 64;

// ---------------------------------------------------------------------------
// Message index for priority queue
// ---------------------------------------------------------------------------
//
// To avoid copying IpcMessages (>4KB) into the ring, we store a
// compact `PrioMsgSlot`: header + inline payload 256 bytes max.
// For larger payloads, use StreamChannel.

pub const PRIO_INLINE_PAYLOAD: usize = 256;

/// Message slot in priority queue
#[repr(C, align(64))]
pub struct PrioMsgSlot {
    pub seq: u64,
    pub src: u32,
    pub dst: u32,
    pub priority: u8,
    pub msg_type: u8,
    pub flags: u16,
    pub payload_len: u16,
    pub cookie: u64,
    _pad: [u8; 5],
    pub payload: [u8; PRIO_INLINE_PAYLOAD],
}

impl PrioMsgSlot {
    pub const fn empty() -> Self {
        Self {
            seq: 0,
            src: 0,
            dst: 0,
            priority: 0,
            msg_type: 0,
            flags: 0,
            payload_len: 0,
            cookie: 0,
            _pad: [0u8; 5],
            payload: [0u8; PRIO_INLINE_PAYLOAD],
        }
    }

    pub fn from_message(msg: &IpcMessage) -> Self {
        let plen = (msg.payload_len as usize).min(PRIO_INLINE_PAYLOAD);
        let mut slot = Self::empty();
        slot.seq = msg.seq;
        slot.src = msg.src.0.get() as u32;
        slot.dst = msg.dst.0.get() as u32;
        slot.priority = msg.priority;
        slot.msg_type = msg.msg_type as u8;
        slot.flags = msg.flags.bits();
        slot.payload_len = plen as u16;
        slot.cookie = msg.cookie;
        if plen > 0 {
            slot.payload[..plen].copy_from_slice(&msg.payload()[..plen]);
        }
        slot
    }
}

// SAFETY: PrioMsgSlot is Copy-able in terms of data
unsafe impl Sync for PrioMsgSlot {}

// ---------------------------------------------------------------------------
// Static ring for priority queue
// ---------------------------------------------------------------------------

/// Bounded circular ring for PrioMsgSlot (no dependency on SpscRing<T>)
struct PrioRing<const N: usize> {
    slots: [MaybeUninit<PrioMsgSlot>; N],
    head: AtomicU32,
    tail: AtomicU32,
    capacity: u32,
}

impl<const N: usize> PrioRing<N> {
    const fn new() -> Self {
        const EMPTY: MaybeUninit<PrioMsgSlot> = MaybeUninit::uninit();
        Self {
            slots: [EMPTY; N],
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            capacity: N as u32,
        }
    }

    fn push(&self, slot: PrioMsgSlot) -> bool {
        loop {
            let tail = self.tail.load(Ordering::Relaxed);
            let head = self.head.load(Ordering::Acquire);
            if tail.wrapping_sub(head) >= self.capacity {
                return false; // full
            }
            match self.tail.compare_exchange_weak(
                tail,
                tail.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    let idx = (tail % self.capacity) as usize;
                    // SAFETY: exclusive slot via CAS tail
                    unsafe {
                        (self.slots[idx].as_ptr() as *mut PrioMsgSlot).write(slot);
                    }
                    return true;
                }
                Err(_) => core::hint::spin_loop(),
            }
        }
    }

    fn pop(&self) -> Option<PrioMsgSlot> {
        loop {
            let head = self.head.load(Ordering::Relaxed);
            let tail = self.tail.load(Ordering::Acquire);
            if head == tail {
                return None; // empty
            }
            match self.head.compare_exchange_weak(
                head,
                head.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    let idx = (head % self.capacity) as usize;
                    // SAFETY: initialized slot (tail > head)
                    let slot = unsafe { self.slots[idx].assume_init_read() };
                    return Some(slot);
                }
                Err(_) => core::hint::spin_loop(),
            }
        }
    }

    fn is_empty(&self) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        head == tail
    }

    fn len(&self) -> u32 {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        tail.wrapping_sub(head)
    }
}

// SAFETY: PrioRing concurrent access via CAS
unsafe impl<const N: usize> Sync for PrioRing<N> {}

// ---------------------------------------------------------------------------
// PriorityQueue — two-level queue
// ---------------------------------------------------------------------------

/// Two-level IPC priority queue (RT + NORMAL).
pub struct PriorityQueue {
    pub id: u32,
    /// RT ring (high priority)
    rt_ring: PrioRing<PRIORITY_RT_RING_CAP>,
    /// NORMAL ring (standard priority)
    normal_ring: PrioRing<PRIORITY_NORMAL_RING_CAP>,
    /// Counter for anti-starvation
    normal_reads_since_last_rt: AtomicU32,
    /// Statistics
    pub rt_enqueued: AtomicU64,
    pub normal_enqueued: AtomicU64,
    pub rt_dequeued: AtomicU64,
    pub normal_dequeued: AtomicU64,
    pub rt_degraded: AtomicU64,
    pub rt_full_drops: AtomicU64,
}

// SAFETY: PrioRing is Sync
unsafe impl Sync for PriorityQueue {}
unsafe impl Send for PriorityQueue {}

impl PriorityQueue {
    pub const fn new(id: u32) -> Self {
        Self {
            id,
            rt_ring: PrioRing::new(),
            normal_ring: PrioRing::new(),
            normal_reads_since_last_rt: AtomicU32::new(0),
            rt_enqueued: AtomicU64::new(0),
            normal_enqueued: AtomicU64::new(0),
            rt_dequeued: AtomicU64::new(0),
            normal_dequeued: AtomicU64::new(0),
            rt_degraded: AtomicU64::new(0),
            rt_full_drops: AtomicU64::new(0),
        }
    }

    // -----------------------------------------------------------------------
    // Enqueue
    // -----------------------------------------------------------------------

    /// Enqueues a message. RT messages (priority >= PRIORITY_RT_THRESHOLD)
    /// go to the RT ring; others to the NORMAL ring.
    /// If the RT ring is full, the message is downgraded to NORMAL.
    pub fn enqueue(&self, msg: &IpcMessage) -> Result<(), IpcError> {
        let slot = PrioMsgSlot::from_message(msg);
        if msg.priority >= PRIORITY_RT_THRESHOLD {
            if self.rt_ring.push(slot) {
                self.rt_enqueued.fetch_add(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageSent);
                return Ok(());
            } else {
                // RT ring full: downgrade
                self.rt_degraded.fetch_add(1, Ordering::Relaxed);
                // Re-create slot (consumed above)
                let slot2 = PrioMsgSlot::from_message(msg);
                if self.normal_ring.push(slot2) {
                    self.normal_enqueued.fetch_add(1, Ordering::Relaxed);
                    IPC_STATS.record(StatEvent::MessageSent);
                    return Ok(());
                } else {
                    self.rt_full_drops.fetch_add(1, Ordering::Relaxed);
                    IPC_STATS.record(StatEvent::MessageDropped);
                    return Err(IpcError::Full);
                }
            }
        } else {
            if self.normal_ring.push(slot) {
                self.normal_enqueued.fetch_add(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageSent);
                return Ok(());
            } else {
                IPC_STATS.record(StatEvent::MessageDropped);
                return Err(IpcError::Full);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Dequeue
    // -----------------------------------------------------------------------

    /// Dequeues the next message according to priority policy:
    /// 1. If RT ring not empty AND (anti-starvation counter < RATIO or normal empty) -> RT
    /// 2. Otherwise -> NORMAL
    pub fn dequeue(&self) -> Option<PrioMsgSlot> {
        let normal_count = self.normal_reads_since_last_rt.load(Ordering::Relaxed);
        let rt_empty = self.rt_ring.is_empty();
        let normal_empty = self.normal_ring.is_empty();

        if rt_empty && normal_empty {
            return None;
        }

        // Policy: RT or anti-starvation?
        let take_rt = !rt_empty && (normal_empty || normal_count < PRIORITY_ANTI_STARVATION_RATIO);

        if take_rt {
            if let Some(slot) = self.rt_ring.pop() {
                self.rt_dequeued.fetch_add(1, Ordering::Relaxed);
                self.normal_reads_since_last_rt.store(0, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageReceived);
                return Some(slot);
            }
        }

        // Consume from NORMAL
        if let Some(slot) = self.normal_ring.pop() {
            self.normal_dequeued.fetch_add(1, Ordering::Relaxed);
            self.normal_reads_since_last_rt
                .fetch_add(1, Ordering::Relaxed);
            IPC_STATS.record(StatEvent::MessageReceived);
            return Some(slot);
        }

        // If RT available despite ratio
        if let Some(slot) = self.rt_ring.pop() {
            self.rt_dequeued.fetch_add(1, Ordering::Relaxed);
            self.normal_reads_since_last_rt.store(0, Ordering::Relaxed);
            IPC_STATS.record(StatEvent::MessageReceived);
            return Some(slot);
        }

        None
    }

    /// Non-blocking dequeue attempt (alias of dequeue).
    pub fn try_dequeue(&self) -> Option<PrioMsgSlot> {
        self.dequeue()
    }

    /// Checks if queue is empty.
    pub fn is_empty(&self) -> bool {
        self.rt_ring.is_empty() && self.normal_ring.is_empty()
    }

    /// Total number of messages in both rings.
    pub fn len(&self) -> u32 {
        self.rt_ring.len() + self.normal_ring.len()
    }

    /// Statistics snapshot.
    pub fn snapshot(&self) -> PriorityQueueStats {
        PriorityQueueStats {
            id: self.id,
            rt_pending: self.rt_ring.len(),
            normal_pending: self.normal_ring.len(),
            rt_enqueued: self.rt_enqueued.load(Ordering::Relaxed),
            normal_enqueued: self.normal_enqueued.load(Ordering::Relaxed),
            rt_dequeued: self.rt_dequeued.load(Ordering::Relaxed),
            normal_dequeued: self.normal_dequeued.load(Ordering::Relaxed),
            rt_degraded: self.rt_degraded.load(Ordering::Relaxed),
            rt_full_drops: self.rt_full_drops.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PriorityQueueStats {
    pub id: u32,
    pub rt_pending: u32,
    pub normal_pending: u32,
    pub rt_enqueued: u64,
    pub normal_enqueued: u64,
    pub rt_dequeued: u64,
    pub normal_dequeued: u64,
    pub rt_degraded: u64,
    pub rt_full_drops: u64,
}

// ---------------------------------------------------------------------------
// Global priority queue table
// ---------------------------------------------------------------------------

struct PrioQueueSlot {
    queue: MaybeUninit<PriorityQueue>,
    occupied: AtomicBool,
}

impl PrioQueueSlot {
    const fn empty() -> Self {
        Self {
            queue: MaybeUninit::uninit(),
            occupied: AtomicBool::new(false),
        }
    }
}

struct PriorityQueueTable {
    slots: [PrioQueueSlot; MAX_PRIORITY_QUEUES],
    count: AtomicU32,
}

unsafe impl Sync for PriorityQueueTable {}

impl PriorityQueueTable {
    const fn new() -> Self {
        Self {
            slots: [const { PrioQueueSlot::empty() }; MAX_PRIORITY_QUEUES],
            count: AtomicU32::new(0),
        }
    }

    fn alloc(&self, id: u32) -> Option<usize> {
        for i in 0..MAX_PRIORITY_QUEUES {
            if !self.slots[i].occupied.load(Ordering::Relaxed) {
                if self.slots[i]
                    .occupied
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    // SAFETY: AcqRel CAS guarantees exclusivity; queue MaybeUninit<PriorityQueue> write-once.
                    unsafe {
                        (self.slots[i].queue.as_ptr() as *mut PriorityQueue)
                            .write(PriorityQueue::new(id));
                    }
                    self.count.fetch_add(1, Ordering::Relaxed);
                    return Some(i);
                }
            }
        }
        None
    }

    fn get(&self, idx: usize) -> Option<&PriorityQueue> {
        if idx >= MAX_PRIORITY_QUEUES {
            return None;
        }
        if !self.slots[idx].occupied.load(Ordering::Acquire) {
            return None;
        }
        Some(unsafe { &*self.slots[idx].queue.as_ptr() })
    }

    fn free(&self, idx: usize) -> bool {
        if idx >= MAX_PRIORITY_QUEUES {
            return false;
        }
        if self.slots[idx]
            .occupied
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            self.count.fetch_sub(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

static PRIO_QUEUE_TABLE: PriorityQueueTable = PriorityQueueTable::new();

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn prio_queue_create(id: u32) -> Option<usize> {
    PRIO_QUEUE_TABLE.alloc(id)
}

pub fn prio_queue_enqueue(idx: usize, msg: &IpcMessage) -> Result<(), IpcError> {
    PRIO_QUEUE_TABLE
        .get(idx)
        .ok_or(IpcError::InvalidHandle)?
        .enqueue(msg)
}

pub fn prio_queue_dequeue(idx: usize) -> Option<PrioMsgSlot> {
    PRIO_QUEUE_TABLE.get(idx)?.dequeue()
}

pub fn prio_queue_is_empty(idx: usize) -> Option<bool> {
    PRIO_QUEUE_TABLE.get(idx).map(|q| q.is_empty())
}

pub fn prio_queue_len(idx: usize) -> Option<u32> {
    PRIO_QUEUE_TABLE.get(idx).map(|q| q.len())
}

pub fn prio_queue_destroy(idx: usize) -> bool {
    PRIO_QUEUE_TABLE.free(idx)
}

pub fn prio_queue_stats(idx: usize) -> Option<PriorityQueueStats> {
    PRIO_QUEUE_TABLE.get(idx).map(|q| q.snapshot())
}
