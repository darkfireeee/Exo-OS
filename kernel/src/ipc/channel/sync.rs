// ipc/channel/sync.rs — Synchronous channel (rendezvous) for Exo-OS kernel IPC
//
// This channel implements the rendezvous pattern: the sender is blocked until
// the receiver explicitly accepts the message. Strong delivery guarantee:
// no message can be lost (no buffer queue). Used for critical synchronous calls
// where reception confirmation is required.
//
// Features:
//   - Optional zero-copy via ZeroCopyRef
//   - Configurable timeout (wall-clock delay in nanoseconds)
//   - WaitQueue scheduler integration for efficient suspension
//   - No allocation: everything is static or on the stack

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::ipc::core::constants::{MAX_MSG_SIZE, SYNC_CHANNEL_TIMEOUT_NS};
use crate::ipc::core::types::{
    alloc_channel_id, alloc_message_id, ChannelId, IpcError, MessageId, MsgFlags,
};
use crate::ipc::ring::zerocopy::ZeroCopyRef;
use crate::ipc::stats::counters::{StatEvent, IPC_STATS};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::security::capability::{CapTable, CapToken, Rights};

// ---------------------------------------------------------------------------
// Internal states of an ongoing rendezvous
// ---------------------------------------------------------------------------

/// Rendezvous protocol phase
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum RendezVousState {
    /// Empty channel, nobody waiting
    Idle = 0,
    /// A sender is currently writing (exclusive slot reservation, T8-005)
    SenderWriting = 1,
    /// A sender deposited a message and awaits the receiver
    SenderWaiting = 2,
    /// The receiver took the message, signaling the sender
    ReceiverAcked = 3,
    /// Timeout or cancellation
    Cancelled = 4,
}

impl RendezVousState {
    fn from_u32(v: u32) -> Self {
        match v {
            0 => Self::Idle,
            1 => Self::SenderWriting,
            2 => Self::SenderWaiting,
            3 => Self::ReceiverAcked,
            4 => Self::Cancelled,
            _ => Self::Cancelled,
        }
    }
}

#[inline]
fn monotonic_ns() -> u64 {
    crate::scheduler::timer::clock::monotonic_ns()
}

#[inline]
fn deadline_from(timeout_ns: u64) -> u64 {
    if timeout_ns == 0 {
        u64::MAX
    } else {
        monotonic_ns().saturating_add(timeout_ns)
    }
}

#[inline]
fn deadline_expired(deadline_ns: u64) -> bool {
    deadline_ns != u64::MAX && monotonic_ns() >= deadline_ns
}

#[inline]
fn wake_tid(tid: u64) {
    if tid != 0 {
        crate::ipc::sync::sched_hooks::wake_thread(tid);
    }
}

// ---------------------------------------------------------------------------
// Rendezvous slot — inline message storage (no-alloc)
// ---------------------------------------------------------------------------

/// Maximum inline payload size for a synchronous channel
pub const SYNC_INLINE_SIZE: usize = MAX_MSG_SIZE;

/// Inline slot shared between sender and receiver.
/// Protected by an AtomicU32 state for mutex-free fast-path access.
#[repr(C, align(64))]
pub struct SyncSlot {
    /// Current rendezvous state (encoded RendezVousState)
    pub state: AtomicU32,
    /// Sequence number to detect ABA
    pub sequence: AtomicU64,
    /// Length of payload copied into `data`
    pub payload_len: AtomicUsize,
    /// Message flags
    pub flags: AtomicU32,
    /// Zero-copy reference (valid if ZEROCOPY flag is set)
    pub zc_ref: UnsafeCell<ZeroCopyRef>,
    /// Inline payload
    pub data: UnsafeCell<[u8; SYNC_INLINE_SIZE]>,
    /// Message identifier for correlation
    pub msg_id: AtomicU64,
    /// Deposit timestamp (ns since boot)
    pub timestamp_ns: AtomicU64,
    /// Blocked sender TID (for wake-up by receiver)
    pub sender_tid: AtomicU64,
    /// Blocked receiver TID (for wake-up by sender)
    pub receiver_tid: AtomicU64,
}

// SAFETY: SyncSlot is a shared data structure between threads but
// all mutable field accesses go through atomics or are protected
// by the state protocol semantics (strict state machine).
unsafe impl Sync for SyncSlot {}
unsafe impl Send for SyncSlot {}

impl SyncSlot {
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(RendezVousState::Idle as u32),
            sequence: AtomicU64::new(0),
            payload_len: AtomicUsize::new(0),
            flags: AtomicU32::new(0),
            zc_ref: UnsafeCell::new(ZeroCopyRef::null()),
            data: UnsafeCell::new([0u8; SYNC_INLINE_SIZE]),
            msg_id: AtomicU64::new(0),
            timestamp_ns: AtomicU64::new(0),
            sender_tid: AtomicU64::new(0),
            receiver_tid: AtomicU64::new(0),
        }
    }

    /// Resets slot to Idle for next rendezvous.
    /// SAFETY: must be called only when slot is free (no race).
    pub unsafe fn reset(&self) {
        self.payload_len.store(0, Ordering::Relaxed);
        self.flags.store(0, Ordering::Relaxed);
        self.msg_id.store(0, Ordering::Relaxed);
        self.timestamp_ns.store(0, Ordering::Relaxed);
        self.sender_tid.store(0, Ordering::Relaxed);
        self.receiver_tid.store(0, Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);
        self.state
            .store(RendezVousState::Idle as u32, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// Local statistics for synchronous channel
// ---------------------------------------------------------------------------

/// Performance counters specific to a SyncChannel
#[repr(C, align(64))]
pub struct SyncChannelStats {
    pub sends_ok: AtomicU64,
    pub sends_timeout: AtomicU64,
    pub sends_cancelled: AtomicU64,
    pub recvs_ok: AtomicU64,
    pub recvs_would_block: AtomicU64,
    pub total_bytes: AtomicU64,
    _pad: [u8; 16],
}

impl SyncChannelStats {
    pub const fn new() -> Self {
        Self {
            sends_ok: AtomicU64::new(0),
            sends_timeout: AtomicU64::new(0),
            sends_cancelled: AtomicU64::new(0),
            recvs_ok: AtomicU64::new(0),
            recvs_would_block: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            _pad: [0u8; 16],
        }
    }
}

// ---------------------------------------------------------------------------
// SyncChannel — main structure
// ---------------------------------------------------------------------------

/// Synchronous channel (rendezvous) between exactly one sender and one receiver.
///
/// Two-phase protocol:
///  1. `send()`: copies message into `slot`, transitions to `SenderWaiting`,
///     then spin loops (or suspends via scheduler) until `ReceiverAcked`.
///  2. `recv()`: waits for `SenderWaiting`, copies from `slot`,
///     transitions to `ReceiverAcked` to unblock sender.
#[repr(C, align(64))]
pub struct SyncChannel {
    /// Unique channel identifier
    pub id: ChannelId,
    /// Shared data slot
    pub slot: SyncSlot,
    /// Local statistics
    pub stats: SyncChannelStats,
    /// Open or closed channel
    pub closed: AtomicU32,
    /// Send timeout in nanoseconds (0 = infinite)
    pub send_timeout_ns: AtomicU64,
    /// Active operations counter to prevent UAF on destroy (T8-002)
    pub active_ops: AtomicU32,
    _pad: [u8; 20],
}

// SAFETY: ChannelId is Copy, SyncSlot is Sync+Send, AtomicU32 is Sync+Send.
unsafe impl Sync for SyncChannel {}
unsafe impl Send for SyncChannel {}

impl SyncChannel {
    /// Creates a new synchronous channel with an allocated unique identifier.
    pub fn new() -> Self {
        Self {
            id: alloc_channel_id(),
            slot: SyncSlot::new(),
            stats: SyncChannelStats::new(),
            closed: AtomicU32::new(0),
            send_timeout_ns: AtomicU64::new(SYNC_CHANNEL_TIMEOUT_NS),
            active_ops: AtomicU32::new(0),
            _pad: [0u8; 20],
        }
    }

    /// Returns `true` if channel is closed.
    #[inline]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) != 0
    }

    /// Closes channel — wakes up any blocked sender/receiver with Cancelled.
    pub fn close(&self) {
        self.closed.store(1, Ordering::Release);
        let sender_tid = self.slot.sender_tid.load(Ordering::Relaxed);
        let receiver_tid = self.slot.receiver_tid.load(Ordering::Relaxed);
        // Signal any pending wait
        let state = self.slot.state.load(Ordering::Acquire);
        if RendezVousState::from_u32(state) == RendezVousState::SenderWaiting {
            self.slot
                .state
                .store(RendezVousState::Cancelled as u32, Ordering::Release);
        }
        wake_tid(sender_tid);
        wake_tid(receiver_tid);
    }

    // -----------------------------------------------------------------------
    // SEND — sender path
    // -----------------------------------------------------------------------

    /// Sends a message and awaits receiver confirmation (rendezvous).
    ///
    /// # Errors
    /// - `IpcError::Closed` — channel closed
    /// - `IpcError::Timeout` — timeout exceeded without receiver
    /// - `IpcError::WouldBlock` — NOWAIT set and no receiver ready
    /// - `IpcError::MessageTooLarge` — payload > MAX_MSG_SIZE
    pub fn send(&self, data: &[u8], flags: MsgFlags) -> Result<MessageId, IpcError> {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }

        if data.len() > SYNC_INLINE_SIZE {
            return Err(IpcError::MessageTooLarge);
        }

        // T8-005 FIX: Wait and exclusive reservation via atomic CAS Idle -> SenderWriting
        let timeout_ns = self.send_timeout_ns.load(Ordering::Relaxed);
        let deadline_ns = deadline_from(timeout_ns);
        let mut spins: u32 = 0;
        loop {
            if self.is_closed() {
                return Err(IpcError::Closed);
            }
            if self.slot.state.compare_exchange_weak(
                RendezVousState::Idle as u32,
                RendezVousState::SenderWriting as u32,
                Ordering::AcqRel,
                Ordering::Acquire,
            ).is_ok() {
                break;
            }
            if flags.contains(MsgFlags::NOWAIT) {
                return Err(IpcError::WouldBlock);
            }
            if deadline_expired(deadline_ns) {
                self.stats.sends_timeout.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }
            core::hint::spin_loop();
            spins += 1;
            if spins > 64 {
                unsafe {
                    let _ = crate::scheduler::core::switch::cooperative_reschedule();
                }
                spins = 0;
            }
        }

        // Deposit message into slot
        let mid = alloc_message_id();
        self.slot.msg_id.store(mid.get(), Ordering::Relaxed);
        self.slot.payload_len.store(data.len(), Ordering::Relaxed);
        self.slot.flags.store(flags.bits(), Ordering::Relaxed);

        // SAFETY: only sender writes here (reserved SenderWriting state)
        unsafe {
            let dst = self.slot.data.get() as *mut u8;
            core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        }

        // Transition SenderWriting -> SenderWaiting (Release to make writes visible)
        // Record sender TID before transition so receiver can wake us up after taking the message.
        let my_tid = crate::ipc::sync::sched_hooks::current_tid();
        self.slot.sender_tid.store(my_tid, Ordering::Relaxed);
        self.slot
            .state
            .store(RendezVousState::SenderWaiting as u32, Ordering::Release);
        wake_tid(self.slot.receiver_tid.load(Ordering::Relaxed));

        // Wait for ReceiverAcked or Cancelled.
        // Strategy: short spin (<= 64 rounds) then real block via scheduler.
        let timeout_ns = self.send_timeout_ns.load(Ordering::Relaxed);
        let deadline_ns = deadline_from(timeout_ns);
        let mut spins: u64 = 0;

        loop {
            core::hint::spin_loop();
            spins += 1;
            let s = RendezVousState::from_u32(self.slot.state.load(Ordering::Acquire));
            match s {
                RendezVousState::ReceiverAcked => {
                    // SAFETY: slot.state == ReceiverAcked, only sender resets here
                    unsafe { self.slot.reset() };
                    self.stats.sends_ok.fetch_add(1, Ordering::Relaxed);
                    self.stats
                        .total_bytes
                        .fetch_add(data.len() as u64, Ordering::Relaxed);
                    IPC_STATS.record(StatEvent::MessageSent);
                    return Ok(mid);
                }
                RendezVousState::Cancelled => {
                    // SAFETY: Cancelled state — only sender resets here.
                    unsafe { self.slot.reset() };
                    self.stats.sends_cancelled.fetch_add(1, Ordering::Relaxed);
                    return Err(IpcError::Closed);
                }
                _ => {}
            }

            if self.is_closed() {
                self.slot
                    .state
                    .store(RendezVousState::Idle as u32, Ordering::Release);
                self.stats.sends_cancelled.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Closed);
            }

            if deadline_expired(deadline_ns) {
                let _ = self.slot.state.compare_exchange(
                    RendezVousState::SenderWaiting as u32,
                    RendezVousState::Cancelled as u32,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                wake_tid(self.slot.receiver_tid.load(Ordering::Relaxed));
                self.stats.sends_timeout.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }

            // After short spin phase: real block.
            if spins == 64 {
                // Check one last time before blocking.
                let s2 = RendezVousState::from_u32(self.slot.state.load(Ordering::Acquire));
                if s2 == RendezVousState::ReceiverAcked || s2 == RendezVousState::Cancelled {
                    continue; // Will be handled on next iteration
                }
                if timeout_ns == 0 {
                    // RULE PREEMPT-BLOCK (B6): blocking with PreemptGuard active = deadlock.
                    debug_assert!(
                        crate::scheduler::core::preempt::PreemptGuard::depth() == 0,
                        "SyncChannel::send (attente ack): block_current() avec PreemptGuard actif"
                    );
                    // SAFETY: block_current() safe if PreemptGuard::depth() == 0.
                    unsafe {
                        crate::ipc::sync::sched_hooks::block_current(my_tid);
                    }
                } else {
                    unsafe {
                        let _ = crate::scheduler::core::switch::cooperative_reschedule();
                    }
                }
                spins = 0;
            }
        }
    }

    /// Zero-copy send: shares a physical page via ZeroCopyRef.
    pub fn send_zerocopy(
        &self,
        zc_ref: ZeroCopyRef,
        flags: MsgFlags,
    ) -> Result<MessageId, IpcError> {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }

        // T8-006 FIX: Wait and exclusive reservation via atomic CAS Idle -> SenderWriting
        let timeout_ns = self.send_timeout_ns.load(Ordering::Relaxed);
        let deadline_ns = deadline_from(timeout_ns);
        let mut spins: u32 = 0;
        loop {
            if self.is_closed() {
                return Err(IpcError::Closed);
            }
            if self.slot.state.compare_exchange_weak(
                RendezVousState::Idle as u32,
                RendezVousState::SenderWriting as u32,
                Ordering::AcqRel,
                Ordering::Acquire,
            ).is_ok() {
                break;
            }
            if flags.contains(MsgFlags::NOWAIT) {
                return Err(IpcError::WouldBlock);
            }
            if deadline_expired(deadline_ns) {
                self.stats.sends_timeout.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }
            core::hint::spin_loop();
            spins += 1;
            if spins > 64 {
                unsafe {
                    let _ = crate::scheduler::core::switch::cooperative_reschedule();
                }
                spins = 0;
            }
        }

        let mid = alloc_message_id();
        self.slot.msg_id.store(mid.get(), Ordering::Relaxed);
        self.slot.payload_len.store(0, Ordering::Relaxed);
        let mut f = flags;
        f.insert(MsgFlags::ZEROCOPY);
        self.slot.flags.store(f.bits(), Ordering::Relaxed);

        // SAFETY: only sender writes zc_ref here (reserved SenderWriting state)
        unsafe {
            *self.slot.zc_ref.get() = zc_ref;
        }

        // Record TID before transition for wake-up by receiver.
        let my_tid = crate::ipc::sync::sched_hooks::current_tid();
        self.slot.sender_tid.store(my_tid, Ordering::Relaxed);
        self.slot
            .state
            .store(RendezVousState::SenderWaiting as u32, Ordering::Release);
        wake_tid(self.slot.receiver_tid.load(Ordering::Relaxed));

        let timeout_ns = self.send_timeout_ns.load(Ordering::Relaxed);
        let deadline_ns = deadline_from(timeout_ns);
        let mut spins: u64 = 0;
        loop {
            core::hint::spin_loop();
            spins += 1;
            let s = RendezVousState::from_u32(self.slot.state.load(Ordering::Acquire));
            if s == RendezVousState::ReceiverAcked {
                // SAFETY: ReceiverAcked — only sender resets (1-sender protocol).
                unsafe { self.slot.reset() };
                self.stats.sends_ok.fetch_add(1, Ordering::Relaxed);
                IPC_STATS.record(StatEvent::MessageSent);
                return Ok(mid);
            }
            if s == RendezVousState::Cancelled || self.is_closed() {
                // SAFETY: Cancelled / closed — same owner invariant as above.
                unsafe { self.slot.reset() };
                return Err(IpcError::Closed);
            }
            if deadline_expired(deadline_ns) {
                let _ = self.slot.state.compare_exchange(
                    RendezVousState::SenderWaiting as u32,
                    RendezVousState::Cancelled as u32,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                wake_tid(self.slot.receiver_tid.load(Ordering::Relaxed));
                return Err(IpcError::Timeout);
            }
            // Real blocking after 64 spin iterations.
            if spins == 64 {
                let s2 = RendezVousState::from_u32(self.slot.state.load(Ordering::Acquire));
                if s2 != RendezVousState::SenderWaiting {
                    continue;
                }
                if timeout_ns == 0 {
                    // RULE PREEMPT-BLOCK (B6): blocking with PreemptGuard active = deadlock.
                    debug_assert!(
                        crate::scheduler::core::preempt::PreemptGuard::depth() == 0,
                        "SyncChannel::send_zerocopy: block_current() avec PreemptGuard actif"
                    );
                    // SAFETY: block_current() safe if depth()==0 (debug_assert above, send_zerocopy).
                    unsafe {
                        crate::ipc::sync::sched_hooks::block_current(my_tid);
                    }
                } else {
                    unsafe {
                        let _ = crate::scheduler::core::switch::cooperative_reschedule();
                    }
                }
                spins = 0;
            }
        }
    }

    // -----------------------------------------------------------------------
    // RECV — receiver path
    // -----------------------------------------------------------------------

    /// Receives a message from the channel.
    ///
    /// # Return
    /// - `Ok((MessageId, copied_length, MsgFlags))` — message received
    /// - `Err(IpcError::WouldBlock)` — no sender ready (if NOWAIT)
    /// - `Err(IpcError::Closed)` — channel closed
    pub fn recv(
        &self,
        buf: &mut [u8],
        flags: MsgFlags,
    ) -> Result<(MessageId, usize, MsgFlags), IpcError> {
        if self.is_closed() {
            return Err(IpcError::Closed);
        }

        // Wait for a sender to be present.
        // Strategy: short spin (<= 64) then real block.
        if flags.contains(MsgFlags::NOWAIT) {
            let s = RendezVousState::from_u32(self.slot.state.load(Ordering::Acquire));
            if s != RendezVousState::SenderWaiting {
                self.stats.recvs_would_block.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::WouldBlock);
            }
        } else {
            let my_tid = crate::ipc::sync::sched_hooks::current_tid();
            self.slot.receiver_tid.store(my_tid, Ordering::Relaxed);
            let mut spins: u32 = 0;
            loop {
                let s = RendezVousState::from_u32(self.slot.state.load(Ordering::Acquire));
                if s == RendezVousState::SenderWaiting {
                    break;
                }
                if self.is_closed() {
                    return Err(IpcError::Closed);
                }
                core::hint::spin_loop();
                spins += 1;
                if spins == 64 {
                    // RULE PREEMPT-BLOCK (B6): blocking with PreemptGuard active = deadlock.
                    debug_assert!(
                        crate::scheduler::core::preempt::PreemptGuard::depth() == 0,
                        "SyncChannel::recv: block_current() appelé avec PreemptGuard actif"
                    );
                    // SAFETY: block_current() safe if depth()==0 (debug_assert above, recv path).
                    unsafe {
                        crate::ipc::sync::sched_hooks::block_current(my_tid);
                    }
                    spins = 0;
                }
                if spins > 2_000_000 {
                    return Err(IpcError::Timeout);
                }
            }
        }

        // --- Read metadata (Acquire ensures visibility of sender's previous store) ---
        let msg_flags_raw = self.slot.flags.load(Ordering::Acquire);
        let msg_flags = MsgFlags::from_bits_truncate(msg_flags_raw);
        let len = self.slot.payload_len.load(Ordering::Relaxed);
        let mid_raw = self.slot.msg_id.load(Ordering::Relaxed);

        // SAFETY: NonZeroU64 guaranteed by alloc_message_id — sender wrote
        // msg_id before transition to SenderWaiting.
        let mid = unsafe { MessageId::new_unchecked(mid_raw) };

        if msg_flags.contains(MsgFlags::ZEROCOPY) {
            // Zero-copy mode: return reference without copying data
            let copy_len = 0usize;
            let sender_tid = self.slot.sender_tid.load(Ordering::Relaxed);
            self.slot
                .state
                .store(RendezVousState::ReceiverAcked as u32, Ordering::Release);
            // Wake sender if blocked.
            wake_tid(sender_tid);
            self.stats.recvs_ok.fetch_add(1, Ordering::Relaxed);
            IPC_STATS.record(StatEvent::MessageReceived);
            return Ok((mid, copy_len, msg_flags));
        }

        // Check that destination buffer is sufficient
        if len > buf.len() {
            return Err(IpcError::MessageTooLarge);
        }

        // Inline copy
        let copy_len = len;
        if copy_len > 0 {
            // SAFETY: sender completed before SenderWaiting transition (Release/Acquire); single receiver.
            unsafe {
                let src = self.slot.data.get() as *const u8;
                core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), copy_len);
            }
        }

        // Signal sender — transition to ReceiverAcked.
        let sender_tid = self.slot.sender_tid.load(Ordering::Relaxed);
        self.slot
            .state
            .store(RendezVousState::ReceiverAcked as u32, Ordering::Release);
        // Wake sender if blocked in scheduler.
        wake_tid(sender_tid);

        self.stats.recvs_ok.fetch_add(1, Ordering::Relaxed);
        self.stats
            .total_bytes
            .fetch_add(copy_len as u64, Ordering::Relaxed);
        IPC_STATS.record(StatEvent::MessageReceived);
        Ok((mid, copy_len, msg_flags))
    }

    /// Receives a zero-copy reference from channel.
    /// Returns `Some(ZeroCopyRef)` if current message is in ZC mode.
    pub fn recv_zerocopy(&self) -> Option<ZeroCopyRef> {
        let s = RendezVousState::from_u32(self.slot.state.load(Ordering::Acquire));
        if s != RendezVousState::SenderWaiting {
            return None;
        }
        let msg_flags_raw = self.slot.flags.load(Ordering::Relaxed);
        let msg_flags = MsgFlags::from_bits_truncate(msg_flags_raw);
        if !msg_flags.contains(MsgFlags::ZEROCOPY) {
            return None;
        }
        // SAFETY: SenderWaiting state and ZEROCOPY flag — sender wrote zc_ref
        let zc = unsafe { *self.slot.zc_ref.get() };
        let sender_tid = self.slot.sender_tid.load(Ordering::Relaxed);
        self.slot
            .state
            .store(RendezVousState::ReceiverAcked as u32, Ordering::Release);
        wake_tid(sender_tid);
        self.stats.recvs_ok.fetch_add(1, Ordering::Relaxed);
        IPC_STATS.record(StatEvent::MessageReceived);
        Some(zc)
    }

    // -----------------------------------------------------------------------
    // Utilities
    // -----------------------------------------------------------------------

    /// Returns `true` if a sender is waiting for rendezvous.
    #[inline]
    pub fn has_pending_sender(&self) -> bool {
        RendezVousState::from_u32(self.slot.state.load(Ordering::Acquire))
            == RendezVousState::SenderWaiting
    }

    /// Statistics snapshot (atomic copy of counters).
    pub fn snapshot_stats(&self) -> SyncChannelSnapshot {
        SyncChannelSnapshot {
            sends_ok: self.stats.sends_ok.load(Ordering::Relaxed),
            sends_timeout: self.stats.sends_timeout.load(Ordering::Relaxed),
            sends_cancelled: self.stats.sends_cancelled.load(Ordering::Relaxed),
            recvs_ok: self.stats.recvs_ok.load(Ordering::Relaxed),
            recvs_would_block: self.stats.recvs_would_block.load(Ordering::Relaxed),
            total_bytes: self.stats.total_bytes.load(Ordering::Relaxed),
        }
    }

    // -----------------------------------------------------------------------
    // CAP-CHECKED SEND — IPC-04 (v6): direct security::access_control call
    // -----------------------------------------------------------------------

    /// Sends with capability check (RULE IPC-04 v6).
    ///
    /// Calls `security::access_control::check_access()` before actual send.
    /// Syscall entry points use this variant.
    ///
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
    ///
    /// # Required rights: `Rights::IPC_RECV`
    #[inline]
    pub fn recv_checked(
        &self,
        buf: &mut [u8],
        flags: MsgFlags,
        table: &CapTable,
        token: CapToken,
    ) -> Result<(MessageId, usize, MsgFlags), IpcError> {
        crate::ipc::capability_bridge::check_channel_access(table, token, Rights::IPC_RECV)?;
        self.recv(buf, flags)
    }
}

/// Immutable statistics snapshot
#[derive(Debug, Clone, Copy)]
pub struct SyncChannelSnapshot {
    pub sends_ok: u64,
    pub sends_timeout: u64,
    pub sends_cancelled: u64,
    pub recvs_ok: u64,
    pub recvs_would_block: u64,
    pub total_bytes: u64,
}

// ---------------------------------------------------------------------------
// Global static synchronous channel table
// ---------------------------------------------------------------------------

/// Capacity of global synchronous channel table
pub const SYNC_CHANNEL_TABLE_SIZE: usize = 512;

/// Global static table of allocated synchronous channels
static SYNC_CHANNEL_TABLE: SpinLock<SyncChannelTable> = SpinLock::new(SyncChannelTable::new());

pub struct SyncChannelTable {
    slots: [MaybeUninit<SyncChannel>; SYNC_CHANNEL_TABLE_SIZE],
    used: [bool; SYNC_CHANNEL_TABLE_SIZE],
    count: usize,
}

// SAFETY: all accesses are protected by SpinLock<SyncChannelTable>
unsafe impl Send for SyncChannelTable {}

impl SyncChannelTable {
    pub const fn new() -> Self {
        // SAFETY: mem::zeroed() avoids const-eval memory limits for large arrays.
        unsafe { core::mem::zeroed() }
    }

    /// Allocates a new synchronous channel in table.
    /// Returns index or `None` if table is full.
    pub fn alloc(&mut self) -> Option<usize> {
        for i in 0..SYNC_CHANNEL_TABLE_SIZE {
            if !self.used[i] {
                self.slots[i].write(SyncChannel::new());
                self.used[i] = true;
                self.count += 1;
                return Some(i);
            }
        }
        None
    }

    /// Frees channel by index.
    pub fn free(&mut self, idx: usize) -> bool {
        if idx < SYNC_CHANNEL_TABLE_SIZE && self.used[idx] {
            // SAFETY: used[idx] guarantees slot is initialized
            unsafe { self.slots[idx].assume_init_drop() };
            self.used[idx] = false;
            self.count -= 1;
            true
        } else {
            false
        }
    }

    /// Access to channel by index (non-mut reference).
    /// SAFETY: caller must ensure used[idx] is true.
    pub unsafe fn get_unchecked(&self, idx: usize) -> &SyncChannel {
        self.slots[idx].assume_init_ref()
    }

    pub fn count(&self) -> usize {
        self.count
    }
}

// ---------------------------------------------------------------------------
// High-level public API
// ---------------------------------------------------------------------------

/// Creates a new synchronous channel and returns its index in global table.
pub fn sync_channel_create() -> Result<usize, IpcError> {
    let mut tbl = SYNC_CHANNEL_TABLE.lock();
    tbl.alloc().ok_or(IpcError::OutOfResources)
}

/// Sends on synchronous channel identified by `idx`.
pub fn sync_channel_send(idx: usize, data: &[u8], flags: MsgFlags) -> Result<MessageId, IpcError> {
    let tbl = SYNC_CHANNEL_TABLE.lock();
    if idx >= SYNC_CHANNEL_TABLE_SIZE || !tbl.used[idx] {
        return Err(IpcError::InvalidHandle);
    }
    // SAFETY: used[idx] is true, verified above under spinlock
    let chan_ptr: *const SyncChannel = unsafe { tbl.get_unchecked(idx) as *const SyncChannel };
    let chan_ref: &'static SyncChannel = unsafe { &*chan_ptr };
    chan_ref.active_ops.fetch_add(1, Ordering::Acquire);
    drop(tbl);

    let res = chan_ref.send(data, flags);
    chan_ref.active_ops.fetch_sub(1, Ordering::Release);
    res
}

/// Receives from synchronous channel identified by `idx`.
pub fn sync_channel_recv(
    idx: usize,
    buf: &mut [u8],
    flags: MsgFlags,
) -> Result<(MessageId, usize, MsgFlags), IpcError> {
    let tbl = SYNC_CHANNEL_TABLE.lock();
    if idx >= SYNC_CHANNEL_TABLE_SIZE || !tbl.used[idx] {
        return Err(IpcError::InvalidHandle);
    }
    // SAFETY: used[idx] verified under spinlock above.
    let chan_ptr: *const SyncChannel = unsafe { tbl.get_unchecked(idx) as *const SyncChannel };
    let chan_ref: &'static SyncChannel = unsafe { &*chan_ptr };
    chan_ref.active_ops.fetch_add(1, Ordering::Acquire);
    drop(tbl);

    let res = chan_ref.recv(buf, flags);
    chan_ref.active_ops.fetch_sub(1, Ordering::Release);
    res
}

/// Closes synchronous channel identified by `idx`.
pub fn sync_channel_close(idx: usize) -> Result<(), IpcError> {
    let tbl = SYNC_CHANNEL_TABLE.lock();
    if idx >= SYNC_CHANNEL_TABLE_SIZE || !tbl.used[idx] {
        return Err(IpcError::InvalidHandle);
    }
    // SAFETY: used[idx] verified under spinlock above.
    let chan = unsafe { tbl.get_unchecked(idx) };
    chan.close();
    drop(tbl);
    Ok(())
}

/// Destroys and frees synchronous channel identified by `idx`.
pub fn sync_channel_destroy(idx: usize) -> Result<(), IpcError> {
    let mut tbl = SYNC_CHANNEL_TABLE.lock();
    if idx >= SYNC_CHANNEL_TABLE_SIZE || !tbl.used[idx] {
        return Err(IpcError::InvalidHandle);
    }
    // Close channel to wake up waiting threads before destruction
    let chan = unsafe { tbl.get_unchecked(idx) };
    chan.close();

    // T8-002 FIX: wait until in-flight operations are complete before freeing slot
    let mut spins = 0u32;
    while chan.active_ops.load(Ordering::Acquire) != 0 {
        if spins >= 256 {
            return Err(IpcError::Retry);
        }
        core::hint::spin_loop();
        spins += 1;
        if spins % 32 == 0 {
            unsafe {
                let _ = crate::scheduler::core::switch::cooperative_reschedule();
            }
        }
    }

    if !tbl.free(idx) {
        return Err(IpcError::InvalidHandle);
    }
    Ok(())
}

/// Returns number of active synchronous channels.
pub fn sync_channel_count() -> usize {
    SYNC_CHANNEL_TABLE.lock().count()
}

/// Sends on synchronous channel `idx` with capability check (IPC-04 v6).
///
/// Used by syscall layer. Internal kernel calls can use
/// `sync_channel_send()` directly.
pub fn sync_channel_send_checked(
    idx: usize,
    data: &[u8],
    flags: MsgFlags,
    table: &CapTable,
    token: CapToken,
) -> Result<MessageId, IpcError> {
    let tbl = SYNC_CHANNEL_TABLE.lock();
    if idx >= SYNC_CHANNEL_TABLE_SIZE || !tbl.used[idx] {
        return Err(IpcError::InvalidHandle);
    }
    let chan_ptr: *const SyncChannel = unsafe { tbl.get_unchecked(idx) as *const SyncChannel };
    let chan_ref: &'static SyncChannel = unsafe { &*chan_ptr };
    chan_ref.active_ops.fetch_add(1, Ordering::Acquire);
    drop(tbl);
    let res = chan_ref.send_checked(data, flags, table, token);
    chan_ref.active_ops.fetch_sub(1, Ordering::Release);
    res
}

/// Receives from synchronous channel `idx` with capability check (IPC-04 v6).
pub fn sync_channel_recv_checked(
    idx: usize,
    buf: &mut [u8],
    flags: MsgFlags,
    table: &CapTable,
    token: CapToken,
) -> Result<(MessageId, usize, MsgFlags), IpcError> {
    let tbl = SYNC_CHANNEL_TABLE.lock();
    if idx >= SYNC_CHANNEL_TABLE_SIZE || !tbl.used[idx] {
        return Err(IpcError::InvalidHandle);
    }
    let chan_ptr: *const SyncChannel = unsafe { tbl.get_unchecked(idx) as *const SyncChannel };
    let chan_ref: &'static SyncChannel = unsafe { &*chan_ptr };
    chan_ref.active_ops.fetch_add(1, Ordering::Acquire);
    drop(tbl);
    let res = chan_ref.recv_checked(buf, flags, table, token);
    chan_ref.active_ops.fetch_sub(1, Ordering::Release);
    res
}
