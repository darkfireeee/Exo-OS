// kernel/src/process/state/wakeup.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ProcessWakeupHandler impl DmaWakeupHandler — RULE PROC-02 (DOC4)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Implements the DmaWakeupHandler trait from memory/ so that the DMA subsystem
// can wake a thread waiting for a DMA transaction.
//
// Dependency inversion:
//   memory/ defines DmaWakeupHandler (trait).
//   process/ implements it here (ProcessWakeupHandler).
//   process/mod.rs::init() calls register_with_dma() to register.

use crate::memory::dma::core::types::{DmaError, DmaTransactionId};
use crate::memory::dma::core::wakeup_iface::DmaWakeupHandler;
use crate::scheduler::sync::wait_queue::WaitQueue;
use core::sync::atomic::Ordering;

// ─────────────────────────────────────────────────────────────────────────────
// Global wait queue for threads waiting on DMA
// ─────────────────────────────────────────────────────────────────────────────

/// Global wait queue: threads waiting for DMA completion.
static DMA_WAIT_QUEUE: WaitQueue = WaitQueue::new();

// ─────────────────────────────────────────────────────────────────────────────
// DMA result registry by TID
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum capacity of pending DMA completion registry.
const DMA_COMPLETION_SLOTS: usize = 256;

/// DMA completion slot.
#[repr(C)]
struct DmaCompletionSlot {
    /// Target TID (0 = free slot).
    tid: core::sync::atomic::AtomicU64,
    /// Transaction ID.
    txn_id: core::sync::atomic::AtomicU64,
    /// Result (0 = OK, negative errno = error).
    result: core::sync::atomic::AtomicI64,
}

impl DmaCompletionSlot {
    const fn empty() -> Self {
        Self {
            tid: core::sync::atomic::AtomicU64::new(0),
            txn_id: core::sync::atomic::AtomicU64::new(0),
            result: core::sync::atomic::AtomicI64::new(0),
        }
    }
}

/// Static array of completion slots.
static DMA_COMPLETIONS: [DmaCompletionSlot; DMA_COMPLETION_SLOTS] =
    [const { DmaCompletionSlot::empty() }; DMA_COMPLETION_SLOTS];

/// Records transaction completion for thread `tid`.
fn store_completion(tid: u64, txn_id: u64, result: i64) {
    // Find a free slot via simple round-robin.
    for slot in &DMA_COMPLETIONS {
        if slot
            .tid
            .compare_exchange(0, tid, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            slot.txn_id.store(txn_id, Ordering::Release);
            slot.result.store(result, Ordering::Release);
            return;
        }
    }
    // Table full: ignore (the thread will notice the timeout).
}

/// Retrieves and clears the completion for thread `tid` and transaction `txn_id`.
/// Returns Some(result) or None.
pub fn consume_completion(tid: u64, txn_id: u64) -> Option<i64> {
    for slot in &DMA_COMPLETIONS {
        let stored_tid = slot.tid.load(Ordering::Acquire);
        if stored_tid == tid {
            let stored_txn = slot.txn_id.load(Ordering::Acquire);
            if stored_txn == txn_id {
                let result = slot.result.load(Ordering::Acquire);
                slot.tid.store(0, Ordering::Release);
                return Some(result);
            }
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// ProcessWakeupHandler
// ─────────────────────────────────────────────────────────────────────────────

/// DmaWakeupHandler implementation for the process/ module.
/// Single static instance registered with memory/dma/ at init.
pub struct ProcessWakeupHandler;

/// Converts a DmaError to a negative errno code (Linux-compatible).
#[inline]
fn dma_error_to_errno(e: DmaError) -> i64 {
    match e {
        DmaError::NoChannel => -11,        // EAGAIN
        DmaError::OutOfMemory => -12,      // ENOMEM
        DmaError::InvalidParams => -22,    // EINVAL
        DmaError::Timeout => -110,         // ETIMEDOUT
        DmaError::HardwareError => -5,     // EIO
        DmaError::IommuFault => -14,       // EFAULT
        DmaError::NotInitialized => -6,    // ENXIO
        DmaError::AlreadySubmitted => -22, // EINVAL
        DmaError::Cancelled => -125,       // ECANCELED
        DmaError::MisalignedBuffer => -22, // EINVAL
        DmaError::WrongZone => -22,        // EINVAL
        DmaError::NotSupported => -95,     // ENOTSUP
    }
}

impl DmaWakeupHandler for ProcessWakeupHandler {
    /// Called by the DMA driver when a transaction completes.
    /// Wakes up thread `tid` waiting in DMA_WAIT_QUEUE.
    fn wake_on_completion(
        &self,
        tid: u64,
        txn_id: DmaTransactionId,
        result: Result<usize, DmaError>,
    ) {
        // Convert result to i64 (bytes or negative errno)
        let result_i64 = match result {
            Ok(bytes) => bytes as i64,
            Err(e) => dma_error_to_errno(e),
        };
        // Store result so the thread can read it after wakeup.
        store_completion(tid, txn_id.0, result_i64);
        // Wake all threads waiting for DMA;
        // each will check if it matches its TID.
        DMA_WAIT_QUEUE.notify_all();
    }

    /// Called by the DMA driver on fatal error on a channel.
    /// Wakes all threads waiting on this channel.
    fn wake_all_on_error(&self, channel_id: u32, error: DmaError) {
        let errno = dma_error_to_errno(error);
        // Mark all transactions on this channel as failed.
        for slot in &DMA_COMPLETIONS {
            let tid_stored = slot.tid.load(Ordering::Acquire);
            if tid_stored != 0 {
                let txn = slot.txn_id.load(Ordering::Acquire);
                // Channel TXN IDs contain channel_id in the high bits.
                if (txn >> 32) as u32 == channel_id {
                    slot.result.store(errno, Ordering::Release);
                }
            }
        }
        DMA_WAIT_QUEUE.notify_all();
    }
}

/// Static instance registered with memory/dma/.
pub static PROCESS_WAKEUP_HANDLER: ProcessWakeupHandler = ProcessWakeupHandler;

/// Registers PROCESS_WAKEUP_HANDLER with the DMA subsystem.
/// Called from process::init() (RULE PROC-02).
pub fn register_with_dma() {
    // SAFETY: called once from process::init() after scheduler::init().
    unsafe {
        crate::memory::dma::register_wakeup_handler(&PROCESS_WAKEUP_HANDLER);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point for threads waiting on DMA
// ─────────────────────────────────────────────────────────────────────────────

/// Waits for completion of a DMA transaction.
/// The current thread blocks in DMA_WAIT_QUEUE until woken up.
/// Returns result or -EINTR if signal received.
pub fn wait_for_dma(
    tcb: &crate::scheduler::core::task::ThreadControlBlock,
    tid: u64,
    txn_id: u64,
) -> i64 {
    loop {
        // Check if completion has already arrived (no spurious).
        if let Some(result) = consume_completion(tid, txn_id) {
            return result;
        }
        // Check for signal before blocking.
        if tcb.has_signal_pending() {
            return -4; // -EINTR
        }
        // SAFETY: tcb points to the current TCB, no active &mut alias.
        unsafe {
            DMA_WAIT_QUEUE.wait_interruptible(tcb as *const _ as *mut _);
        }
    }
}
