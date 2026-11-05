// kernel/src/memory/dma/completion/polling.rs
//
// High-frequency polling for DMA completion.
//
// Used when a kernel thread must wait for the completion of a DMA transfer
// without going through the wakeup mechanism (contexts without active scheduler:
// init, early_boot, embedded drivers).
//
// Two modes:
//   - `spin_poll`: busy spin until completion or timeout (TSC-based).
//   - `yield_poll`: same but with `pause` (REPNE) between reads.
//
// LAYER 0 — no external dependencies. No scheduler::sleep().

use core::sync::atomic::{AtomicU64, Ordering};

use crate::memory::dma::completion::handler::DMA_COMPLETION;
use crate::memory::dma::core::types::{DmaError, DmaTransactionId};

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTS
// ─────────────────────────────────────────────────────────────────────────────

/// Default timeout in TSC cycles (~10 ms at 3 GHz).
pub const DEFAULT_POLL_TIMEOUT_CYCLES: u64 = 30_000_000;

/// Long timeout for large DMA transfers (~1 s at 3 GHz).
pub const LONG_POLL_TIMEOUT_CYCLES: u64 = 3_000_000_000;

/// Number of TSC reads without `pause` before inserting a `pause`.
const SPIN_BATCH_SIZE: u32 = 64;

// ─────────────────────────────────────────────────────────────────────────────
// POLLING RESULT
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a polling operation.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum PollResult {
    /// Transfer completed successfully. Transferred bytes.
    Done(usize),
    /// Transfer completed with error.
    Error(DmaError),
    /// Timeout expired without completion.
    Timeout,
    /// Transaction is not in the completion table.
    NotFound,
}

// ─────────────────────────────────────────────────────────────────────────────
// POLLING STATISTICS
// ─────────────────────────────────────────────────────────────────────────────

pub struct PollStats {
    /// Successful polls (Done).
    pub successes: AtomicU64,
    /// Failed polls (Error).
    pub errors: AtomicU64,
    /// Expired polls.
    pub timeouts: AtomicU64,
    /// Total TSC cycles consumed by successful polls.
    pub cycles_total: AtomicU64,
    /// Maximum TSC cycles on a successful poll (worst-case latency).
    pub cycles_max: AtomicU64,
}

impl PollStats {
    const fn new() -> Self {
        PollStats {
            successes: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            timeouts: AtomicU64::new(0),
            cycles_total: AtomicU64::new(0),
            cycles_max: AtomicU64::new(0),
        }
    }
}

pub static POLL_STATS: PollStats = PollStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// POLLING ENGINE
// ─────────────────────────────────────────────────────────────────────────────

/// Busy-spin DMA polling.
///
/// Spins until:
///   - The transaction is marked `Done`/`Error` in `DMA_COMPLETION`,
///   - Or `timeout_cycles` TSC cycles have elapsed.
///
/// # Parameters
/// - `txn_id`: transaction to wait for.
/// - `timeout_cycles`: max duration in TSC cycles (0 = no timeout).
///
/// # Context
/// Can be called with interrupts disabled.
/// Must NOT be called from an interrupt context (recursion).
pub fn spin_poll(txn_id: DmaTransactionId, timeout_cycles: u64) -> PollResult {
    let start = read_tsc();
    let mut batch = 0u32;

    loop {
        // Query state in the completion table.
        match DMA_COMPLETION.query(txn_id) {
            Some(Ok(bytes)) => {
                let elapsed = read_tsc().wrapping_sub(start);
                POLL_STATS.successes.fetch_add(1, Ordering::Relaxed);
                POLL_STATS
                    .cycles_total
                    .fetch_add(elapsed, Ordering::Relaxed);
                // Update max without lock (acceptable approximation).
                let old_max = POLL_STATS.cycles_max.load(Ordering::Relaxed);
                if elapsed > old_max {
                    POLL_STATS.cycles_max.store(elapsed, Ordering::Relaxed);
                }
                return PollResult::Done(bytes);
            }
            Some(Err(err)) => {
                POLL_STATS.errors.fetch_add(1, Ordering::Relaxed);
                return PollResult::Error(err);
            }
            None => {
                // Transaction not yet completed — check timeout.
                if timeout_cycles > 0 {
                    let elapsed = read_tsc().wrapping_sub(start);
                    if elapsed >= timeout_cycles {
                        POLL_STATS.timeouts.fetch_add(1, Ordering::Relaxed);
                        return PollResult::Timeout;
                    }
                }
            }
        }

        // `pause` every SPIN_BATCH_SIZE iterations to avoid saturating
        // the memory load execution port and allow progress of
        // the producer thread (hyper-threading).
        batch += 1;
        if batch >= SPIN_BATCH_SIZE {
            batch = 0;
            cpu_pause();
        }
    }
}

/// Variant of `spin_poll` with a bounded number of iterations.
///
/// Returns `PollResult::Timeout` if the transaction is not completed
/// after `max_iters` reads, without taking TSC into account.
///
/// Useful in contexts where reading the TSC is prohibited.
pub fn bounded_poll(txn_id: DmaTransactionId, max_iters: u32) -> PollResult {
    for i in 0..max_iters {
        match DMA_COMPLETION.query(txn_id) {
            Some(Ok(bytes)) => {
                POLL_STATS.successes.fetch_add(1, Ordering::Relaxed);
                return PollResult::Done(bytes);
            }
            Some(Err(err)) => {
                POLL_STATS.errors.fetch_add(1, Ordering::Relaxed);
                return PollResult::Error(err);
            }
            None => {}
        }
        if i % SPIN_BATCH_SIZE == SPIN_BATCH_SIZE - 1 {
            cpu_pause();
        }
    }
    POLL_STATS.timeouts.fetch_add(1, Ordering::Relaxed);
    PollResult::Timeout
}

/// Polling with default timeout (`DEFAULT_POLL_TIMEOUT_CYCLES`).
#[inline]
pub fn poll(txn_id: DmaTransactionId) -> PollResult {
    spin_poll(txn_id, DEFAULT_POLL_TIMEOUT_CYCLES)
}

/// Polling with long timeout (large transfers).
#[inline]
pub fn poll_long(txn_id: DmaTransactionId) -> PollResult {
    spin_poll(txn_id, LONG_POLL_TIMEOUT_CYCLES)
}

// ─────────────────────────────────────────────────────────────────────────────
// UTILITIES
// ─────────────────────────────────────────────────────────────────────────────

/// Reads the TSC.
#[inline(always)]
fn read_tsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: rdtsc available on all x86_64; options(nostack, nomem) correct.
    unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
        ((hi as u64) << 32) | (lo as u64)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

/// Instruction `pause` (REPNE NOP) — reduces cache coherence contention.
#[inline(always)]
fn cpu_pause() {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: pause available on x86_64; reduces cache coherence contention.
    unsafe {
        core::arch::asm!("pause", options(nostack, nomem, preserves_flags));
    }
}
