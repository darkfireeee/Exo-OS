// kernel/src/memory/dma/ops/interleaved.rs
//
// Interleaved DMA — transfers with stride on source/destination.
//
// An interleaved transfer moves non-contiguous chunks:
//   - Source:  addresses src_base, src_base+src_stride, src_base+2*src_stride...
//   - Dest:    addresses dst_base, dst_base+dst_stride, dst_base+2*dst_stride...
//   - Each chunk is `chunk_bytes` bytes.
//
// Typical use cases:
//   - Multi-channel audio de-interleaving (L/R → separate buffers).
//   - RAID-like striping across memory zones.
//   - Matrix transposition in memory.
//   - Reorganizing scatter DMA into gather.
//
// LAYER 0 — no external dependencies.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

use crate::memory::core::layout::PHYS_MAP_BASE;
use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::types::{DmaChannelId, DmaError, DmaTransactionId};

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTS
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of chunks per interleaved transfer.
pub const MAX_INTERLEAVED_CHUNKS: usize = 64;

/// Minimum chunk size.
pub const MIN_CHUNK_BYTES: usize = 8;

/// Maximum number of concurrent interleaved transfers.
pub const MAX_INTERLEAVED_TRANSFERS: usize = 32;

// ─────────────────────────────────────────────────────────────────────────────
// CONFIGURATION
// ─────────────────────────────────────────────────────────────────────────────

/// Stride pattern for one endpoint (source or destination).
#[derive(Copy, Clone, Debug)]
pub struct InterleavedStride {
    /// Physical starting address.
    pub base: PhysAddr,
    /// Stride in bytes between two consecutive chunks.
    /// 0 = contiguous (same as ordinary scatter-gather).
    pub stride: usize,
}

/// Interleaved DMA transfer configuration.
#[derive(Copy, Clone)]
pub struct InterleavedConfig {
    /// DMA channel.
    pub channel: DmaChannelId,
    /// Source pattern.
    pub src: InterleavedStride,
    /// Destination pattern.
    pub dst: InterleavedStride,
    /// Size of each chunk in bytes.
    pub chunk_bytes: usize,
    /// Number of chunks.
    pub chunk_count: usize,
}

/// Validation error for an interleaved configuration.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum InterleavedError {
    ChunkTooSmall,
    TooManyChunks,
    InvalidAlignment,
    ZeroChunkCount,
    ChannelUnsupported,
}

impl InterleavedConfig {
    pub fn validate(&self) -> Result<(), InterleavedError> {
        if self.chunk_count == 0 {
            return Err(InterleavedError::ZeroChunkCount);
        }
        if self.chunk_bytes < MIN_CHUNK_BYTES {
            return Err(InterleavedError::ChunkTooSmall);
        }
        if self.chunk_bytes & 7 != 0 {
            return Err(InterleavedError::InvalidAlignment);
        }
        if self.chunk_count > MAX_INTERLEAVED_CHUNKS {
            return Err(InterleavedError::TooManyChunks);
        }
        Ok(())
    }

    /// Returns the physical address of chunk `i` on the source side.
    #[inline]
    pub fn src_chunk_phys(&self, i: usize) -> PhysAddr {
        PhysAddr::new(self.src.base.as_u64() + (i * self.src.stride) as u64)
    }

    /// Returns the physical address of chunk `i` on the destination side.
    #[inline]
    pub fn dst_chunk_phys(&self, i: usize) -> PhysAddr {
        PhysAddr::new(self.dst.base.as_u64() + (i * self.dst.stride) as u64)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SOFTWARE INTERLEAVED TRANSFER (FALLBACK)
// ─────────────────────────────────────────────────────────────────────────────

/// Executes an interleaved transfer in software mode (CPU memcpy).
///
/// Used when the DMA channel does not natively support interleaved mode.
/// Synchronous — returns when all chunks have been copied.
///
/// # Safety
/// `src.base` and `dst.base` must point to valid physical memory regions
/// accessible via the physmap (PHYS_MAP_BASE).
pub unsafe fn sw_interleaved_copy(config: &InterleavedConfig) -> Result<usize, InterleavedError> {
    config.validate()?;

    let phys_map_base = PHYS_MAP_BASE.as_u64();
    let mut total = 0usize;

    for i in 0..config.chunk_count {
        let src_virt = (phys_map_base + config.src_chunk_phys(i).as_u64()) as *const u8;
        let dst_virt = (phys_map_base + config.dst_chunk_phys(i).as_u64()) as *mut u8;

        // SAFETY: The addresses are in the physmap (validity guaranteed by the caller).
        core::ptr::copy_nonoverlapping(src_virt, dst_virt, config.chunk_bytes);
        total += config.chunk_bytes;
    }

    Ok(total)
}

// ─────────────────────────────────────────────────────────────────────────────
// INTERLEAVED TRANSFER VIA ASYNCHRONOUS DMA
// ─────────────────────────────────────────────────────────────────────────────

/// Internal state of an asynchronous interleaved transfer.
#[repr(u8)]
#[derive(Copy, Clone, Eq, PartialEq)]
enum IxferState {
    Free = 0,
    InProgress = 1,
    Done = 2,
    Error = 3,
}

/// An ongoing asynchronous interleaved transfer.
struct InterleavedXfer {
    config: InterleavedConfig,
    state: AtomicU8,
    txn_id: DmaTransactionId,
    /// Chunks already submitted to the DMA engine.
    chunks_submitted: AtomicU32,
    /// Chunks completed successfully.
    chunks_done: AtomicU32,
    /// Chunks in error.
    chunks_error: AtomicU32,
    /// Total bytes transferred.
    bytes_done: AtomicU64,
}

impl InterleavedXfer {
    const fn new() -> Self {
        InterleavedXfer {
            config: InterleavedConfig {
                channel: DmaChannelId(u32::MAX),
                src: InterleavedStride {
                    base: PhysAddr::new(0),
                    stride: 0,
                },
                dst: InterleavedStride {
                    base: PhysAddr::new(0),
                    stride: 0,
                },
                chunk_bytes: 0,
                chunk_count: 0,
            },
            state: AtomicU8::new(IxferState::Free as u8),
            txn_id: DmaTransactionId::INVALID,
            chunks_submitted: AtomicU32::new(0),
            chunks_done: AtomicU32::new(0),
            chunks_error: AtomicU32::new(0),
            bytes_done: AtomicU64::new(0),
        }
    }

    fn is_free(&self) -> bool {
        self.state.load(Ordering::Acquire) == IxferState::Free as u8
    }

    #[allow(dead_code)]
    fn is_done(&self) -> bool {
        let done = self.chunks_done.load(Ordering::Acquire) as usize;
        let total = self.config.chunk_count;
        done >= total && total > 0
    }

    /// Notifies chunk completion (called by IRQ handler).
    pub fn on_chunk_complete(&self, success: bool) {
        if success {
            let done = self.chunks_done.fetch_add(1, Ordering::AcqRel) + 1;
            self.bytes_done
                .fetch_add(self.config.chunk_bytes as u64, Ordering::Relaxed);
            if done >= self.config.chunk_count as u32 {
                self.state.store(IxferState::Done as u8, Ordering::Release);
            }
        } else {
            self.chunks_error.fetch_add(1, Ordering::Relaxed);
            self.state.store(IxferState::Error as u8, Ordering::Release);
        }
    }

    /// Transfer result (if completed).
    pub fn result(&self) -> Option<Result<usize, DmaError>> {
        match self.state.load(Ordering::Acquire) {
            s if s == IxferState::Done as u8 => {
                Some(Ok(self.bytes_done.load(Ordering::Relaxed) as usize))
            }
            s if s == IxferState::Error as u8 => Some(Err(DmaError::HardwareError)),
            _ => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// INTERLEAVED TRANSFERS MANAGER
// ─────────────────────────────────────────────────────────────────────────────

struct InterleavedTable {
    xfers: [InterleavedXfer; MAX_INTERLEAVED_TRANSFERS],
}

impl InterleavedTable {
    const fn new() -> Self {
        InterleavedTable {
            xfers: [const { InterleavedXfer::new() }; MAX_INTERLEAVED_TRANSFERS],
        }
    }

    fn alloc(&mut self) -> Option<usize> {
        for (i, x) in self.xfers.iter().enumerate() {
            if x.is_free() {
                return Some(i);
            }
        }
        None
    }
}

/// Global interleaved DMA transfers manager.
pub struct InterleavedManager {
    inner: Mutex<InterleavedTable>,
    pub submitted: AtomicU64,
    pub completed: AtomicU64,
    pub sw_fallbacks: AtomicU64,
}

unsafe impl Sync for InterleavedManager {}
unsafe impl Send for InterleavedManager {}

impl Default for InterleavedManager {
    fn default() -> Self {
        Self::new()
    }
}

impl InterleavedManager {
    pub const fn new() -> Self {
        InterleavedManager {
            inner: Mutex::new(InterleavedTable::new()),
            submitted: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            sw_fallbacks: AtomicU64::new(0),
        }
    }

    /// Submits an asynchronous interleaved transfer.
    ///
    /// Returns the slot index to track progress via `result`.
    ///
    /// If the channel does not support interleaved mode, returns
    /// `Err(InterleavedError::ChannelUnsupported)`. Then uses
    /// `sw_interleaved_copy` as a synchronous fallback.
    pub fn submit_async(&self, config: InterleavedConfig) -> Result<usize, InterleavedError> {
        config.validate()?;

        let mut table = self.inner.lock();
        let idx = table.alloc().ok_or(InterleavedError::TooManyChunks)?;

        let txn = DmaTransactionId::generate();
        let xfer = &mut table.xfers[idx];
        xfer.config = config;
        xfer.txn_id = txn;
        xfer.chunks_submitted.store(0, Ordering::Relaxed);
        xfer.chunks_done.store(0, Ordering::Relaxed);
        xfer.chunks_error.store(0, Ordering::Relaxed);
        xfer.bytes_done.store(0, Ordering::Relaxed);
        xfer.state
            .store(IxferState::InProgress as u8, Ordering::Release);

        self.submitted.fetch_add(1, Ordering::Relaxed);
        Ok(idx)
    }

    /// Notifies completion of chunk `chunk_idx` of transfer `xfer_idx`.
    pub fn on_chunk_complete(&self, xfer_idx: usize, success: bool) {
        let table = self.inner.lock();
        if xfer_idx < MAX_INTERLEAVED_TRANSFERS {
            table.xfers[xfer_idx].on_chunk_complete(success);
        }
    }

    /// Returns the result of a transfer (None if still in progress).
    pub fn result(&self, xfer_idx: usize) -> Option<Result<usize, DmaError>> {
        let table = self.inner.lock();
        if xfer_idx < MAX_INTERLEAVED_TRANSFERS {
            let res = table.xfers[xfer_idx].result();
            if res.is_some() {
                self.completed.fetch_add(1, Ordering::Relaxed);
            }
            res
        } else {
            Some(Err(DmaError::InvalidParams))
        }
    }

    /// Releases a completed transfer slot.
    pub fn release(&self, xfer_idx: usize) {
        let table = self.inner.lock();
        if xfer_idx < MAX_INTERLEAVED_TRANSFERS {
            table.xfers[xfer_idx]
                .state
                .store(IxferState::Free as u8, Ordering::Release);
        }
    }
}

/// Global interleaved DMA manager.
pub static DMA_INTERLEAVED: InterleavedManager = InterleavedManager::new();
