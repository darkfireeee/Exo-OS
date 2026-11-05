// kernel/src/memory/dma/stats/counters.rs
//
// DMA statistics counters — throughput, latency, errors, timeouts.
// Layer 0 — no external dependencies except `core`.
//
// Each DMA engine has its own set of counters, plus an
// aggregated global counter. All accesses are atomic (relaxed for
// performance counters, AcqRel for fatal errors).

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// PER-ENGINE STATISTICS
// ─────────────────────────────────────────────────────────────────────────────

/// Statistics of an individual DMA engine.
#[repr(C, align(64))]
pub struct DmaEngineStats {
    /// Number of submitted transfers.
    pub submitted: AtomicU64,
    /// Number of successfully completed transfers.
    pub completed: AtomicU64,
    /// Number of hard errors (ECC, IOMMU timeout, etc.).
    pub errors: AtomicU64,
    /// Number of timeouts (forced cancellation).
    pub timeouts: AtomicU64,
    /// Bytes transferred (DMA → RAM or RAM → device).
    pub bytes_tx: AtomicU64,
    /// Bytes received (device → RAM).
    pub bytes_rx: AtomicU64,
    /// Cumulative latency in TSC cycles (for average computation).
    pub latency_sum_cycles: AtomicU64,
    /// Maximum observed latency in TSC cycles.
    pub latency_max_cycles: AtomicU64,
    /// Number of in-flight transactions (in progress).
    pub in_flight: AtomicU32,
    /// Number of re-submissions (retry after transient error).
    pub retries: AtomicU32,
}

impl Default for DmaEngineStats {
    fn default() -> Self {
        Self::new()
    }
}

impl DmaEngineStats {
    pub const fn new() -> Self {
        DmaEngineStats {
            submitted: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            timeouts: AtomicU64::new(0),
            bytes_tx: AtomicU64::new(0),
            bytes_rx: AtomicU64::new(0),
            latency_sum_cycles: AtomicU64::new(0),
            latency_max_cycles: AtomicU64::new(0),
            in_flight: AtomicU32::new(0),
            retries: AtomicU32::new(0),
        }
    }

    /// Records transfer submission.
    #[inline]
    pub fn on_submit(&self) {
        self.submitted.fetch_add(1, Ordering::Relaxed);
        self.in_flight.fetch_add(1, Ordering::Relaxed);
    }

    /// Records transfer completion.
    ///
    /// `bytes`:      number of bytes transferred.
    /// `is_write`:   true = DMA from RAM (TX), false = to RAM (RX).
    /// `latency_cy`: latency in TSC cycles.
    #[inline]
    pub fn on_complete(&self, bytes: u64, is_write: bool, latency_cy: u64) {
        self.completed.fetch_add(1, Ordering::Relaxed);
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
        if is_write {
            self.bytes_tx.fetch_add(bytes, Ordering::Relaxed);
        } else {
            self.bytes_rx.fetch_add(bytes, Ordering::Relaxed);
        }
        self.latency_sum_cycles
            .fetch_add(latency_cy, Ordering::Relaxed);
        // Update max latency (compare-and-swap loop).
        let mut cur = self.latency_max_cycles.load(Ordering::Relaxed);
        while latency_cy > cur {
            match self.latency_max_cycles.compare_exchange_weak(
                cur,
                latency_cy,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => cur = v,
            }
        }
    }

    /// Records a transfer error.
    #[inline]
    pub fn on_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
    }

    /// Records a timeout.
    #[inline]
    pub fn on_timeout(&self) {
        self.timeouts.fetch_add(1, Ordering::AcqRel);
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
    }

    /// Records a re-submission.
    #[inline]
    pub fn on_retry(&self) {
        self.retries.fetch_add(1, Ordering::Relaxed);
    }

    /// Average latency in cycles (0 if no completions).
    pub fn avg_latency_cycles(&self) -> u64 {
        let completed = self.completed.load(Ordering::Relaxed);
        if completed == 0 {
            return 0;
        }
        self.latency_sum_cycles.load(Ordering::Relaxed) / completed
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GLOBAL DMA STATISTICS
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of tracked DMA engines.
pub const MAX_DMA_ENGINES: usize = 16;

/// Global DMA statistics, aggregated across all engines.
pub struct DmaStats {
    /// Per-engine statistics (index = engine_id).
    pub engines: [DmaEngineStats; MAX_DMA_ENGINES],
    /// Number of active engines.
    pub active_engines: AtomicU32,
    /// Global counter of fatal IOMMU errors.
    pub iommu_faults: AtomicU64,
    /// Number of IOMMU TLB flushes performed.
    pub iommu_tlb_flushes: AtomicU64,
    /// Number of domain resets.
    pub domain_resets: AtomicU64,
}

// SAFETY: DmaStats is only accessible via atomic methods.
unsafe impl Sync for DmaStats {}

impl Default for DmaStats {
    fn default() -> Self {
        Self::new()
    }
}

impl DmaStats {
    const fn new() -> Self {
        DmaStats {
            engines: [const { DmaEngineStats::new() }; MAX_DMA_ENGINES],
            active_engines: AtomicU32::new(0),
            iommu_faults: AtomicU64::new(0),
            iommu_tlb_flushes: AtomicU64::new(0),
            domain_resets: AtomicU64::new(0),
        }
    }

    /// Returns engine stats by its ID.
    #[inline]
    pub fn engine(&self, id: usize) -> Option<&DmaEngineStats> {
        self.engines.get(id)
    }

    /// Registers an additional active engine.
    #[inline]
    pub fn register_engine(&self) -> usize {
        let id = self.active_engines.fetch_add(1, Ordering::AcqRel) as usize;
        id.min(MAX_DMA_ENGINES - 1)
    }

    /// Aggregated counters of all active engines.
    pub fn total_submitted(&self) -> u64 {
        let n = self.active_engines.load(Ordering::Relaxed) as usize;
        self.engines[..n.min(MAX_DMA_ENGINES)]
            .iter()
            .map(|e| e.submitted.load(Ordering::Relaxed))
            .sum()
    }

    pub fn total_completed(&self) -> u64 {
        let n = self.active_engines.load(Ordering::Relaxed) as usize;
        self.engines[..n.min(MAX_DMA_ENGINES)]
            .iter()
            .map(|e| e.completed.load(Ordering::Relaxed))
            .sum()
    }

    pub fn total_errors(&self) -> u64 {
        let n = self.active_engines.load(Ordering::Relaxed) as usize;
        self.engines[..n.min(MAX_DMA_ENGINES)]
            .iter()
            .map(|e| e.errors.load(Ordering::Relaxed))
            .sum()
    }

    pub fn total_bytes(&self) -> u64 {
        let n = self.active_engines.load(Ordering::Relaxed) as usize;
        self.engines[..n.min(MAX_DMA_ENGINES)]
            .iter()
            .map(|e| e.bytes_tx.load(Ordering::Relaxed) + e.bytes_rx.load(Ordering::Relaxed))
            .sum()
    }
}

/// Global DMA statistics instance.
pub static DMA_STATS: DmaStats = DmaStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// GLOBAL HELPER FUNCTIONS
// ─────────────────────────────────────────────────────────────────────────────

/// Records a submission for engine `engine_id`.
#[inline]
pub fn dma_stat_submit(engine_id: usize) {
    if let Some(e) = DMA_STATS.engine(engine_id) {
        e.on_submit();
    }
}

/// Records a completion for engine `engine_id`.
#[inline]
pub fn dma_stat_complete(engine_id: usize, bytes: u64, is_write: bool, latency_cy: u64) {
    if let Some(e) = DMA_STATS.engine(engine_id) {
        e.on_complete(bytes, is_write, latency_cy);
    }
}

/// Records an error for engine `engine_id`.
#[inline]
pub fn dma_stat_error(engine_id: usize) {
    if let Some(e) = DMA_STATS.engine(engine_id) {
        e.on_error();
    }
}

/// Records a timeout for engine `engine_id`.
#[inline]
pub fn dma_stat_timeout(engine_id: usize) {
    if let Some(e) = DMA_STATS.engine(engine_id) {
        e.on_timeout();
    }
}

/// Adds transferred bytes for engine `engine_id`.
#[inline]
pub fn dma_bytes_transferred(engine_id: usize, bytes: u64, is_write: bool) {
    if let Some(e) = DMA_STATS.engine(engine_id) {
        if is_write {
            e.bytes_tx.fetch_add(bytes, Ordering::Relaxed);
        } else {
            e.bytes_rx.fetch_add(bytes, Ordering::Relaxed);
        }
    }
}

/// Dumps global stats (for diagnostics).
/// Returns (submitted, completed, errors, bytes_total, iommu_faults).
#[inline]
pub fn dump_dma_stats() -> (u64, u64, u64, u64, u64) {
    (
        DMA_STATS.total_submitted(),
        DMA_STATS.total_completed(),
        DMA_STATS.total_errors(),
        DMA_STATS.total_bytes(),
        DMA_STATS.iommu_faults.load(Ordering::Relaxed),
    )
}
