// kernel/src/memory/dma/ops/memset.rs
//
// DMA memset operation — initializes a physical region with a given value.
//
// Strategy:
//   1. Searches for a DMA channel supporting MEMSET (Intel I/OAT, DSA engines...).
//   2. If available: submits a MEMSET descriptor to the channel and returns.
//   3. Otherwise: software fallback via physmap (write_bytes).
//
// LAYER 0 — no external dependencies.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::memory::core::types::PhysAddr;
use crate::memory::dma::channels::manager::DMA_CHANNELS;
use crate::memory::dma::completion::handler::DMA_COMPLETION;
use crate::memory::dma::core::descriptor::DMA_DESCRIPTOR_TABLE;
use crate::memory::dma::core::mapping::IOVA_ALLOCATOR;
use crate::memory::dma::core::types::{
    DmaCapabilities, DmaDirection, DmaError, DmaMapFlags, DmaPriority,
};
use crate::memory::dma::iommu::domain::IDENTITY_DOMAIN_ID;

// ─────────────────────────────────────────────────────────────────────────────
// STATISTICS
// ─────────────────────────────────────────────────────────────────────────────

pub struct DmaMemsetStats {
    /// Calls to dma_memset (total).
    pub calls: AtomicU64,
    /// Operations processed by a hardware DMA channel.
    pub hw_ops: AtomicU64,
    /// Operations processed by the software path (fallback).
    pub sw_ops: AtomicU64,
    /// Zero-fill bytes via hardware DMA.
    pub hw_bytes: AtomicU64,
    /// Zero-fill bytes via software.
    pub sw_bytes: AtomicU64,
    /// Errors (invalidParams, OutOfMemory, mapping...).
    pub errors: AtomicU64,
}

impl Default for DmaMemsetStats {
    fn default() -> Self {
        Self::new()
    }
}

impl DmaMemsetStats {
    pub const fn new() -> Self {
        DmaMemsetStats {
            calls: AtomicU64::new(0),
            hw_ops: AtomicU64::new(0),
            sw_ops: AtomicU64::new(0),
            hw_bytes: AtomicU64::new(0),
            sw_bytes: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        }
    }
}

pub static DMA_MEMSET_STATS: DmaMemsetStats = DmaMemsetStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// HW THRESHOLD: do not engage DMA for very small regions
// (descriptor overhead > gain).
// ─────────────────────────────────────────────────────────────────────────────
const HW_MEMSET_THRESHOLD: usize = 4096; // 1 page

// ─────────────────────────────────────────────────────────────────────────────
// PUBLIC API
// ─────────────────────────────────────────────────────────────────────────────

/// Physically fills `size` bytes at `dst` with `value`.
///
/// Uses a hardware DMA channel if available and if `size >= HW_MEMSET_THRESHOLD`.
/// Falls back to software otherwise.
///
/// # Safety
/// `dst` must be a valid physical address, mapped in the kernel physmap.
pub unsafe fn dma_memset(dst: PhysAddr, value: u8, size: usize) -> Result<(), DmaError> {
    DMA_MEMSET_STATS.calls.fetch_add(1, Ordering::Relaxed);

    if size == 0 {
        return Ok(());
    }

    // Small transfer → direct SW without channel overhead.
    if size < HW_MEMSET_THRESHOLD {
        sw_memset(dst, value, size);
        DMA_MEMSET_STATS.sw_ops.fetch_add(1, Ordering::Relaxed);
        DMA_MEMSET_STATS
            .sw_bytes
            .fetch_add(size as u64, Ordering::Relaxed);
        return Ok(());
    }

    // Attempt a DMA MEMSET channel.
    let ch_opt = DMA_CHANNELS.alloc_channel(DmaCapabilities::MEMSET, DmaPriority::Normal);

    if let Some(ch_id) = ch_opt {
        // Channel available: submit a MEMSET descriptor.
        match submit_hw_memset(dst, value, size, ch_id.0) {
            Ok(_) => {
                // Asynchronous success — completion will be handled by DMA_COMPLETION.
                // Here we cannot wait (Layer 0, no scheduler) so we free the channel
                // after submission. A real engine would hold the channel until completion
                // via interrupt; this implementation simulates best-effort submission.
                DMA_CHANNELS.free_channel(ch_id);
                DMA_MEMSET_STATS.hw_ops.fetch_add(1, Ordering::Relaxed);
                DMA_MEMSET_STATS
                    .hw_bytes
                    .fetch_add(size as u64, Ordering::Relaxed);
                return Ok(());
            }
            Err(_) => {
                DMA_CHANNELS.free_channel(ch_id);
                DMA_MEMSET_STATS.errors.fetch_add(1, Ordering::Relaxed);
                // Fallthrough to SW.
            }
        }
    }

    // Software fallback.
    sw_memset(dst, value, size);
    DMA_MEMSET_STATS.sw_ops.fetch_add(1, Ordering::Relaxed);
    DMA_MEMSET_STATS
        .sw_bytes
        .fetch_add(size as u64, Ordering::Relaxed);
    Ok(())
}

/// DMA zero-fill (most frequent case — page initialization).
///
/// # Safety
/// Same preconditions as `dma_memset`.
#[inline]
pub unsafe fn dma_zero(dst: PhysAddr, size: usize) -> Result<(), DmaError> {
    dma_memset(dst, 0, size)
}

// ─────────────────────────────────────────────────────────────────────────────
// HW DESCRIPTOR SUBMISSION
// ─────────────────────────────────────────────────────────────────────────────

/// Submits a MEMSET descriptor to channel `ch_id`.
///
/// # Safety
/// `dst` must be physically valid.
unsafe fn submit_hw_memset(
    dst: PhysAddr,
    value: u8,
    size: usize,
    ch_id: u32,
) -> Result<(), DmaError> {
    // Allocate a transaction descriptor.
    let desc = DMA_DESCRIPTOR_TABLE
        .alloc_descriptor(ch_id, 0 /* requester = kernel */)
        .ok_or(DmaError::OutOfMemory)?;

    // Configure the MEMSET descriptor.
    // NOTE: setup_fill() encodes the fill value in the src_phys field
    //       (Exo-OS convention for I/OAT/DSA engines).
    desc.setup_fill(dst, value, size);

    // Map destination to IOVA.
    let dst_iova = IOVA_ALLOCATOR.map(
        dst,
        size,
        DmaDirection::ToDevice,
        DmaMapFlags::NONE,
        IDENTITY_DOMAIN_ID,
    )?;
    desc.dst_iova = dst_iova;

    let txn_id = desc.txn_id;

    // Submit to channel.
    DMA_CHANNELS
        .channel(crate::memory::dma::core::types::DmaChannelId(ch_id))
        .ok_or(DmaError::NoChannel)?
        .enqueue(txn_id)?;

    // Register for completion (no wakeup: null tid).
    DMA_COMPLETION.register(txn_id, ch_id, 0);

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// SOFTWARE FALLBACK
// ─────────────────────────────────────────────────────────────────────────────

/// Software fill via physmap (fallback or small regions).
///
/// # Safety
/// `dst` must be mapped in the physmap.
#[inline]
pub unsafe fn sw_memset(dst: PhysAddr, value: u8, size: usize) {
    use crate::memory::core::address::phys_to_virt;
    let ptr = phys_to_virt(dst).as_u64() as *mut u8;
    // SAFETY: physmap covers all RAM.
    core::ptr::write_bytes(ptr, value, size);
}

/// Clears (zeroes) a physical region via the physmap.
///
/// # Safety
/// Same preconditions as `sw_memset`.
#[inline]
pub unsafe fn sw_zero(dst: PhysAddr, size: usize) {
    sw_memset(dst, 0, size);
}
