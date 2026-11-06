// kernel/src/ipc/ring/spsc.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SPSC RING — Single-Producer Single-Consumer ultra-fast
// (Exo-OS · IPC Layer 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Implementation of a lock-free SPSC FIFO queue with Release/Acquire barriers.
// This is the fastest path for kernel-to-kernel or thread-to-thread IPC
// between the same pair of communicants.
//
// ALGORITHM:
//   • Producer: reads head, accesses cell[head % N], writes data, advances head.
//   • Consumer: reads tail, accesses cell[tail % N], reads data, advances tail.
//   • Tail and Head are on separate cache lines (false sharing eliminated).
//   • Sequences in each slot prevent needing a global lock.
//
// TARGET PERFORMANCE: > 50 million msgs/s in SPSC per channel @ 3 GHz.
// ═══════════════════════════════════════════════════════════════════════════════

use super::slot::SlotCell;
use crate::ipc::core::transfer::MessageHeader;
use crate::ipc::core::IpcFastMsg;
use crate::ipc::core::{
    alloc_message_id, array_index_nospec, IpcError, MessageId, MsgFlags, RING_MASK, RING_SIZE,
};
use core::cell::UnsafeCell;
use core::sync::atomic::{fence, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// SPSC capacity — public alias of RING_SIZE
// ─────────────────────────────────────────────────────────────────────────────

/// Number of slots in an SPSC ring (= RING_SIZE).
/// Re-exported for use in message/ and channel/ modules.
pub const SPSC_CAPACITY: usize = RING_SIZE;

const _: () = assert!(
    SPSC_CAPACITY.is_power_of_two(),
    "SPSC capacity must support ring masking"
);
const _: () = assert!(
    core::mem::size_of::<crate::ipc::core::transfer::RingSlot>()
        >= core::mem::size_of::<MessageHeader>() + crate::ipc::core::MAX_MSG_SIZE,
    "SPSC ring slot cannot hold the IPC header and inline payload"
);
const _: () = assert!(
    core::mem::size_of::<SlotCell>() * SPSC_CAPACITY <= 4 * 1024 * 1024,
    "SPSC ring storage exceeds the audit budget"
);

// ─────────────────────────────────────────────────────────────────────────────
// SpscRing — main structure
// ─────────────────────────────────────────────────────────────────────────────

/// Cache line padding for AtomicU64 — separates head and tail of the ring.
/// AtomicU64 = 8 bytes, padding = 56 bytes -> total 64 bytes = 1 cache line.
#[repr(C, align(64))]
struct CachePad(AtomicU64, [u8; 56]);

/// SPSC ring with head and tail on distinct cache lines.
///
/// # Usage
/// ```
/// let ring = SpscRing::new();
/// ring.init();
/// // In producer:
/// ring.push_copy(&data, data.len(), MsgFlags::default())?;
/// // In consumer:
/// ring.pop_into(&mut buf)?;
/// ```
pub struct SpscRing {
    /// Position of next slot to write (cache line 0).
    head: CachePad,
    /// Position of next slot to read (cache line 1).
    tail: CachePad,
    /// Ring slots (~16 MiB — allocated in SHM in production).
    cells: UnsafeCell<[SlotCell; RING_SIZE]>,
}

// SAFETY: access to cells regulated by the head/tail/sequence protocol.
unsafe impl Send for SpscRing {}
unsafe impl Sync for SpscRing {}

impl SpscRing {
    /// Constructs an SpscRing — head and tail on separate cache lines.
    /// Note: `CachePad` = AtomicU64(8B) + padding(56B) = 64B = 1 cache line.
    pub const fn new() -> Self {
        Self {
            head: CachePad(AtomicU64::new(0), [0u8; 56]),
            tail: CachePad(AtomicU64::new(0), [0u8; 56]),
            cells: UnsafeCell::new([const { SlotCell::new_at(0) }; RING_SIZE]),
        }
    }

    /// Initializes sequences of all cells.
    /// Must be called only once after construction.
    pub fn init(&self) {
        // SAFETY: init() is called only once prior to any usage.
        let cells = unsafe { &mut *self.cells.get() };
        for (i, cell) in cells.iter_mut().enumerate() {
            cell.sequence.store(i as u64, Ordering::Relaxed);
        }
        fence(Ordering::Release);
    }

    /// Returns cell at position `pos`.
    /// Uses array_index_nospec (RULE IPC-08 — Spectre v1).
    #[inline(always)]
    fn cell_at(&self, pos: u64) -> &SlotCell {
        let cells = unsafe { &*self.cells.get() };
        // array_index_nospec: prevents out-of-bounds CPU speculation (Spectre v1).
        let idx = array_index_nospec((pos as usize) & RING_MASK, RING_SIZE);
        &cells[idx]
    }

    // ───────────────────────── PRODUCER ─────────────────────────────────

    /// Sends a message (data copy from slice).
    ///
    /// # Return
    /// - `Ok(MessageId)`  : success.
    /// - `Err(WouldBlock)`: ring full (non-blocking).
    /// - `Err(MessageTooLarge)`: len > MAX_MSG_SIZE.
    #[inline]
    pub fn push_copy(&self, src: &[u8], flags: MsgFlags) -> Result<MessageId, IpcError> {
        if src.len() > crate::ipc::core::MAX_MSG_SIZE {
            return Err(IpcError::MessageTooLarge);
        }
        let pos = self.head.0.load(Ordering::Relaxed);
        let cell = self.cell_at(pos);
        let seq = cell.load_seq();

        // Free slot if seq == pos (not yet occupied).
        if seq != pos {
            // seq < pos -> ring full; seq > pos -> impossible in SPSC.
            return Err(IpcError::QueueFull);
        }

        let id = alloc_message_id();
        // SAFETY: single producer (SPSC); seq == pos proves consumer has freed this slot.
        unsafe {
            let slot = (*cell.slot.get()).assume_init_mut();
            slot.header = MessageHeader::new_inline(id, flags, src.len());
            if !src.is_empty() {
                core::ptr::copy_nonoverlapping(src.as_ptr(), slot.payload.as_mut_ptr(), src.len());
            }
        }

        // Release: makes slot visible to consumer.
        cell.store_seq(pos + 1);
        self.head.0.store(pos + 1, Ordering::Release);
        Ok(id)
    }

    /// Sends a zero-copy message (data already in shared buffer).
    /// `zc_ref` is encoded in the first 8 bytes of payload.
    #[inline]
    pub fn push_zerocopy(
        &self,
        zc_ref: crate::ipc::core::transfer::ZeroCopyRef,
        flags: MsgFlags,
    ) -> Result<MessageId, IpcError> {
        let pos = self.head.0.load(Ordering::Relaxed);
        let cell = self.cell_at(pos);
        if cell.load_seq() != pos {
            return Err(IpcError::QueueFull);
        }
        let id = alloc_message_id();
        let f = MsgFlags(flags.0 | MsgFlags::ZEROCOPY.0);
        // SAFETY: SPSC — single active producer.
        unsafe {
            let slot = (*cell.slot.get()).assume_init_mut();
            slot.header = MessageHeader::new_zerocopy(id, f);
            core::ptr::copy_nonoverlapping(
                &zc_ref as *const _ as *const u8,
                slot.payload.as_mut_ptr(),
                core::mem::size_of::<crate::ipc::core::transfer::ZeroCopyRef>(),
            );
        }
        cell.store_seq(pos + 1);
        self.head.0.store(pos + 1, Ordering::Release);
        Ok(id)
    }

    // ───────────────────────── CONSUMER ──────────────────────────────

    /// Receives a message into `dst`. Returns number of copied bytes.
    ///
    /// # Return
    /// - `Ok((n, flags))` : n bytes copied, message flags.
    /// - `Err(QueueEmpty)` : ring empty.
    #[inline]
    pub fn pop_into(&self, dst: &mut [u8]) -> Result<(usize, MsgFlags), IpcError> {
        let pos = self.tail.0.load(Ordering::Relaxed);
        let cell = self.cell_at(pos);
        let seq = cell.load_seq();

        // Slot available if seq == pos + 1.
        if seq != pos + 1 {
            return Err(IpcError::QueueEmpty);
        }

        // SAFETY: seq == pos + 1 -> produced and visible. SPSC -> single consumer.
        let (header, len) = unsafe {
            let slot = (*cell.slot.get()).assume_init_ref();
            let h = slot.header;
            let ln = slot.header.len as usize;
            if ln > dst.len() {
                // Do not consume slot: message remains at ring head so caller
                // can allocate a larger buffer and retry.
                return Err(IpcError::MessageTooLarge);
            }
            if ln > 0 {
                core::ptr::copy_nonoverlapping(slot.payload.as_ptr(), dst.as_mut_ptr(), ln);
            }
            (h, ln)
        };
        let _ = header;

        // Releases slot: pos + RING_SIZE = next lap of producer.
        cell.store_seq(pos + RING_SIZE as u64);
        self.tail.0.store(pos + 1, Ordering::Release);
        Ok((len, MsgFlags(header.flags)))
    }

    /// Inspects next message without consuming it (peek).
    /// Returns header without copying payload.
    #[inline]
    pub fn peek_header(&self) -> Option<MessageHeader> {
        let pos = self.tail.0.load(Ordering::Relaxed);
        let cell = self.cell_at(pos);
        if cell.load_seq() != pos + 1 {
            return None;
        }
        // SAFETY: seq == pos + 1 -> data available.
        let h = unsafe { (*cell.slot.get()).assume_init_ref().header };
        Some(h)
    }

    /// Returns true if ring is empty.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        let pos = self.tail.0.load(Ordering::Relaxed);
        let cell = self.cell_at(pos);
        cell.load_seq() != pos + 1
    }

    /// Returns true if ring is full.
    #[inline(always)]
    pub fn is_full(&self) -> bool {
        let pos = self.head.0.load(Ordering::Relaxed);
        let cell = self.cell_at(pos);
        cell.load_seq() != pos
    }

    /// Approximate number of pending messages.
    #[inline(always)]
    pub fn len_approx(&self) -> usize {
        let h = self.head.0.load(Ordering::Relaxed);
        let t = self.tail.0.load(Ordering::Relaxed);
        h.wrapping_sub(t) as usize
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// no_mangle functions called from fastcall_asm.s
// ─────────────────────────────────────────────────────────────────────────────

/// Global table of active SPSC rings (indexed by channel_id % MAX_SPSC_RINGS).
/// In production, replaced by a dynamically allocated structure from
/// the SHM pool. Here: static table for bootstrap and tests.
// FIX P1-04: increase MAX_SPSC_RINGS to support more IPC channels.
// 4096 rings x ~200 bytes/ring ≈ 800 KiB .bss — acceptable for the kernel.
// Aspirational: 65536 (MAX_CHANNELS) — will be revisited with dynamic allocation.
const MAX_SPSC_RINGS: usize = 4096;

static SPSC_RINGS: [SpscRing; MAX_SPSC_RINGS] = [const { SpscRing::new() }; MAX_SPSC_RINGS];

static SPSC_INIT: AtomicU64 = AtomicU64::new(0);

/// Initializes all rings in the static table (called at IPC boot).
pub fn init_spsc_rings() {
    if SPSC_INIT
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
    {
        for ring in &SPSC_RINGS {
            ring.init();
        }
    }
}

/// Accesses ring corresponding to a channel_id.
/// array_index_nospec (RULE IPC-08): prevents out-of-bounds speculation.
#[inline(always)]
fn ring_for(channel_id: u64) -> Result<&'static SpscRing, IpcError> {
    let raw_idx = usize::try_from(channel_id).map_err(|_| IpcError::InvalidParam)?;
    if raw_idx >= MAX_SPSC_RINGS {
        return Err(IpcError::InvalidParam);
    }
    let idx = array_index_nospec(raw_idx, MAX_SPSC_RINGS);
    Ok(&SPSC_RINGS[idx])
}

/// Fast write for fastcall_asm.s.
///
/// # Safety
/// `msg` must be a valid pointer to an `IpcFastMsg`.
pub unsafe fn spsc_fast_write(msg: *const IpcFastMsg, channel_id: u64) -> u64 {
    let m = &*msg;
    let len = m.len as usize;
    let flags = MsgFlags(m.flags);
    let ring = match ring_for(channel_id) {
        Ok(ring) => ring,
        Err(err) => return err as u64,
    };

    match ring.push_copy(&m.data[..len.min(64)], flags) {
        Ok(_) => 0,
        Err(e) => e as u64,
    }
}

/// Fast read for fastcall_asm.s.
///
/// # Safety
/// `dst` must be a valid pointer to an `IpcFastMsg`.
pub unsafe fn spsc_fast_read(dst: *mut IpcFastMsg, channel_id: u64) -> u64 {
    let m = &mut *dst;
    let ring = match ring_for(channel_id) {
        Ok(ring) => ring,
        Err(err) => return err as u64,
    };
    let buf = &mut m.data[..];

    match ring.pop_into(buf) {
        Ok(n) => {
            m.len = n.0 as u16;
            0
        }
        Err(e) => e as u64,
    }
}

/// Reply wait (polling loop + yield) for fast call.
///
/// # Safety
/// `dst` must be a valid pointer to an `IpcFastMsg`.
pub unsafe fn spsc_wait_reply(dst: *mut IpcFastMsg, channel_id: u64, timeout_ns: u64) -> u64 {
    // Polling with spin counter before yield.
    const SPIN_LIMIT: u64 = 10_000;
    let ring = match ring_for(channel_id) {
        Ok(ring) => ring,
        Err(err) => return err as u64,
    };
    let m = &mut *dst;
    let buf = &mut m.data[..];

    let start_ns = if timeout_ns > 0 {
        crate::scheduler::timer::clock::monotonic_ns()
    } else {
        0
    };
    let mut spins: u64 = 0;
    loop {
        match ring.pop_into(buf) {
            Ok(n) => {
                m.len = n.0 as u16;
                return 0;
            }
            Err(IpcError::QueueEmpty) => {}
            Err(e) => return e as u64,
        }
        spins += 1;
        if spins > SPIN_LIMIT {
            // Yield to scheduler to let other threads run.
            // SAFETY: safe kernel call from kernel context.
            extern "C" {
                fn arch_cpu_relax();
            }
            arch_cpu_relax();
        }
        if timeout_ns > 0 {
            let elapsed = crate::scheduler::timer::clock::monotonic_ns().saturating_sub(start_ns);
            if elapsed >= timeout_ns {
                return IpcError::Timeout as u64;
            }
        }
        core::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::{init_spsc_rings, spsc_fast_read, spsc_fast_write, MAX_SPSC_RINGS};
    use crate::ipc::core::{IpcError, IpcFastMsg};

    #[test]
    fn test_invalid_channel_id_returns_error() {
        init_spsc_rings();

        let mut msg = IpcFastMsg::zeroed();
        msg.len = 1;
        msg.data[0] = 0xAB;

        let write_status = unsafe { spsc_fast_write(&msg as *const _, MAX_SPSC_RINGS as u64) };
        let read_status = unsafe { spsc_fast_read(&mut msg as *mut _, MAX_SPSC_RINGS as u64) };

        assert_eq!(write_status, IpcError::InvalidParam as u64);
        assert_eq!(read_status, IpcError::InvalidParam as u64);
    }

    #[test]
    fn test_invalid_channel_ids_do_not_alias_stress() {
        init_spsc_rings();

        let mut seed = IpcFastMsg::zeroed();
        seed.len = 3;
        seed.data[0..3].copy_from_slice(&[1, 2, 3]);

        let ok = unsafe { spsc_fast_write(&seed as *const _, 0) };
        assert_eq!(ok, 0);

        for channel_id in MAX_SPSC_RINGS as u64..(MAX_SPSC_RINGS as u64 + 4096) {
            let write_status = unsafe { spsc_fast_write(&seed as *const _, channel_id) };
            assert_eq!(write_status, IpcError::InvalidParam as u64);
        }

        let mut recv = IpcFastMsg::zeroed();
        let read_ok = unsafe { spsc_fast_read(&mut recv as *mut _, 0) };
        assert_eq!(read_ok, 0);
        assert_eq!(recv.len, 3);
        assert_eq!(&recv.data[0..3], &[1, 2, 3]);
    }
}
