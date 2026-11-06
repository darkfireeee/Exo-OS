// kernel/src/ipc/ring/slot.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SLOT MANAGEMENT — slot management in IPC rings
// (Exo-OS · IPC Layer 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// A slot is the atomic communication unit within a ring.
// Each slot has a state (Sequence State) controlling who can
// read or write into it, without locks.
//
// TECHNIQUE: Sequence-based ring (Dmitry Vyukov MPMC Queue).
// A slot's state is encoded in an AtomicU64 containing the slot's
// sequence number. The producer waits for seq == pos, the consumer
// waits for seq == pos + 1.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::ipc::core::constants::RING_SIZE;
use crate::ipc::core::transfer::RingSlot;
use crate::ipc::core::types::array_index_nospec;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// SlotCell — atomic cell of a ring
// ─────────────────────────────────────────────────────────────────────────────

/// A ring cell: sequence number + data.
/// The sequence number encodes the cell state:
///   - seq == index          -> free (ready for producer)
///   - seq == index + 1      -> data available (ready for consumer)
///   - other                 -> write or read in progress
#[repr(C, align(64))]
pub struct SlotCell {
    /// Atomic sequence number — determines who can access the slot.
    pub sequence: AtomicU64,
    /// Inlined data in the slot.
    pub slot: UnsafeCell<MaybeUninit<RingSlot>>,
    /// Padding to avoid false sharing (fills to 64 bytes minimum).
    _pad: [u8; 0],
}

// SAFETY: SlotCell is accessed only via atomics and sequence agreements.
// UnsafeCell is protected by the sequential protocol.
unsafe impl Send for SlotCell {}
unsafe impl Sync for SlotCell {}

impl SlotCell {
    /// Initializes a cell with its index (initial sequence = index).
    pub const fn new_at(index: u64) -> Self {
        Self {
            sequence: AtomicU64::new(index),
            slot: UnsafeCell::new(MaybeUninit::uninit()),
            _pad: [],
        }
    }

    /// Loads sequence number (Acquire).
    #[inline(always)]
    pub fn load_seq(&self) -> u64 {
        self.sequence.load(Ordering::Acquire)
    }

    /// Stores sequence number (Release).
    #[inline(always)]
    pub fn store_seq(&self, seq: u64) {
        self.sequence.store(seq, Ordering::Release)
    }

    /// Accesses data slot exclusively (precondition: sequence acquired).
    ///
    /// # Safety
    /// Caller must hold exclusive access to slot via sequence protocol.
    #[inline(always)]
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn data_mut(&self) -> &mut MaybeUninit<RingSlot> {
        &mut *self.slot.get()
    }

    /// Accesses data slot for reading (precondition: sequence acquired).
    ///
    /// # Safety
    /// Caller must hold read access via sequence protocol.
    #[inline(always)]
    pub unsafe fn data_ref(&self) -> &MaybeUninit<RingSlot> {
        &*self.slot.get()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RingBuffer — static array of RING_SIZE SlotCells
// ─────────────────────────────────────────────────────────────────────────────

/// Fixed-size ring buffer of RING_SIZE.
/// Statically allocated or embedded in a larger structure.
///
/// NOTE: total size ≈ RING_SIZE x sizeof(SlotCell)
///       = 4096 x 4112 bytes ≈ 16 MiB per ring.
/// In practice, rings are created dynamically from the SHM pool
/// rather than all statically allocated. A single static ring = system ring.
pub struct RingBuffer {
    cells: [SlotCell; RING_SIZE],
}

impl RingBuffer {
    /// Initializes a ring buffer (sequences = index for each cell).
    pub fn init(&mut self) {
        for (i, cell) in self.cells.iter().enumerate() {
            cell.sequence.store(i as u64, Ordering::Relaxed);
        }
        // Final barrier so that all stores become visible.
        core::sync::atomic::fence(Ordering::Release);
    }

    /// Accesses cell at index `pos % RING_SIZE`.
    /// Uses array_index_nospec (RULE IPC-08 — Spectre v1).
    #[inline(always)]
    pub fn cell_at(&self, pos: u64) -> &SlotCell {
        let idx = array_index_nospec((pos as usize) & (RING_SIZE - 1), RING_SIZE);
        &self.cells[idx]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SlotGuard — RAII for automatic slot release
// ─────────────────────────────────────────────────────────────────────────────

/// RAII guard on a ring slot.
/// On Drop, advances sequence to signal end of read/write.
pub struct SlotWriteGuard<'a> {
    cell: &'a SlotCell,
    /// Sequence to write on Drop (= pos + 1 to signal "data available").
    commit_seq: u64,
    /// Abort instead of commit if Drop is called without commit().
    aborted: bool,
}

impl<'a> SlotWriteGuard<'a> {
    /// Creates a write guard.
    /// `commit_seq` is pos + 1 for a standard ring.
    pub fn new(cell: &'a SlotCell, commit_seq: u64) -> Self {
        Self {
            cell,
            commit_seq,
            aborted: false,
        }
    }

    /// Accesses slot data for writing.
    ///
    /// # Safety
    /// Caller is the sole active producer on this slot.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn data_mut(&self) -> &mut MaybeUninit<RingSlot> {
        self.cell.data_mut()
    }

    /// Commits the write.
    pub fn commit(mut self) {
        self.aborted = false;
        self.cell.store_seq(self.commit_seq);
        core::mem::forget(self); // avoids Drop
    }

    /// Aborts write (restores seq = commit_seq - 1).
    pub fn abort(mut self) {
        self.aborted = true;
        // Restore original sequence = pos (commit_seq - 1).
        self.cell.store_seq(self.commit_seq.wrapping_sub(1));
        core::mem::forget(self);
    }
}

impl<'a> Drop for SlotWriteGuard<'a> {
    fn drop(&mut self) {
        // If neither commit nor abort was called, it's a bug — panic in debug.
        if !self.aborted {
            // Restore to prevent ring corruption.
            self.cell.store_seq(self.commit_seq.wrapping_sub(1));
            debug_assert!(
                false,
                "SlotWriteGuard droppé sans commit ni abort — ring corrompu"
            );
        }
    }
}

/// RAII guard on a ring slot in read mode.
pub struct SlotReadGuard<'a> {
    cell: &'a SlotCell,
    /// Sequence to write on Drop (= pos + RING_SIZE to reuse slot).
    release_seq: u64,
    released: bool,
}

impl<'a> SlotReadGuard<'a> {
    pub fn new(cell: &'a SlotCell, release_seq: u64) -> Self {
        Self {
            cell,
            release_seq,
            released: false,
        }
    }

    /// Accesses slot data in read mode.
    ///
    /// # Safety
    /// Caller is the sole active consumer on this slot.
    pub unsafe fn data_ref(&self) -> &MaybeUninit<RingSlot> {
        self.cell.data_ref()
    }

    /// Releases slot (returns it to ring for next producer).
    pub fn release(mut self) {
        self.released = true;
        self.cell.store_seq(self.release_seq);
        core::mem::forget(self);
    }
}

impl<'a> Drop for SlotReadGuard<'a> {
    fn drop(&mut self) {
        if !self.released {
            self.cell.store_seq(self.release_seq);
            debug_assert!(
                false,
                "SlotReadGuard droppé sans release — slot jamais libéré"
            );
        }
    }
}
