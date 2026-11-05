// kernel/src/memory/dma/ops/cyclic.rs
//
// Cyclic DMA — audio/streaming transfers in circular buffer.
//
// A cyclic DMA transfer runs indefinitely in a circular buffer
// split into `period_count` periods. At the end of each period, a
// `period_elapsed` flag is raised so that the audio driver is notified.
//
// Architecture:
//   - The buffer is split into periods of `period_bytes` bytes.
//   - The descriptor list is looped (last → first).
//   - An atomic counter `elapsed_periods` increments on each DMA IRQ.
//   - The driver reads `elapsed_periods` to know how many periods to process.
//   - `stop()` interrupts the ongoing cycle.
//
// LAYER 0 — no external dependencies.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::types::{DmaChannelId, DmaDirection, DmaError, DmaTransactionId};

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTS
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of concurrent cyclic channels.
pub const MAX_CYCLIC_CHANNELS: usize = 16;

/// Maximum number of periods per cyclic transfer.
pub const MAX_CYCLIC_PERIODS: usize = 32;

/// Minimum period size (64 bytes — cache line alignment).
pub const MIN_PERIOD_BYTES: usize = 64;

/// Maximum period size (1 MiB).
pub const MAX_PERIOD_BYTES: usize = 1024 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// CYCLIC TRANSFER CONFIGURATION
// ─────────────────────────────────────────────────────────────────────────────

/// Cyclic DMA transfer configuration.
#[derive(Copy, Clone)]
pub struct CyclicConfig {
    /// DMA channel to use.
    pub channel: DmaChannelId,
    /// Physical address of the circular buffer.
    pub buf_phys: PhysAddr,
    /// Total buffer size (must be a multiple of `period_bytes`).
    pub buf_bytes: usize,
    /// Period size in bytes.
    pub period_bytes: usize,
    /// Transfer direction.
    pub direction: DmaDirection,
    /// Peripheral physical address (FIFO register, etc.).
    pub dev_phys: PhysAddr,
}

/// Result of validating a cyclic configuration.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum CyclicConfigError {
    InvalidAlignment,
    TooManyPeriods,
    PeriodTooSmall,
    PeriodTooLarge,
    BufNotMultipleOfPeriod,
    ChannelUnsupported,
}

impl CyclicConfig {
    /// Validates configuration and returns the number of periods.
    pub fn validate(&self) -> Result<usize, CyclicConfigError> {
        if self.period_bytes < MIN_PERIOD_BYTES {
            return Err(CyclicConfigError::PeriodTooSmall);
        }
        if self.period_bytes > MAX_PERIOD_BYTES {
            return Err(CyclicConfigError::PeriodTooLarge);
        }
        if self.period_bytes & 63 != 0 {
            return Err(CyclicConfigError::InvalidAlignment);
        }
        if !self.buf_bytes.is_multiple_of(self.period_bytes) {
            return Err(CyclicConfigError::BufNotMultipleOfPeriod);
        }
        let periods = self.buf_bytes / self.period_bytes;
        if periods > MAX_CYCLIC_PERIODS {
            return Err(CyclicConfigError::TooManyPeriods);
        }
        Ok(periods)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CYCLIC TRANSFER STATE
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Eq, PartialEq)]
#[repr(u8)]
#[allow(dead_code)]
enum CyclicState {
    Idle = 0,
    Running = 1,
    Paused = 2,
    Error = 3,
}

/// Internal descriptor of a cyclic period.
#[allow(dead_code)]
#[derive(Copy, Clone)]
struct PeriodDesc {
    /// Physical start address of the period in the buffer.
    phys_start: PhysAddr,
    /// Bytes in this period.
    bytes: usize,
}

/// An active cyclic DMA transfer.
pub struct CyclicTransfer {
    /// Configuration of this transfer.
    config: CyclicConfig,
    /// Segmented periods.
    periods: [PeriodDesc; MAX_CYCLIC_PERIODS],
    period_count: usize,
    /// Current state.
    state: AtomicU8, // CyclicState
    /// Number of periods elapsed since startup (IRQ increments this counter).
    elapsed_periods: AtomicU64,
    /// Last period consumed by the audio driver.
    consumed_periods: AtomicU64,
    /// Current DMA transaction (reused for each period).
    current_txn: DmaTransactionId,
    /// Index of the period currently being transferred.
    current_period: AtomicU32,
    /// Statistics.
    pub underruns: AtomicU32,
    pub overruns: AtomicU32,
}

impl CyclicTransfer {
    const fn new() -> Self {
        const PD: PeriodDesc = PeriodDesc {
            phys_start: PhysAddr::new(0),
            bytes: 0,
        };
        CyclicTransfer {
            config: CyclicConfig {
                channel: DmaChannelId(u32::MAX),
                buf_phys: PhysAddr::new(0),
                buf_bytes: 0,
                period_bytes: 0,
                direction: DmaDirection::None,
                dev_phys: PhysAddr::new(0),
            },
            periods: [PD; MAX_CYCLIC_PERIODS],
            period_count: 0,
            state: AtomicU8::new(CyclicState::Idle as u8),
            elapsed_periods: AtomicU64::new(0),
            consumed_periods: AtomicU64::new(0),
            current_txn: DmaTransactionId::INVALID,
            current_period: AtomicU32::new(0),
            underruns: AtomicU32::new(0),
            overruns: AtomicU32::new(0),
        }
    }

    /// Prepares the transfer from a validated configuration.
    fn setup(&mut self, config: CyclicConfig, period_count: usize) {
        self.config = config;
        self.period_count = period_count;
        for i in 0..period_count {
            self.periods[i] = PeriodDesc {
                phys_start: PhysAddr::new(
                    config.buf_phys.as_u64() + (i * config.period_bytes) as u64,
                ),
                bytes: config.period_bytes,
            };
        }
        self.elapsed_periods.store(0, Ordering::Relaxed);
        self.consumed_periods.store(0, Ordering::Relaxed);
        self.current_period.store(0, Ordering::Relaxed);
    }

    /// Starts the transfer (first DMA command submission).
    pub fn start(&mut self) -> Result<DmaTransactionId, DmaError> {
        if self.state.load(Ordering::Acquire) == CyclicState::Running as u8 {
            return Err(DmaError::AlreadySubmitted);
        }
        let txn = DmaTransactionId::generate();
        self.current_txn = txn;
        self.state
            .store(CyclicState::Running as u8, Ordering::Release);
        Ok(txn)
    }

    /// Called by the IRQ handler when a period completes.
    ///
    /// Increments `elapsed_periods` and schedules the next period.
    pub fn on_period_complete(&self) {
        let elapsed = self.elapsed_periods.fetch_add(1, Ordering::AcqRel) + 1;
        let next_period = (elapsed as usize) % self.period_count;
        self.current_period
            .store(next_period as u32, Ordering::Relaxed);

        // Xrun detection.
        let consumed = self.consumed_periods.load(Ordering::Relaxed);
        let lag = elapsed.saturating_sub(consumed);
        if lag > self.period_count as u64 {
            self.overruns.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Returns how many periods are available for the driver.
    pub fn available_periods(&self) -> u64 {
        let elapsed = self.elapsed_periods.load(Ordering::Acquire);
        let consumed = self.consumed_periods.load(Ordering::Relaxed);
        elapsed.saturating_sub(consumed)
    }

    /// The driver marks `n` periods as consumed.
    pub fn consume_periods(&self, n: u64) {
        self.consumed_periods.fetch_add(n, Ordering::Relaxed);
    }

    /// Stops the cyclic transfer.
    pub fn stop(&self) {
        self.state.store(CyclicState::Idle as u8, Ordering::Release);
    }

    pub fn is_running(&self) -> bool {
        self.state.load(Ordering::Acquire) == CyclicState::Running as u8
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CYCLIC TRANSFERS TABLE
// ─────────────────────────────────────────────────────────────────────────────

#[allow(dead_code)]
struct CyclicTable {
    slots: [CyclicTransfer; MAX_CYCLIC_CHANNELS],
    count: usize,
}

impl CyclicTable {
    const fn new() -> Self {
        CyclicTable {
            slots: [const { CyclicTransfer::new() }; MAX_CYCLIC_CHANNELS],
            count: 0,
        }
    }

    fn alloc_slot(&mut self) -> Option<usize> {
        for (i, slot) in self.slots.iter().enumerate() {
            if !slot.is_running() && slot.period_count == 0 {
                return Some(i);
            }
        }
        None
    }
}

/// Table of cyclic DMA transfers.
pub struct CyclicManager {
    inner: Mutex<CyclicTable>,
}

unsafe impl Sync for CyclicManager {}
unsafe impl Send for CyclicManager {}

impl Default for CyclicManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CyclicManager {
    const fn new() -> Self {
        CyclicManager {
            inner: Mutex::new(CyclicTable::new()),
        }
    }

    /// Prepares and starts a cyclic transfer.
    ///
    /// Returns the allocated slot index (for `on_period_complete`/`stop` calls).
    pub fn start(&self, config: CyclicConfig) -> Result<usize, CyclicConfigError> {
        let period_count = config.validate()?;
        let mut table = self.inner.lock();
        let slot_idx = table
            .alloc_slot()
            .ok_or(CyclicConfigError::TooManyPeriods)?;
        table.slots[slot_idx].setup(config, period_count);
        table.slots[slot_idx]
            .start()
            .map_err(|_| CyclicConfigError::ChannelUnsupported)?;
        Ok(slot_idx)
    }

    /// Notifies completion of a period for slot `idx`.
    pub fn on_period_complete(&self, idx: usize) {
        let table = self.inner.lock();
        if idx < MAX_CYCLIC_CHANNELS {
            table.slots[idx].on_period_complete();
        }
    }

    /// Returns the number of periods available for reading (slot `idx`).
    pub fn available_periods(&self, idx: usize) -> u64 {
        let table = self.inner.lock();
        if idx < MAX_CYCLIC_CHANNELS {
            table.slots[idx].available_periods()
        } else {
            0
        }
    }

    /// Marks `n` periods as consumed by the audio driver.
    pub fn consume_periods(&self, idx: usize, n: u64) {
        let table = self.inner.lock();
        if idx < MAX_CYCLIC_CHANNELS {
            table.slots[idx].consume_periods(n);
        }
    }

    /// Stops the cyclic transfer of slot `idx`.
    pub fn stop(&self, idx: usize) {
        let table = self.inner.lock();
        if idx < MAX_CYCLIC_CHANNELS {
            table.slots[idx].stop();
        }
    }
}

/// Global cyclic DMA transfer manager.
pub static DMA_CYCLIC: CyclicManager = CyclicManager::new();
