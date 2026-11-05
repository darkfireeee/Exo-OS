// kernel/src/memory/dma/core/error.rs
//
// Enriched DMA error context, static descriptions, and global counters.
// Complements `DmaError` variants defined in `types.rs`.
// LAYER 0 — zero external dependencies.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::memory::dma::core::types::{DmaChannelId, DmaError, DmaTransactionId};

// ─────────────────────────────────────────────────────────────────────────────
// ENRICHED ERROR CONTEXT
// ─────────────────────────────────────────────────────────────────────────────

/// Full context of a DMA error associating the error with its origin.
///
/// Used by interrupt handlers and the completion manager to
/// propagate all diagnostic information in a single value.
#[derive(Copy, Clone, Debug)]
pub struct DmaErrorContext {
    /// Cause of error.
    pub error: DmaError,
    /// Affected DMA channel.
    pub channel: DmaChannelId,
    /// Affected transaction (`DmaTransactionId::INVALID` if not applicable).
    pub transaction: DmaTransactionId,
    /// TSC timestamp at error time (0 if not available).
    pub tsc: u64,
    /// Faulting physical address (0 if not applicable — e.g. IOMMU fault).
    pub fault_addr: u64,
}

impl DmaErrorContext {
    /// Constructs a context with current TSC timestamp.
    #[inline]
    pub fn new(error: DmaError, channel: DmaChannelId, txn: DmaTransactionId) -> Self {
        DmaErrorContext {
            error,
            channel,
            transaction: txn,
            tsc: read_tsc(),
            fault_addr: 0,
        }
    }

    /// Constructs a context without associated transaction.
    #[inline]
    pub fn channel_error(error: DmaError, channel: DmaChannelId) -> Self {
        Self::new(error, channel, DmaTransactionId::INVALID)
    }

    /// Associates a faulting physical address (IOMMU fault).
    #[inline]
    pub fn with_fault_addr(mut self, addr: u64) -> Self {
        self.fault_addr = addr;
        self
    }

    /// Returns a static text description of the error.
    #[inline]
    pub fn description(self) -> &'static str {
        self.error.description()
    }

    /// `true` if error renders the channel unusable and requires a reset.
    #[inline]
    pub fn is_fatal(self) -> bool {
        matches!(
            self.error,
            DmaError::HardwareError | DmaError::IommuFault | DmaError::NotInitialized
        )
    }

    /// `true` if error is transient (next request may succeed).
    #[inline]
    pub fn is_transient(self) -> bool {
        matches!(
            self.error,
            DmaError::Timeout | DmaError::OutOfMemory | DmaError::Cancelled
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// STATIC DESCRIPTIONS
// ─────────────────────────────────────────────────────────────────────────────

impl DmaError {
    /// Returns a static string describing the error.
    pub const fn description(self) -> &'static str {
        match self {
            DmaError::NoChannel => "no DMA channel available",
            DmaError::OutOfMemory => "DMA memory exhausted",
            DmaError::InvalidParams => "invalid DMA parameters (size/alignment)",
            DmaError::Timeout => "DMA transfer timeout",
            DmaError::HardwareError => "DMA hardware error (bus/parity)",
            DmaError::IommuFault => "IOMMU page fault during DMA",
            DmaError::NotInitialized => "DMA channel not initialized",
            DmaError::AlreadySubmitted => "transaction already submitted to channel",
            DmaError::Cancelled => "DMA transaction cancelled by caller",
            DmaError::MisalignedBuffer => "DMA buffer not aligned to channel granularity",
            DmaError::WrongZone => "physical address outside required DMA zone",
            DmaError::NotSupported => "operation not supported by this channel",
        }
    }

    /// `true` if error indicates an unrecoverable hardware problem.
    #[inline]
    pub const fn is_hardware(self) -> bool {
        matches!(self, DmaError::HardwareError | DmaError::IommuFault)
    }

    /// `true` if error can be resolved by retrying.
    #[inline]
    pub const fn is_retriable(self) -> bool {
        matches!(
            self,
            DmaError::Timeout | DmaError::OutOfMemory | DmaError::NoChannel
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GLOBAL ERROR COUNTERS
// ─────────────────────────────────────────────────────────────────────────────

/// Global DMA error counters — one counter per variant + total.
///
/// Allows kernel monitoring to detect trends without access to logs.
pub struct DmaErrorCounters {
    pub no_channel: AtomicU64,
    pub out_of_memory: AtomicU64,
    pub invalid_params: AtomicU64,
    pub timeout: AtomicU64,
    pub hardware_error: AtomicU64,
    pub iommu_fault: AtomicU64,
    pub not_initialized: AtomicU64,
    pub already_submitted: AtomicU64,
    pub cancelled: AtomicU64,
    pub misaligned_buffer: AtomicU64,
    pub wrong_zone: AtomicU64,
    pub not_supported: AtomicU64,
    /// Total across all causes.
    pub total: AtomicU64,
}

impl Default for DmaErrorCounters {
    fn default() -> Self {
        Self::new()
    }
}

impl DmaErrorCounters {
    pub const fn new() -> Self {
        DmaErrorCounters {
            no_channel: AtomicU64::new(0),
            out_of_memory: AtomicU64::new(0),
            invalid_params: AtomicU64::new(0),
            timeout: AtomicU64::new(0),
            hardware_error: AtomicU64::new(0),
            iommu_fault: AtomicU64::new(0),
            not_initialized: AtomicU64::new(0),
            already_submitted: AtomicU64::new(0),
            cancelled: AtomicU64::new(0),
            misaligned_buffer: AtomicU64::new(0),
            wrong_zone: AtomicU64::new(0),
            not_supported: AtomicU64::new(0),
            total: AtomicU64::new(0),
        }
    }

    /// Increments the counter corresponding to the error.
    pub fn record(&self, err: DmaError) {
        self.total.fetch_add(1, Ordering::Relaxed);
        let counter = match err {
            DmaError::NoChannel => &self.no_channel,
            DmaError::OutOfMemory => &self.out_of_memory,
            DmaError::InvalidParams => &self.invalid_params,
            DmaError::Timeout => &self.timeout,
            DmaError::HardwareError => &self.hardware_error,
            DmaError::IommuFault => &self.iommu_fault,
            DmaError::NotInitialized => &self.not_initialized,
            DmaError::AlreadySubmitted => &self.already_submitted,
            DmaError::Cancelled => &self.cancelled,
            DmaError::MisalignedBuffer => &self.misaligned_buffer,
            DmaError::WrongZone => &self.wrong_zone,
            DmaError::NotSupported => &self.not_supported,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns total recorded errors.
    pub fn get_total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    /// Returns number of hardware errors (fatal).
    pub fn hardware_total(&self) -> u64 {
        self.hardware_error.load(Ordering::Relaxed) + self.iommu_fault.load(Ordering::Relaxed)
    }

    /// Returns number of timeouts.
    pub fn timeout_total(&self) -> u64 {
        self.timeout.load(Ordering::Relaxed)
    }

    /// Counter snapshot for monitoring.
    pub fn snapshot(&self) -> DmaErrorSnapshot {
        DmaErrorSnapshot {
            no_channel: self.no_channel.load(Ordering::Relaxed),
            out_of_memory: self.out_of_memory.load(Ordering::Relaxed),
            invalid_params: self.invalid_params.load(Ordering::Relaxed),
            timeout: self.timeout.load(Ordering::Relaxed),
            hardware_error: self.hardware_error.load(Ordering::Relaxed),
            iommu_fault: self.iommu_fault.load(Ordering::Relaxed),
            not_initialized: self.not_initialized.load(Ordering::Relaxed),
            already_submitted: self.already_submitted.load(Ordering::Relaxed),
            cancelled: self.cancelled.load(Ordering::Relaxed),
            misaligned_buffer: self.misaligned_buffer.load(Ordering::Relaxed),
            wrong_zone: self.wrong_zone.load(Ordering::Relaxed),
            not_supported: self.not_supported.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot of error counters (non-atomic values for consistent read).
#[derive(Copy, Clone, Debug, Default)]
pub struct DmaErrorSnapshot {
    pub no_channel: u64,
    pub out_of_memory: u64,
    pub invalid_params: u64,
    pub timeout: u64,
    pub hardware_error: u64,
    pub iommu_fault: u64,
    pub not_initialized: u64,
    pub already_submitted: u64,
    pub cancelled: u64,
    pub misaligned_buffer: u64,
    pub wrong_zone: u64,
    pub not_supported: u64,
    pub total: u64,
}

/// Global DMA error counters.
pub static DMA_ERROR_COUNTERS: DmaErrorCounters = DmaErrorCounters::new();

/// Records a DMA error in global counters.
#[inline]
pub fn record_error(err: DmaError) {
    DMA_ERROR_COUNTERS.record(err);
}

/// Records a full error context.
#[inline]
pub fn record_error_ctx(ctx: &DmaErrorContext) {
    DMA_ERROR_COUNTERS.record(ctx.error);
}

// ─────────────────────────────────────────────────────────────────────────────
// UTILITIES
// ─────────────────────────────────────────────────────────────────────────────

/// Reads current TSC via RDTSC (x86_64 only).
/// Returns 0 on other architectures.
#[inline(always)]
fn read_tsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: RDTSC available on x86_64; non-serializing sufficient for diagnostics.
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
