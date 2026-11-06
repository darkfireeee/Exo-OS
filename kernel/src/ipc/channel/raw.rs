// ipc/channel/raw.rs — Raw IPC API for syscall layer (Exo-OS)
//
// Provides `send_raw` / `recv_raw`: bridge between u64 handles of syscall
// exo_ipc_send / exo_ipc_recv and internal IPC channels.
//
// Architecture: static table of MAX_RAW_SLOTS ring-buffer mailboxes.
// Each mailbox is identified by an EndpointId (u64) and contains a
// circular ring of inline messages (no-alloc).
//
// RULE NO-ALLOC: zero Vec/Box/Arc. Everything is static.
// RULE IPC-RAW-01: auto-open on first write (send_raw creates the slot).
// RULE IPC-RAW-02: ring overflow (full slot) increments drop_count.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::ipc::core::constants::MAX_MSG_SIZE;
use crate::ipc::core::types::{alloc_message_id, EndpointId, IpcError, MessageId};
use crate::ipc::stats::counters::{StatEvent, IPC_STATS};
use crate::scheduler::core::task::ThreadControlBlock;
use crate::scheduler::sync::spinlock::SpinLock;
use crate::security::capability::{CapTable, CapToken, Rights};

// ─────────────────────────────────────────────────────────────────────────────
// Sizing
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of concurrent raw mailboxes.
pub const MAX_RAW_SLOTS: usize = 128;

/// Ring depth per mailbox (power of 2).
pub const RAW_RING_DEPTH: usize = 16;

const RAW_RING_MASK: usize = RAW_RING_DEPTH - 1;

const _: () = assert!(
    RAW_RING_DEPTH.is_power_of_two(),
    "RAW_RING_DEPTH doit être une puissance de 2"
);

// ─────────────────────────────────────────────────────────────────────────────
// InnerMsg — a message in the ring (no-alloc)
// ─────────────────────────────────────────────────────────────────────────────

struct InnerMsg {
    len: usize,
    data: [u8; MAX_MSG_SIZE],
}

impl InnerMsg {
    const fn empty() -> Self {
        Self {
            len: 0,
            data: [0u8; MAX_MSG_SIZE],
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// InnerRing — circular ring (under SpinLock)
// ─────────────────────────────────────────────────────────────────────────────

struct InnerRing {
    head: usize,  // read (consumer)
    tail: usize,  // write (producer)
    count: usize, // present messages
    msgs: [InnerMsg; RAW_RING_DEPTH],
}

impl InnerRing {
    const fn empty() -> Self {
        Self {
            head: 0,
            tail: 0,
            count: 0,
            // NOTE: const array init with const function
            msgs: {
                let arr = [const { InnerMsg::empty() }; RAW_RING_DEPTH];
                arr
            },
        }
    }

    /// Enqueues a message. Returns `false` if ring is full.
    #[inline]
    fn enqueue(&mut self, data: &[u8]) -> bool {
        if self.count == RAW_RING_DEPTH {
            return false;
        }
        let len = data.len().min(MAX_MSG_SIZE);
        let slot = &mut self.msgs[self.tail & RAW_RING_MASK];
        slot.len = len;
        slot.data[..len].copy_from_slice(&data[..len]);
        self.tail = self.tail.wrapping_add(1);
        self.count += 1;
        true
    }

    /// Dequeues a message. Returns `None` if empty.
    ///
    /// If calling buffer is too small, message remains at queue head:
    /// no partial dequeue is allowed, avoiding IPC truncation.
    #[inline]
    fn dequeue(&mut self, buf: &mut [u8]) -> Result<Option<usize>, IpcError> {
        if self.count == 0 {
            return Ok(None);
        }
        let slot = &self.msgs[self.head & RAW_RING_MASK];
        if slot.len > buf.len() {
            return Err(IpcError::MessageTooLarge);
        }
        let len = slot.len;
        buf[..len].copy_from_slice(&slot.data[..len]);
        self.head = self.head.wrapping_add(1);
        self.count -= 1;
        Ok(Some(len))
    }

    #[inline(always)]
    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.count == 0
    }

    #[inline(always)]
    fn is_full(&self) -> bool {
        self.count == RAW_RING_DEPTH
    }

    /// Resets ring (used during `mailbox_close`).
    fn reset(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.count = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RawSlot — a mailbox assigned to an endpoint
// ─────────────────────────────────────────────────────────────────────────────

struct RawSlot {
    /// 0 = free slot, >0 = owner endpoint_id.
    endpoint_id: AtomicU64,
    /// Message ring protected by spinlock.
    ring: SpinLock<InnerRing>,
    /// TCB of receiver blocked on this mailbox (0 = none).
    ///
    /// RULE IPC-RAW-03: read/written ONLY under locked `ring`. The
    /// receiver therefore cannot leave `recv_ipc_message` (and free its
    /// TCB) while a sender wakes it up.
    waiter: AtomicUsize,
    /// Statistical counters.
    send_count: AtomicU64,
    recv_count: AtomicU64,
    drop_count: AtomicU64,
}

impl RawSlot {
    const fn new() -> Self {
        Self {
            endpoint_id: AtomicU64::new(0),
            ring: SpinLock::new(InnerRing::empty()),
            waiter: AtomicUsize::new(0),
            send_count: AtomicU64::new(0),
            recv_count: AtomicU64::new(0),
            drop_count: AtomicU64::new(0),
        }
    }

    /// Extracts registered receiver, if any, for wake-up outside lock.
    ///
    /// Must be called with `ring` locked (RULE IPC-RAW-03).
    /// FIX-IPC-LOCKORD-01: former `wake_waiter_locked` acquired runqueue
    /// lock (Scheduler level = 2) while holding IPC lock (level 4),
    /// violating hierarchy documented in ipc/mod.rs (Memory->Scheduler->
    /// Security->IPC) and risking deadlock inversion. Wake-up is now
    /// performed AFTER releasing ring lock.
    #[inline]
    fn take_waiter_locked(&self) -> *mut ThreadControlBlock {
        self.waiter.swap(0, Ordering::AcqRel) as *mut ThreadControlBlock
    }

    /// Wakes up receiver extracted by `take_waiter_locked`.
    /// Call OUTSIDE ring lock. No-op if pointer is null
    /// (receiver was already served or no waiter was registered).
    #[inline]
    fn wake_waiter(waiter: *mut ThreadControlBlock) {
        if !waiter.is_null() {
            // SAFETY: pointer was published by receiver under ring lock
            // and extracted atomically (swap) under the same lock: it
            // remains live until wake-up (receiver cannot free its TCB
            // without re-acquiring lock, and waiter has already been removed).
            unsafe { wake_receiver(waiter) };
        }
    }
}

/// Transition Sleeping -> Runnable + enqueue of blocked IPC receiver.
///
/// If receiver is not (yet) sleeping, call is a no-op: it will
/// re-probe ring at the latest when its bounded sleep expires.
unsafe fn wake_receiver(tcb: *mut ThreadControlBlock) {
    use crate::scheduler::core::preempt::{IrqGuard, MAX_CPUS};
    use crate::scheduler::core::runqueue::run_queue;
    use crate::scheduler::core::task::{CpuId, TaskState};
    use core::ptr::NonNull;

    let Some(nn) = NonNull::new(tcb) else {
        return;
    };
    let t = &*tcb;
    let cpu_raw = t.cpu_id.load(Ordering::Relaxed) as usize;
    if cpu_raw >= MAX_CPUS {
        return;
    }
    let _irq = IrqGuard::new();
    if !t.try_transition(TaskState::Sleeping, TaskState::Runnable) {
        return;
    }
    run_queue(CpuId(cpu_raw as u32)).enqueue(nn);
    let current = crate::scheduler::core::switch::current_thread_raw();
    if !current.is_null() {
        let cur = &*current;
        if cur.current_cpu().0 as usize == cpu_raw {
            cur.request_preemption();
        }
    }
}

// SAFETY: all mutations go through SpinLock or AtomicU64.
unsafe impl Sync for RawSlot {}
unsafe impl Send for RawSlot {}

// ─────────────────────────────────────────────────────────────────────────────
// Static global table
// ─────────────────────────────────────────────────────────────────────────────

static RAW_TABLE: [RawSlot; MAX_RAW_SLOTS] = [const { RawSlot::new() }; MAX_RAW_SLOTS];

/// Number of open mailboxes.
static OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Internal functions
// ─────────────────────────────────────────────────────────────────────────────

/// Internal lookup: returns slot index for `ep_id` or `None`.
#[inline]
fn find_slot(ep_id: u64) -> Option<usize> {
    // Linear probe from ideal position.
    let start = (ep_id as usize).wrapping_mul(2654435761) % MAX_RAW_SLOTS;
    for i in 0..MAX_RAW_SLOTS {
        let idx = (start + i) % MAX_RAW_SLOTS;
        if RAW_TABLE[idx].endpoint_id.load(Ordering::Acquire) == ep_id {
            return Some(idx);
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Mailbox management API
// ─────────────────────────────────────────────────────────────────────────────

/// Opens a mailbox for `ep_id`.
///
/// Idempotent: if a slot already exists for this `ep_id`, returns `true`.
/// Returns `false` if table is full.
pub fn mailbox_open(ep_id: EndpointId) -> bool {
    let id = ep_id.get();
    if id == 0 {
        return false;
    }
    // Idempotence: already open?
    if find_slot(id).is_some() {
        return true;
    }
    // Find a free slot.
    let start = (id as usize).wrapping_mul(2654435761) % MAX_RAW_SLOTS;
    for i in 0..MAX_RAW_SLOTS {
        let idx = (start + i) % MAX_RAW_SLOTS;
        if RAW_TABLE[idx]
            .endpoint_id
            .compare_exchange(0, id, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            OPEN_COUNT.fetch_add(1, Ordering::Relaxed);
            return true;
        }
    }
    false // table full
}

/// Closes mailbox associated with `ep_id` and empties its ring.
pub fn mailbox_close(ep_id: EndpointId) {
    let id = ep_id.get();
    if let Some(idx) = find_slot(id) {
        let slot = &RAW_TABLE[idx];
        let waiter = {
            // T8-007 FIX: lock ring before CAS to guarantee
            // that no in-progress send_raw can enqueue a message during or after reset.
            let mut ring = slot.ring.lock();
            if slot
                .endpoint_id
                .compare_exchange(id, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let w = slot.take_waiter_locked();
                ring.reset();
                OPEN_COUNT.fetch_sub(1, Ordering::Relaxed);
                w
            } else {
                core::ptr::null_mut()
            }
        };
        wake_waiter_outside_lock(waiter);
    }
}

/// Number of currently open mailboxes.
#[inline]
pub fn mailbox_open_count() -> usize {
    OPEN_COUNT.load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// Main API — send_raw / recv_raw
// ─────────────────────────────────────────────────────────────────────────────

/// Sends `data` into mailbox of `ep_id`.
///
/// If slot does not exist yet, it is created automatically (auto-open).
/// `flags`: bitmask
///   - `0x0001` (NOWAIT) — returns `Err(WouldBlock)` if ring is full.
///
/// Returns unique kernel-allocated `MessageId`.
pub fn send_raw(ep_id: EndpointId, data: &[u8], flags: u32) -> Result<MessageId, IpcError> {
    if data.len() > MAX_MSG_SIZE {
        return Err(IpcError::MessageTooLarge);
    }

    let id = ep_id.get();
    if id == 0 {
        return Err(IpcError::NullEndpoint);
    }

    // Auto-open if necessary.
    if find_slot(id).is_none() {
        if !mailbox_open(ep_id) {
            return Err(IpcError::OutOfResources);
        }
    }

    let idx = find_slot(id).ok_or(IpcError::NotFound)?;
    let slot = &RAW_TABLE[idx];
    let nowait = flags & 0x0001 != 0;

    {
        let mut ring = slot.ring.lock();
        // FIX-IPC-RAW-TOCTOU-01: re-verify slot ownership UNDER lock.
        // Without this guard, concurrent `mailbox_close` can free slot
        // (CAS endpoint_id->0) and `mailbox_open` can reassign it to ANOTHER
        // endpoint: message would be deposited in a third party's mailbox
        // (inter-endpoint leak).
        if slot.endpoint_id.load(Ordering::Acquire) != id {
            return Err(IpcError::NotFound);
        }
        if !ring.is_full() {
            ring.enqueue(data);
            slot.send_count.fetch_add(1, Ordering::Relaxed);
            IPC_STATS.record(StatEvent::MessageSent);
            let waiter = slot.take_waiter_locked();
            // Wake-up OUTSIDE lock (FIX-IPC-LOCKORD-01) — see drop(ring) below.
            drop(ring);
            wake_waiter_outside_lock(waiter);
            return Ok(alloc_message_id());
        }
        // Full ring.
        if nowait {
            slot.drop_count.fetch_add(1, Ordering::Relaxed);
            return Err(IpcError::WouldBlock);
        }
    }

    // Short spin wait with lock release.
    let mut spins: u32 = 0;
    loop {
        core::hint::spin_loop();
        spins = spins.saturating_add(1);
        let mut ring = slot.ring.lock();
        // FIX-IPC-RAW-TOCTOU-01: same guard in wait loop.
        if slot.endpoint_id.load(Ordering::Acquire) != id {
            return Err(IpcError::NotFound);
        }
        if ring.enqueue(data) {
            slot.send_count.fetch_add(1, Ordering::Relaxed);
            IPC_STATS.record(StatEvent::MessageSent);
            let waiter = slot.take_waiter_locked();
            drop(ring);
            wake_waiter_outside_lock(waiter);
            return Ok(alloc_message_id());
        }
        if (spins & 0x7F) == 0 {
            drop(ring);
            unsafe {
                let _ = crate::scheduler::core::switch::cooperative_reschedule();
            }
        } else {
            drop(ring);
        }
        if spins > 200_000 {
            slot.drop_count.fetch_add(1, Ordering::Relaxed);
            return Err(IpcError::Timeout);
        }
    }
}

/// Free wrapper to wake a waiter outside lock (FIX-IPC-LOCKORD-01).
#[inline]
fn wake_waiter_outside_lock(waiter: *mut ThreadControlBlock) {
    RawSlot::wake_waiter(waiter);
}

/// Strictly non-blocking variant of `send_raw`.
///
/// Constraints:
/// - never spins on ring lock;
/// - returns `WouldBlock` if slot is busy or ring is full;
/// - suited for IRQ/ISR contexts that must remain bounded.
pub fn try_send_raw_nowait(ep_id: EndpointId, data: &[u8]) -> Result<MessageId, IpcError> {
    if data.len() > MAX_MSG_SIZE {
        return Err(IpcError::MessageTooLarge);
    }

    let id = ep_id.get();
    if id == 0 {
        return Err(IpcError::NullEndpoint);
    }

    if find_slot(id).is_none() && !mailbox_open(ep_id) {
        return Err(IpcError::OutOfResources);
    }

    let idx = find_slot(id).ok_or(IpcError::NotFound)?;
    let slot = &RAW_TABLE[idx];
    let Some(mut ring) = slot.ring.try_lock() else {
        slot.drop_count.fetch_add(1, Ordering::Relaxed);
        return Err(IpcError::WouldBlock);
    };

    // FIX-IPC-RAW-TOCTOU-01: ownership re-verification under lock.
    if slot.endpoint_id.load(Ordering::Acquire) != id {
        return Err(IpcError::NotFound);
    }

    if !ring.is_full() {
        ring.enqueue(data);
        slot.send_count.fetch_add(1, Ordering::Relaxed);
        IPC_STATS.record(StatEvent::MessageSent);
        let waiter = slot.take_waiter_locked();
        drop(ring);
        wake_waiter_outside_lock(waiter);
        return Ok(alloc_message_id());
    }

    slot.drop_count.fetch_add(1, Ordering::Relaxed);
    Err(IpcError::WouldBlock)
}

/// Attempts to receive a message; if ring is empty and TCB is provided,
/// atomically registers `waiter_tcb` as waiter under ring lock.
pub fn recv_raw_or_register_waiter(
    ep_id: EndpointId,
    buf: &mut [u8],
    waiter_tcb: *mut ThreadControlBlock,
) -> Result<Option<usize>, IpcError> {
    let id = ep_id.get();
    if id == 0 {
        return Err(IpcError::NullEndpoint);
    }
    let idx = find_slot(id).ok_or(IpcError::NotFound)?;
    let slot = &RAW_TABLE[idx];
    let mut ring = slot.ring.lock();
    // FIX-IPC-RAW-TOCTOU-01: ownership re-verification under lock
    // (avoids reading message from previous endpoint on a reassigned slot).
    if slot.endpoint_id.load(Ordering::Acquire) != id {
        return Err(IpcError::NotFound);
    }
    match ring.dequeue(buf) {
        Ok(Some(n)) => {
            slot.recv_count.fetch_add(1, Ordering::Relaxed);
            IPC_STATS.record(StatEvent::MessageReceived);
            slot.waiter.store(0, Ordering::Release);
            Ok(Some(n))
        }
        Ok(None) => {
            if !waiter_tcb.is_null() {
                slot.waiter.store(waiter_tcb as usize, Ordering::Release);
            }
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

/// Clears registered waiter on mailbox if it matches `tcb`.
pub fn clear_waiter(ep_id: EndpointId, tcb: *mut ThreadControlBlock) {
    let id = ep_id.get();
    if let Some(idx) = find_slot(id) {
        let slot = &RAW_TABLE[idx];
        let mut _ring = slot.ring.lock();
        let _ = slot.waiter.compare_exchange(
            tcb as usize,
            0,
            Ordering::AcqRel,
            Ordering::Relaxed,
        );
    }
}

/// Receives a message from `ep_id` mailbox into `buf`.
///
/// `flags`:
///   - `0x0001` (NOWAIT) — returns `Err(WouldBlock)` if no message.
///
/// Returns number of bytes copied into `buf`.
pub fn recv_raw(ep_id: EndpointId, buf: &mut [u8], flags: u32) -> Result<usize, IpcError> {
    let id = ep_id.get();
    if id == 0 {
        return Err(IpcError::NullEndpoint);
    }

    let idx = find_slot(id).ok_or(IpcError::NotFound)?;
    let slot = &RAW_TABLE[idx];
    let nowait = flags & 0x0001 != 0;

    {
        let mut ring = slot.ring.lock();
        // FIX-IPC-RAW-TOCTOU-01: ownership re-verification under lock.
        if slot.endpoint_id.load(Ordering::Acquire) != id {
            return Err(IpcError::NotFound);
        }
        match ring.dequeue(buf) {
            Ok(Some(n)) => {
                slot.recv_count.fetch_add(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageReceived);
                return Ok(n);
            }
            Ok(None) => {}
            Err(err) => return Err(err),
        }
        if nowait {
            return Err(IpcError::WouldBlock);
        }
    }

    // Spin wait.
    let mut spins: u32 = 0;
    loop {
        core::hint::spin_loop();
        spins = spins.saturating_add(1);
        let mut ring = slot.ring.lock();
        // FIX-IPC-RAW-TOCTOU-01: guard in wait loop as well.
        if slot.endpoint_id.load(Ordering::Acquire) != id {
            return Err(IpcError::NotFound);
        }
        match ring.dequeue(buf) {
            Ok(Some(n)) => {
                slot.recv_count.fetch_add(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageReceived);
                return Ok(n);
            }
            Ok(None) => {}
            Err(err) => return Err(err),
        }
        if (spins & 0x7F) == 0 {
            drop(ring);
            unsafe {
                let _ = crate::scheduler::core::switch::cooperative_reschedule();
            }
        } else {
            drop(ring);
        }
        if spins > 200_000 {
            return Err(IpcError::Timeout);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistics
// ─────────────────────────────────────────────────────────────────────────────

/// Statistics for an active mailbox.
#[derive(Copy, Clone, Debug, Default)]
pub struct RawSlotStats {
    pub endpoint_id: u64,
    pub send_count: u64,
    pub recv_count: u64,
    pub drop_count: u64,
}

/// Statistics snapshot of all active mailboxes.
/// Returns an array of `MAX_RAW_SLOTS` elements (`None` = free slot).
pub fn raw_stats_snapshot() -> [Option<RawSlotStats>; MAX_RAW_SLOTS] {
    let mut out = [None; MAX_RAW_SLOTS];
    for i in 0..MAX_RAW_SLOTS {
        let ep = RAW_TABLE[i].endpoint_id.load(Ordering::Relaxed);
        if ep != 0 {
            out[i] = Some(RawSlotStats {
                endpoint_id: ep,
                send_count: RAW_TABLE[i].send_count.load(Ordering::Relaxed),
                recv_count: RAW_TABLE[i].recv_count.load(Ordering::Relaxed),
                drop_count: RAW_TABLE[i].drop_count.load(Ordering::Relaxed),
            });
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Capability-checked API — IPC-04 (v6): direct security/access_control/ call
// ─────────────────────────────────────────────────────────────────────────────

/// Sends to `ep_id` mailbox with capability check (RULE IPC-04 v6).
///
/// Identical to `send_raw()` but checks `Rights::IPC_SEND` via
/// `security::access_control::check_access()` before any operation.
/// Used by syscall layer (calls from userspace).
/// Internal kernel calls can use `send_raw()` directly.
pub fn send_raw_checked(
    ep_id: EndpointId,
    data: &[u8],
    flags: u32,
    table: &CapTable,
    token: CapToken,
) -> Result<MessageId, IpcError> {
    crate::ipc::capability_bridge::check_channel_access(table, token, Rights::IPC_SEND)?;
    send_raw(ep_id, data, flags)
}

/// Receives from `ep_id` mailbox with capability check (RULE IPC-04 v6).
///
/// # Required rights: `Rights::IPC_RECV`
pub fn recv_raw_checked(
    ep_id: EndpointId,
    buf: &mut [u8],
    flags: u32,
    table: &CapTable,
    token: CapToken,
) -> Result<usize, IpcError> {
    crate::ipc::capability_bridge::check_channel_access(table, token, Rights::IPC_RECV)?;
    recv_raw(ep_id, buf, flags)
}

#[cfg(test)]
mod tests {
    use super::{mailbox_close, mailbox_open, recv_raw, send_raw};
    use crate::ipc::core::types::EndpointId;

    #[test]
    fn test_raw_mailbox_open_send_recv_roundtrip() {
        let ep = EndpointId::new(5).unwrap();
        mailbox_close(ep);
        assert!(mailbox_open(ep));

        let payload = *b"exo-ipc";
        send_raw(ep, &payload, 0).expect("send raw");

        let mut out = [0u8; 32];
        let n = recv_raw(ep, &mut out, 0x0001).expect("recv raw");
        assert_eq!(&out[..n], &payload);

        mailbox_close(ep);
    }

    #[test]
    fn test_raw_mailbox_send_recv_stress() {
        let ep = EndpointId::new(9).unwrap();
        mailbox_close(ep);
        assert!(mailbox_open(ep));

        for round in 0..1024u16 {
            let payload = [
                (round & 0xFF) as u8,
                (round >> 8) as u8,
                (round as u8).wrapping_mul(3),
                (round as u8).wrapping_mul(7),
            ];
            send_raw(ep, &payload, 0).expect("send raw stress");

            let mut out = [0u8; 32];
            let n = recv_raw(ep, &mut out, 0x0001).expect("recv raw stress");
            assert_eq!(n, payload.len());
            assert_eq!(&out[..n], &payload);
        }

        mailbox_close(ep);
    }

    #[test]
    fn test_raw_recv_too_small_preserves_message() {
        let ep = EndpointId::new(10).unwrap();
        mailbox_close(ep);
        assert!(mailbox_open(ep));

        let payload = *b"oversized-for-small-buffer";
        send_raw(ep, &payload, 0).expect("send raw");

        let mut tiny = [0u8; 4];
        assert_eq!(
            recv_raw(ep, &mut tiny, 0x0001),
            Err(crate::ipc::core::types::IpcError::MessageTooLarge)
        );

        let mut out = [0u8; 64];
        let n = recv_raw(ep, &mut out, 0x0001).expect("recv preserved raw");
        assert_eq!(&out[..n], &payload);

        mailbox_close(ep);
    }
}
