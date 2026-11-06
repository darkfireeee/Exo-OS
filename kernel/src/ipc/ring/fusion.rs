// kernel/src/ipc/ring/fusion.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// FUSION RING — Adaptive batching ring
// (Exo-OS · IPC Layer 2a)
// ═══════════════════════════════════════════════════════════════════════════════
//
// The Fusion Ring dynamically adapts its strategy between:
//   • DIRECT MODE   : each message sent immediately (low load)
//   • BATCH MODE    : accumulation up to THRESHOLD or delay (high load)
//
// The decision is based on a counter of in-flight messages and an EWA
// (Exponential Weighted Average) of observed throughput.
//
// INVARIANTS:
//   • Guaranteed latency <= FUSION_MAX_DELAY_TICKS x tick_period.
//   • Overflow impossible: if BatchBuffer is full -> immediate flush.
//   • Thread-safe for a single producer and a single consumer.
// ═══════════════════════════════════════════════════════════════════════════════

use super::batch::BatchBuffer;
use super::spsc::SpscRing;
use crate::ipc::core::{IpcError, MsgFlags};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Operating mode of the Fusion Ring
// ─────────────────────────────────────────────────────────────────────────────

/// Current mode of the Fusion Ring.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum FusionMode {
    /// Direct transmission without buffering (low load).
    Direct = 0,
    /// Accumulation + deferred flush (high load).
    Batch = 1,
}

// ─────────────────────────────────────────────────────────────────────────────
// FusionMetrics — adaptive metrics
// ─────────────────────────────────────────────────────────────────────────────

/// Counters for adaptation decision making.
struct FusionMetrics {
    /// Messages sent since last observation period.
    msgs_since_last: AtomicU64,
    /// Last observation tick.
    last_obs_tick: AtomicU64,
    /// EWA of throughput in msgs/tick.
    ewa_throughput: AtomicU32,
    /// Number of mode switches (diagnostic).
    mode_switches: AtomicU64,
}

impl FusionMetrics {
    const fn new() -> Self {
        Self {
            msgs_since_last: AtomicU64::new(0),
            last_obs_tick: AtomicU64::new(0),
            ewa_throughput: AtomicU32::new(0),
            mode_switches: AtomicU64::new(0),
        }
    }

    /// Updates throughput EWA. Called at each scheduler tick.
    /// EWA formula: ewa = (ewa * 7 + instant) >> 3
    fn update(&self, current_tick: u64) {
        let last = self.last_obs_tick.load(Ordering::Relaxed);
        if current_tick == last {
            return; // same tick, no update
        }
        let dt = (current_tick - last).max(1);
        let msgs = self.msgs_since_last.swap(0, Ordering::Relaxed);
        let instant = (msgs / dt).min(u32::MAX as u64) as u32;
        let ewa =
            ((self.ewa_throughput.load(Ordering::Relaxed) as u64 * 7 + instant as u64) >> 3) as u32;
        self.ewa_throughput.store(ewa, Ordering::Relaxed);
        self.last_obs_tick.store(current_tick, Ordering::Relaxed);
    }

    /// Returns EWA throughput in msgs/tick.
    fn throughput_ewa(&self) -> u32 {
        self.ewa_throughput.load(Ordering::Relaxed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FusionRing
// ─────────────────────────────────────────────────────────────────────────────

/// Throughput threshold to switch to Batch mode (msgs/tick).
const BATCH_THRESHOLD_THROUGHPUT: u32 = 100;
/// Throughput threshold to switch back to Direct mode.
const DIRECT_THRESHOLD_THROUGHPUT: u32 = 20;

/// Adaptive ring — Direct mode below threshold, Batch mode above.
pub struct FusionRing {
    /// Underlying ring (SPSC).
    inner: SpscRing,
    /// Producer-side batch buffer.
    batch: BatchBuffer,
    /// Current mode.
    mode: AtomicU32, // 0 = Direct, 1 = Batch
    /// Metrics for adaptation.
    metrics: FusionMetrics,
    /// Current tick (updated by scheduler tick handler).
    current_tick: AtomicU64,
}

impl FusionRing {
    /// Creates a FusionRing in Direct mode.
    pub const fn new() -> Self {
        Self {
            inner: SpscRing::new(),
            batch: BatchBuffer::new(),
            mode: AtomicU32::new(0),
            metrics: FusionMetrics::new(),
            current_tick: AtomicU64::new(0),
        }
    }

    /// Initializes underlying ring.
    pub fn init(&self) {
        self.inner.init();
    }

    /// Updates current tick (called from scheduler tick handler).
    #[inline(always)]
    pub fn tick(&self, tick: u64) {
        self.current_tick.store(tick, Ordering::Relaxed);
        self.metrics.update(tick);
        self.adapt_mode();
    }

    /// Adapts mode according to observed throughput.
    fn adapt_mode(&self) {
        let ewa = self.metrics.throughput_ewa();
        let mode = self.mode.load(Ordering::Relaxed);
        if mode == 0 && ewa >= BATCH_THRESHOLD_THROUGHPUT {
            self.mode.store(1, Ordering::Relaxed);
            self.metrics.mode_switches.fetch_add(1, Ordering::Relaxed);
        } else if mode == 1 && ewa <= DIRECT_THRESHOLD_THROUGHPUT {
            self.mode.store(0, Ordering::Relaxed);
            self.metrics.mode_switches.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Sends a message (adaptive).
    pub fn send(&mut self, src: &[u8], flags: MsgFlags) -> Result<(), IpcError> {
        let tick = self.current_tick.load(Ordering::Relaxed);
        self.metrics.msgs_since_last.fetch_add(1, Ordering::Relaxed);

        let mode = self.mode.load(Ordering::Relaxed);
        if mode == 0 {
            // DIRECT MODE — immediate transmission.
            self.inner.push_copy(src, flags).map(|_| ())
        } else {
            // BATCH MODE — accumulate then flush if necessary.
            let should_flush = self.batch.add(src, flags, tick);
            if should_flush || self.batch.is_expired(tick) {
                let _ = self.batch.flush_to_ring(&self.inner);
            }
            Ok(())
        }
    }

    /// Forced flush of batch (e.g., end of quantum, channel closing).
    pub fn flush(&mut self) -> usize {
        if self.batch.count() > 0 {
            self.batch.flush_to_ring(&self.inner)
        } else {
            0
        }
    }

    /// Receives a message.
    pub fn recv(&self, dst: &mut [u8]) -> Result<(usize, MsgFlags), IpcError> {
        self.inner.pop_into(dst)
    }

    /// Current ring mode.
    #[inline(always)]
    pub fn mode(&self) -> FusionMode {
        if self.mode.load(Ordering::Relaxed) == 0 {
            FusionMode::Direct
        } else {
            FusionMode::Batch
        }
    }

    /// Number of pending messages in the ring.
    #[inline(always)]
    pub fn pending(&self) -> usize {
        self.inner.len_approx() + self.batch.count()
    }

    /// Diagnostic statistics.
    pub fn mode_switches(&self) -> u64 {
        self.metrics.mode_switches.load(Ordering::Relaxed)
    }

    pub fn throughput_ewa(&self) -> u32 {
        self.metrics.throughput_ewa()
    }
}
