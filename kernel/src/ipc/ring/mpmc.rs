// kernel/src/ipc/ring/mpmc.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// MPMC RING — Multi-Producer Multi-Consumer lock-free (atomic sequence)
// (Exo-OS · IPC Layer 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Dmitry Vyukov's algorithm (adapted):
//   • Each cell holds its own atomic sequence number.
//   • Producer: CAS on head -> reserves a position.
//   • Consumer: CAS on tail -> reserves a position.
//   • No global lock — each thread makes progress independently.
//
// GUARANTEES:
//   • Wait-free for threads succeeding at their CAS on first attempt.
//   • Lock-free in general case (at least one thread progresses).
//   • Approximate FIFO order (not guaranteed if multiple simultaneous producers).
//
// USAGE: MPMC channel for event dispatching, worker pools.
// ═══════════════════════════════════════════════════════════════════════════════

use super::slot::SlotCell;
use crate::ipc::core::transfer::MessageHeader;
use crate::ipc::core::{
    alloc_message_id, array_index_nospec, IpcError, MsgFlags, RING_MASK, RING_SIZE,
};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

/// Number of slots in an MPMC ring (= RING_SIZE).
/// Re-exported for use in channel/mpmc.rs.
pub const MPMC_RING_SIZE: usize = RING_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// MpmcRing
// ─────────────────────────────────────────────────────────────────────────────

/// Lock-free MPMC ring.
/// Head and tail are each on their own cache line.
#[repr(C, align(64))]
pub struct MpmcRing {
    /// Producer cursor — shared between multiple threads.
    head: [u8; 64], // AtomicU64 at offset 0 in this cache pad
    /// Consumer cursor — shared between multiple threads.
    tail: [u8; 64],
    /// Array of cells.
    cells: UnsafeCell<[SlotCell; RING_SIZE]>,
}

// SAFETY: MpmcRing is thread-safe by design (atomics + sequences).
unsafe impl Send for MpmcRing {}
unsafe impl Sync for MpmcRing {}

impl MpmcRing {
    /// Returns head cursor (producer) as AtomicU64.
    #[inline(always)]
    fn head_atomic(&self) -> &AtomicU64 {
        // SAFETY: head[0..8] is 64-byte aligned and contains an AtomicU64.
        unsafe { &*(self.head.as_ptr() as *const AtomicU64) }
    }

    /// Returns tail cursor (consumer) as AtomicU64.
    #[inline(always)]
    fn tail_atomic(&self) -> &AtomicU64 {
        // SAFETY: tail[0..8] is 64-byte aligned and contains an AtomicU64.
        unsafe { &*(self.tail.as_ptr() as *const AtomicU64) }
    }

    /// Creates an uninitialized ring.
    ///
    /// # Safety
    /// `init()` MUST be called prior to any usage.
    pub const fn new_uninit() -> Self {
        Self {
            head: [0u8; 64],
            tail: [0u8; 64],
            cells: UnsafeCell::new([const { SlotCell::new_at(0) }; RING_SIZE]),
        }
    }

    /// Initializes ring — must be called only once after construction.
    pub fn init(&self) {
        self.head_atomic().store(0, Ordering::Relaxed);
        self.tail_atomic().store(0, Ordering::Relaxed);
        let cells = unsafe { &mut *self.cells.get() };
        for (i, cell) in cells.iter_mut().enumerate() {
            cell.sequence.store(i as u64, Ordering::Relaxed);
        }
        core::sync::atomic::fence(Ordering::Release);
    }

    /// Accesses cell at position `pos`.
    /// Uses array_index_nospec (RULE IPC-08 — Spectre v1).
    #[inline(always)]
    fn cell_at(&self, pos: u64) -> &SlotCell {
        let cells = unsafe { &*self.cells.get() };
        let idx = array_index_nospec((pos as usize) & RING_MASK, RING_SIZE);
        &cells[idx]
    }

    // ───────────────────────────── PUSH (producer) ─────────────────────

    /// Sends a message (copy).
    ///
    /// # Multi-thread safety
    /// Can be called from multiple threads simultaneously.
    pub fn push_copy(&self, src: &[u8], flags: MsgFlags) -> Result<u64, IpcError> {
        if src.len() > crate::ipc::core::MAX_MSG_SIZE {
            return Err(IpcError::MessageTooLarge);
        }
        loop {
            let pos = self.head_atomic().load(Ordering::Acquire);
            let cell = self.cell_at(pos);
            let seq = cell.load_seq();
            let diff = (seq as i64).wrapping_sub(pos as i64);
            if diff < 0 {
                return Err(IpcError::QueueFull);
            }
            if diff != 0 {
                core::hint::spin_loop();
                continue;
            }
            if self
                .head_atomic()
                .compare_exchange_weak(pos, pos + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                core::hint::spin_loop();
                continue;
            }

            let id = alloc_message_id();
            // SAFETY: we reserved this slot via sequential protocol.
            unsafe {
                let slot = (*cell.slot.get()).assume_init_mut();
                slot.header = MessageHeader::new_inline(id, flags, src.len());
                if !src.is_empty() {
                    core::ptr::copy_nonoverlapping(
                        src.as_ptr(),
                        slot.payload.as_mut_ptr(),
                        src.len(),
                    );
                }
            }
            // Make visible: sequence = pos + 1.
            cell.store_seq(pos + 1);
            return Ok(id.get());
        }
    }

    // ───────────────────────────── POP (consumer) ────────────────────

    /// Receives a message into `dst`.
    pub fn pop_into(&self, dst: &mut [u8]) -> Result<(usize, MsgFlags), IpcError> {
        loop {
            let pos = self.tail_atomic().load(Ordering::Acquire);
            let cell = self.cell_at(pos);
            let seq = cell.load_seq();
            let diff = (seq as i64).wrapping_sub((pos + 1) as i64);
            if diff < 0 {
                return Err(IpcError::QueueEmpty);
            }
            if diff != 0 {
                core::hint::spin_loop();
                continue;
            }

            // Verify destination buffer is large enough BEFORE consuming slot.
            // SAFETY: seq == pos + 1 guarantees slot was published by producer.
            unsafe {
                let slot = (*cell.slot.get()).assume_init_ref();
                let ln = slot.header.len as usize;
                if ln > dst.len() {
                    // Preserve message at ring head without advancing tail.
                    return Err(IpcError::MessageTooLarge);
                }
            }

            if self
                .tail_atomic()
                .compare_exchange_weak(pos, pos + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                core::hint::spin_loop();
                continue;
            }

            let (len, msg_flags) = unsafe {
                let slot = (*cell.slot.get()).assume_init_ref();
                let ln = slot.header.len as usize;
                let flags = slot.header.flags;
                if ln > 0 {
                    core::ptr::copy_nonoverlapping(slot.payload.as_ptr(), dst.as_mut_ptr(), ln);
                }
                (ln, MsgFlags(flags))
            };

            // Releases slot for next producer.
            cell.store_seq(pos + RING_SIZE as u64);
            return Ok((len, msg_flags));
        }
    }

    /// Returns true if ring is approximately empty.
    #[inline(always)]
    pub fn is_empty_approx(&self) -> bool {
        let h = self.head_atomic().load(Ordering::Relaxed);
        let t = self.tail_atomic().load(Ordering::Relaxed);
        h == t
    }

    /// Approximate number of pending messages.
    #[inline(always)]
    pub fn len_approx(&self) -> usize {
        let h = self.head_atomic().load(Ordering::Relaxed);
        let t = self.tail_atomic().load(Ordering::Relaxed);
        h.wrapping_sub(t) as usize
    }
}
