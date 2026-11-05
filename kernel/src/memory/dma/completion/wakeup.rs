// kernel/src/memory/dma/completion/wakeup.rs
//
// Waiter wakeup on DMA completion.
//
// This module is the "upper" notification layer: it associates a TID
// with an in-flight transaction and triggers wakeup via the handler registered
// by process/ (`DmaWakeupHandler`).
//
// Difference with `core/wakeup_iface.rs`:
//   - `wakeup_iface.rs` defines the **trait** (Layer 0, minimal interface).
//   - `completion/wakeup.rs` manages the **waiters table** and orchestrates
//     notification from the DmaCompletionManager during a DMA IRQ.
//
// LAYER 0 — no external dependencies on process/scheduler.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

use crate::memory::dma::core::types::{DmaChannelId, DmaError, DmaTransactionId};
use crate::memory::dma::core::wakeup_iface::wake_on_completion;

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTS
// ─────────────────────────────────────────────────────────────────────────────

/// Size of the waiters table (transactions awaiting completion).
pub const WAKEUP_TABLE_SIZE: usize = 512;

// ─────────────────────────────────────────────────────────────────────────────
// WAITER STATE
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Eq, PartialEq)]
#[repr(u8)]
enum WaiterState {
    Free = 0,
    Waiting = 1,
    Completed = 2,
}

/// A registered waiter for a DMA transaction.
///
/// Statically allocated — no Box, no Vec.
#[repr(C, align(64))]
pub struct WaiterSlot {
    /// Expected transaction.
    txn_id: DmaTransactionId,
    /// DMA channel.
    channel_id: DmaChannelId,
    /// TID of the thread/process waiting (opaque, interpreted by process/).
    waiter_tid: AtomicU64,
    /// Waiter state.
    state: AtomicU8,
    /// Available result (true = success).
    result_ok: AtomicBool,
    /// Transferred bytes (OK) or error code (Err).
    result_val: AtomicU64,
    _pad: [u8; 6],
}

const _: () = assert!(
    core::mem::size_of::<WaiterSlot>() <= 64,
    "WaiterSlot doit tenir dans 64 bytes"
);

impl WaiterSlot {
    const fn new() -> Self {
        WaiterSlot {
            txn_id: DmaTransactionId::INVALID,
            channel_id: DmaChannelId(u32::MAX),
            waiter_tid: AtomicU64::new(0),
            state: AtomicU8::new(WaiterState::Free as u8),
            result_ok: AtomicBool::new(false),
            result_val: AtomicU64::new(0),
            _pad: [0u8; 6],
        }
    }

    fn is_free(&self) -> bool {
        self.state.load(Ordering::Acquire) == WaiterState::Free as u8
    }

    fn is_waiting_for(&self, txn_id: DmaTransactionId) -> bool {
        self.state.load(Ordering::Acquire) == WaiterState::Waiting as u8
            && self.txn_id.0 == txn_id.0
    }

    /// Marks the slot as waiting for a transaction.
    ///
    /// # Safety
    /// Must be called only on a `Free` slot.
    unsafe fn mark_waiting(&self, txn_id: DmaTransactionId, channel_id: DmaChannelId, tid: u64) {
        let ptr = self as *const _ as *mut WaiterSlot;
        (*ptr).txn_id = txn_id;
        (*ptr).channel_id = channel_id;
        self.waiter_tid.store(tid, Ordering::Relaxed);
        self.result_ok.store(false, Ordering::Relaxed);
        self.result_val.store(0, Ordering::Relaxed);
        self.state
            .store(WaiterState::Waiting as u8, Ordering::Release);
    }

    /// Completes the slot with a result.
    fn complete(&self, result: Result<usize, DmaError>) {
        match result {
            Ok(bytes) => {
                self.result_ok.store(true, Ordering::Relaxed);
                self.result_val.store(bytes as u64, Ordering::Relaxed);
            }
            Err(err) => {
                self.result_ok.store(false, Ordering::Relaxed);
                self.result_val.store(err as u64, Ordering::Relaxed);
            }
        }
        self.state
            .store(WaiterState::Completed as u8, Ordering::Release);
    }

    /// Releases the slot (returns to Free state).
    fn release(&self) {
        self.state.store(WaiterState::Free as u8, Ordering::Release);
    }

    #[allow(dead_code)]
    fn result(&self) -> Result<usize, DmaError> {
        if self.result_ok.load(Ordering::Acquire) {
            Ok(self.result_val.load(Ordering::Relaxed) as usize)
        } else {
            Err(match self.result_val.load(Ordering::Relaxed) {
                0 => DmaError::NoChannel,
                1 => DmaError::OutOfMemory,
                2 => DmaError::InvalidParams,
                3 => DmaError::Timeout,
                4 => DmaError::HardwareError,
                5 => DmaError::IommuFault,
                6 => DmaError::NotInitialized,
                7 => DmaError::AlreadySubmitted,
                8 => DmaError::Cancelled,
                9 => DmaError::MisalignedBuffer,
                10 => DmaError::WrongZone,
                _ => DmaError::NotSupported,
            })
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WAITERS TABLE
// ─────────────────────────────────────────────────────────────────────────────

/// Global DMA waiters table.
///
/// Allocates one slot per pending transaction. When the DMA IRQ arrives,
/// `notify_completion` finds the slot, calls the WakeupHandler,
/// then frees the slot.
pub struct DmaWakeupTable {
    slots: [WaiterSlot; WAKEUP_TABLE_SIZE],
    /// Number of currently occupied slots.
    pub active_count: core::sync::atomic::AtomicU32,
    /// Successfully delivered wakeups.
    pub delivered: AtomicU64,
    /// Orphan notifications (unknown transaction).
    pub orphans: AtomicU64,
    /// Timeouts detected during cleanup.
    pub timeouts: AtomicU64,
}

// SAFETY: Slots are accessed via CAS on state (Waiting -> Completed).
unsafe impl Sync for DmaWakeupTable {}
unsafe impl Send for DmaWakeupTable {}

impl Default for DmaWakeupTable {
    fn default() -> Self {
        Self::new()
    }
}

impl DmaWakeupTable {
    const fn new() -> Self {
        DmaWakeupTable {
            slots: [const { WaiterSlot::new() }; WAKEUP_TABLE_SIZE],
            active_count: core::sync::atomic::AtomicU32::new(0),
            delivered: AtomicU64::new(0),
            orphans: AtomicU64::new(0),
            timeouts: AtomicU64::new(0),
        }
    }

    /// Registers a waiter for `txn_id`.
    ///
    /// The thread identified by `tid` will be awakened by `DmaWakeupHandler`
    /// upon completion of the transaction.
    ///
    /// Returns `false` if the table is full.
    pub fn register(&self, txn_id: DmaTransactionId, channel_id: DmaChannelId, tid: u64) -> bool {
        for slot in self.slots.iter() {
            if slot.is_free() {
                // Attempt a CAS Free -> Waiting to prevent races.
                if slot
                    .state
                    .compare_exchange(
                        WaiterState::Free as u8,
                        WaiterState::Waiting as u8,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    // SAFETY: We just passed the CAS, we are the sole owner.
                    unsafe {
                        slot.mark_waiting(txn_id, channel_id, tid);
                    }
                    self.active_count.fetch_add(1, Ordering::Relaxed);
                    return true;
                }
            }
        }
        false
    }

    /// Notifies the completion of a transaction and wakes up the waiter.
    ///
    /// Called from the DMA interrupt handler.
    /// Finds the slot corresponding to `txn_id`, calls `wake_on_completion`,
    /// then frees the slot.
    ///
    /// Returns `true` if a waiter was found and notified.
    pub fn notify_completion(
        &self,
        txn_id: DmaTransactionId,
        result: Result<usize, DmaError>,
    ) -> bool {
        for slot in self.slots.iter() {
            if slot.is_waiting_for(txn_id) {
                let tid = slot.waiter_tid.load(Ordering::Acquire);
                slot.complete(result);

                // Call into the WakeupHandler registered by process/.
                wake_on_completion(tid, txn_id, result);

                self.delivered.fetch_add(1, Ordering::Relaxed);
                self.active_count.fetch_sub(1, Ordering::Relaxed);
                slot.release();
                return true;
            }
        }
        self.orphans.fetch_add(1, Ordering::Relaxed);
        false
    }

    /// Number of active waiters.
    pub fn active(&self) -> u32 {
        self.active_count.load(Ordering::Relaxed)
    }
}

/// Global DMA wakeup table.
pub static DMA_WAKEUP_TABLE: DmaWakeupTable = DmaWakeupTable::new();

// ─────────────────────────────────────────────────────────────────────────────
// PUBLIC API
// ─────────────────────────────────────────────────────────────────────────────

/// Registers a waiter for a DMA transaction.
#[inline]
pub fn register_wakeup(txn_id: DmaTransactionId, channel_id: DmaChannelId, tid: u64) -> bool {
    DMA_WAKEUP_TABLE.register(txn_id, channel_id, tid)
}

/// Notifies transaction completion (called by IRQ handler).
#[inline]
pub fn notify_completion(txn_id: DmaTransactionId, result: Result<usize, DmaError>) -> bool {
    DMA_WAKEUP_TABLE.notify_completion(txn_id, result)
}
