// kernel/src/memory/dma/core/wakeup_iface.rs
//
// DMA wakeup interface — CRITICAL ARCHITECTURAL RULE.
//
// The DMA subsystem (Layer 0) must be able to wake up processes
// upon transfer completion WITHOUT depending on process/ or scheduler/.
// This is resolved via dependency inversion:
//   — DMA defines the `DmaWakeupHandler` trait here (Layer 0).
//   — process/ implements this trait and registers it via `register_wakeup_handler()`.
//
// LAYER 0 — no dependencies towards scheduler/ipc/fs/process.

use crate::memory::dma::core::types::{DmaError, DmaTransactionId};
use core::sync::atomic::{AtomicUsize, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// WAKEUP TRAIT
// ─────────────────────────────────────────────────────────────────────────────

/// Trait that the process/ subsystem must implement to receive
/// DMA transfer completion notifications.
///
/// This trait is intentionally minimal to limit coupling.
pub trait DmaWakeupHandler: Send + Sync {
    /// Wakes the waiter associated with `tid` when a DMA transaction completes.
    ///
    /// `tid` — identifier of thread/process to wake.
    /// `txn_id` — completed transaction.
    /// `result` — `Ok(bytes_transferred)` or `Err(DmaError)`.
    fn wake_on_completion(
        &self,
        tid: u64,
        txn_id: DmaTransactionId,
        result: Result<usize, DmaError>,
    );

    /// Notifies that a fatal error occurred on a channel.
    /// All waiters on this channel must be unblocked with an error.
    fn wake_all_on_error(&self, channel_id: u32, error: DmaError);
}

// ─────────────────────────────────────────────────────────────────────────────
// DEFAULT HANDLER (NO-OP)
// ─────────────────────────────────────────────────────────────────────────────

/// No-op handler used before registration from process/.
/// Logs via lost calls counter to detect orphan notifications.
struct NopWakeupHandler {
    lost_wakeups: AtomicUsize,
}

impl DmaWakeupHandler for NopWakeupHandler {
    fn wake_on_completion(
        &self,
        _tid: u64,
        _txn_id: DmaTransactionId,
        _result: Result<usize, DmaError>,
    ) {
        self.lost_wakeups.fetch_add(1, Ordering::Relaxed);
    }

    fn wake_all_on_error(&self, _channel_id: u32, _error: DmaError) {
        self.lost_wakeups.fetch_add(1, Ordering::Relaxed);
    }
}

static NOP_HANDLER: NopWakeupHandler = NopWakeupHandler {
    lost_wakeups: AtomicUsize::new(0),
};

/// Returns the number of lost wakeups (handler not yet registered).
pub fn lost_wakeup_count() -> usize {
    NOP_HANDLER.lost_wakeups.load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// GLOBAL HANDLER REGISTRY
// ─────────────────────────────────────────────────────────────────────────────

/// Atomic pointer to active handler.
/// Encodes `*const dyn DmaWakeupHandler` via two words (fat pointer = 2 × usize).
///
/// In no_std/bare-metal the fat pointer is encoded as two adjacent AtomicUsize.
/// The registry is write-once: dynamic swapping is not supported.
struct WakeupHandlerSlot {
    data: AtomicUsize,
    vtbl: AtomicUsize,
    registered: AtomicUsize, // 0=nop, 1=real handler
}

// SAFETY: Slot is write-once; reads are protected by Acquire fence.
unsafe impl Sync for WakeupHandlerSlot {}

static WAKEUP_SLOT: WakeupHandlerSlot = WakeupHandlerSlot {
    data: AtomicUsize::new(0),
    vtbl: AtomicUsize::new(0),
    registered: AtomicUsize::new(0),
};

/// Registers the DMA wakeup handler.
///
/// # Safety
/// - Must be called exactly once, at boot, from process/ or kernel init.
/// - `handler` must have static lifetime (`'static`).
/// - No DMA transaction may be in progress during the call.
pub unsafe fn register_wakeup_handler(handler: &'static dyn DmaWakeupHandler) {
    let fat: (*const (), *const ()) = core::mem::transmute(handler);
    WAKEUP_SLOT.data.store(fat.0 as usize, Ordering::Release);
    WAKEUP_SLOT.vtbl.store(fat.1 as usize, Ordering::Release);
    WAKEUP_SLOT.registered.store(1, Ordering::Release);
}

/// Retrieves active handler (nop if not registered).
#[inline]
fn active_handler() -> &'static dyn DmaWakeupHandler {
    if WAKEUP_SLOT.registered.load(Ordering::Acquire) == 0 {
        return &NOP_HANDLER;
    }
    let data = WAKEUP_SLOT.data.load(Ordering::Acquire) as *const ();
    let vtbl = WAKEUP_SLOT.vtbl.load(Ordering::Acquire) as *const ();
    // SAFETY: Fat pointer was properly stored via register_wakeup_handler().
    unsafe {
        let fat: (*const (), *const ()) = (data, vtbl);
        core::mem::transmute::<(*const (), *const ()), &'static dyn DmaWakeupHandler>(fat)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PUBLIC FUNCTIONS
// ─────────────────────────────────────────────────────────────────────────────

/// Wakes a thread/process after completion of a DMA transaction.
#[inline]
pub fn wake_on_completion(tid: u64, txn_id: DmaTransactionId, result: Result<usize, DmaError>) {
    active_handler().wake_on_completion(tid, txn_id, result);
}

/// Wakes all waiters on a channel upon fatal error.
#[inline]
pub fn wake_all_on_error(channel_id: u32, error: DmaError) {
    active_handler().wake_all_on_error(channel_id, error);
}

/// Indicates whether a real handler has been registered.
#[inline]
pub fn has_real_handler() -> bool {
    WAKEUP_SLOT.registered.load(Ordering::Relaxed) != 0
}
