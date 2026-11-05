// kernel/src/memory/dma/channels/channel.rs
//
// Producer/consumer ring buffer for individual DMA channels.
//
// This module implements the low-level mechanism of a DMA channel:
//   - The producer (driver) posts transactions via `submit()`.
//   - The consumer (DMA engine / IRQ handler) retrieves via `consume()`.
//   - Lock-free single-producer/single-consumer (SPSC) ring buffer.
//   - Backpressure: `submit()` returns Err if the ring is full.
//
// LAYER 0 — no external dependencies.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::types::{
    DmaChannelId, DmaDirection, DmaError, DmaMapFlags, DmaTransactionId,
};

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTS
// ─────────────────────────────────────────────────────────────────────────────

/// Depth of a channel's ring buffer (power of 2 for fast masking).
pub const CHANNEL_RING_SIZE: usize = 128;
const RING_MASK: usize = CHANNEL_RING_SIZE - 1;

// ─────────────────────────────────────────────────────────────────────────────
// DMA COMMAND
// ─────────────────────────────────────────────────────────────────────────────

/// A transfer command in the ring buffer.
///
/// Compact format (64 bytes) to fit in a cache line.
#[repr(C, align(64))]
#[derive(Copy, Clone)]
pub struct DmaCommand {
    /// Parent transaction (for completion/error notification).
    pub txn_id: DmaTransactionId,
    /// Source physical address.
    pub src_phys: PhysAddr,
    /// Destination physical address.
    pub dst_phys: PhysAddr,
    /// Transfer length in bytes.
    pub len: u32,
    /// Transfer direction.
    pub direction: u8, // DmaDirection as u8
    /// DMA mapping flags.
    pub flags: u32, // DmaMapFlags.0
    /// Padding to reach exactly 64 bytes.
    _pad: [u8; 7],
}

const _: () = assert!(
    core::mem::size_of::<DmaCommand>() == 64,
    "DmaCommand doit faire exactement 64 bytes"
);

impl DmaCommand {
    pub const EMPTY: Self = DmaCommand {
        txn_id: DmaTransactionId::INVALID,
        src_phys: PhysAddr::new(0),
        dst_phys: PhysAddr::new(0),
        len: 0,
        direction: 0,
        flags: 0,
        _pad: [0u8; 7],
    };

    pub fn new(
        txn_id: DmaTransactionId,
        src_phys: PhysAddr,
        dst_phys: PhysAddr,
        len: u32,
        direction: DmaDirection,
        flags: DmaMapFlags,
    ) -> Self {
        DmaCommand {
            txn_id,
            src_phys,
            dst_phys,
            len,
            direction: direction as u8,
            flags: flags.0,
            _pad: [0u8; 7],
        }
    }

    pub fn is_valid(self) -> bool {
        self.txn_id.0 != DmaTransactionId::INVALID.0 && self.len > 0
    }

    pub fn direction(self) -> DmaDirection {
        match self.direction {
            0 => DmaDirection::ToDevice,
            1 => DmaDirection::FromDevice,
            2 => DmaDirection::Bidirection,
            _ => DmaDirection::None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SPSC RING BUFFER
// ─────────────────────────────────────────────────────────────────────────────

/// SPSC (Single-Producer/Single-Consumer) ring buffer for DMA commands.
///
/// ## Guarantees
/// - `submit()` can be called from any context (even IRQ)
///   by **a single** producer at a time.
/// - `consume()` is called only by the DMA engine or the IRQ handler,
///   by **a single** consumer at a time.
/// - No locks required for normal operations (lock-free SPSC).
///
/// ## Overflow
/// If the ring is full, `submit()` returns `Err(DmaError::OutOfMemory)`.
/// The producer is responsible for handling backpressure.
pub struct DmaChannelRing {
    /// Channel owning this ring.
    pub channel_id: DmaChannelId,
    /// Commands in the ring.
    /// UnsafeCell for SPSC mutation without locks.
    ring: UnsafeCell<[DmaCommand; CHANNEL_RING_SIZE]>,
    /// Next write slot index (producer).
    head: AtomicU32,
    /// Next read slot index (consumer).
    tail: AtomicU32,
    /// Ring active (can receive submissions).
    active: AtomicBool,
    /// Statistics.
    pub submitted: AtomicU64,
    pub consumed: AtomicU64,
    pub dropped_full: AtomicU64,
    pub errors_injected: AtomicU64,
}

// SAFETY: SPSC guaranteed by convention. head modified only by the producer,
// tail only by the consumer. No concurrent access to the same slots.
unsafe impl Sync for DmaChannelRing {}
unsafe impl Send for DmaChannelRing {}

impl DmaChannelRing {
    pub const fn new(channel_id: DmaChannelId) -> Self {
        DmaChannelRing {
            channel_id,
            ring: UnsafeCell::new([DmaCommand::EMPTY; CHANNEL_RING_SIZE]),
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            active: AtomicBool::new(false),
            submitted: AtomicU64::new(0),
            consumed: AtomicU64::new(0),
            dropped_full: AtomicU64::new(0),
            errors_injected: AtomicU64::new(0),
        }
    }

    /// Activates this ring (allows submissions).
    pub fn activate(&self) {
        self.active.store(true, Ordering::Release);
    }

    /// Deactivates this ring (rejects new submissions).
    pub fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    // ── Producer ─────────────────────────────────────────────────────────────

    /// Submits a DMA command into the ring.
    ///
    /// # Errors
    /// - `DmaError::NotInitialized`: inactive ring.
    /// - `DmaError::OutOfMemory`: ring full (backpressure).
    /// - `DmaError::InvalidParams`: invalid command.
    pub fn submit(&self, cmd: DmaCommand) -> Result<(), DmaError> {
        if !self.is_active() {
            return Err(DmaError::NotInitialized);
        }
        if !cmd.is_valid() {
            return Err(DmaError::InvalidParams);
        }

        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);

        // Ring full: head - tail >= CHANNEL_RING_SIZE.
        if (head.wrapping_sub(tail)) as usize >= CHANNEL_RING_SIZE {
            self.dropped_full.fetch_add(1, Ordering::Relaxed);
            return Err(DmaError::OutOfMemory);
        }

        let slot = (head as usize) & RING_MASK;
        // SAFETY: head % SIZE ∈ [0,SIZE); no concurrent consumer (tail != head+1 checked).
        unsafe {
            (*self.ring.get())[slot] = cmd;
        }

        // Release fence : The slot write must be visible to the consumer before
        // the head increment.
        self.head.store(head.wrapping_add(1), Ordering::Release);
        self.submitted.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    // ── Consumer ─────────────────────────────────────────────────────────────

    /// Retrieves the next command from the ring.
    ///
    /// Returns `None` if the ring is empty.
    /// Called from IRQ / DMA engine context.
    pub fn consume(&self) -> Option<DmaCommand> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);

        if tail == head {
            return None; // Ring empty.
        }

        let slot = (tail as usize) & RING_MASK;
        // SAFETY: tail < head, producer no longer writes to `tail`.
        let cmd = unsafe { (*self.ring.get())[slot] };

        // SAFETY: cleared slot visible before tail increment (Release); single consumer.
        unsafe {
            (*self.ring.get())[slot] = DmaCommand::EMPTY;
        }
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        self.consumed.fetch_add(1, Ordering::Relaxed);
        Some(cmd)
    }

    /// Consumes up to `out_buf.len()` commands in a single pass.
    /// Returns the number of commands written to `out_buf`.
    pub fn consume_batch(&self, out_buf: &mut [DmaCommand]) -> usize {
        let mut count = 0;
        while count < out_buf.len() {
            match self.consume() {
                Some(cmd) => {
                    out_buf[count] = cmd;
                    count += 1;
                }
                None => break,
            }
        }
        count
    }

    // ── Observers ────────────────────────────────────────────────────────────

    /// Approximate number of pending commands in the ring.
    pub fn pending(&self) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        (head.wrapping_sub(tail)) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.pending() == 0
    }
    pub fn is_full(&self) -> bool {
        self.pending() >= CHANNEL_RING_SIZE
    }
    pub fn capacity(&self) -> usize {
        CHANNEL_RING_SIZE
    }
}
