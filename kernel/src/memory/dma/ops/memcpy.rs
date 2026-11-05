// kernel/src/memory/dma/ops/memcpy.rs
//
// DMA memcpy operation — physical src → dst copy via a DMA channel.
// LAYER 0 — no external dependencies.

use crate::memory::core::types::PhysAddr;
use crate::memory::dma::channels::manager::DMA_CHANNELS;
use crate::memory::dma::completion::handler::DMA_COMPLETION;
use crate::memory::dma::core::descriptor::DMA_DESCRIPTOR_TABLE;
use crate::memory::dma::core::mapping::IOVA_ALLOCATOR;
use crate::memory::dma::core::types::{
    DmaCapabilities, DmaDirection, DmaError, DmaMapFlags, DmaPriority, DmaTransactionId,
};
use crate::memory::dma::iommu::domain::IDENTITY_DOMAIN_ID;

// ─────────────────────────────────────────────────────────────────────────────
// API: SUBMIT A DMA MEMCPY OPERATION
// ─────────────────────────────────────────────────────────────────────────────

/// Result of submitting a DMA operation.
pub struct DmaOpHandle {
    pub txn_id: DmaTransactionId,
    pub channel_id: u32,
}

/// Submits an asynchronous DMA copy operation (physmem src → physmem dst).
///
/// Returns a handle used to wait for completion.
///
/// # Safety
/// `src` and `dst` must be valid physical non-overlapping regions.
/// `size` must be > 0.
pub unsafe fn dma_memcpy_async(
    src: PhysAddr,
    dst: PhysAddr,
    size: usize,
    requester_tid: u64,
) -> Result<DmaOpHandle, DmaError> {
    if size == 0 {
        return Err(DmaError::InvalidParams);
    }

    // Allocate a channel supporting MEMCPY.
    let ch_id = DMA_CHANNELS
        .alloc_channel(DmaCapabilities::MEMCPY, DmaPriority::Normal)
        .ok_or(DmaError::NoChannel)?;

    // Allocate a transaction descriptor.
    let desc = DMA_DESCRIPTOR_TABLE
        .alloc_descriptor(ch_id.0, requester_tid)
        .ok_or(DmaError::OutOfMemory)?;

    // Configure transaction.
    desc.setup_simple(src, dst, size, DmaDirection::Bidirection);

    // Map src and dst to IOVA (passthrough or IOMMU depending on config).
    let src_iova = IOVA_ALLOCATOR.map(
        src,
        size,
        DmaDirection::FromDevice,
        DmaMapFlags::NONE,
        IDENTITY_DOMAIN_ID,
    )?;
    let dst_iova = IOVA_ALLOCATOR.map(
        dst,
        size,
        DmaDirection::ToDevice,
        DmaMapFlags::NONE,
        IDENTITY_DOMAIN_ID,
    )?;
    desc.src_iova = src_iova;
    desc.dst_iova = dst_iova;

    let txn_id = desc.txn_id;

    // Enqueue into channel.
    DMA_CHANNELS
        .channel(ch_id)
        .ok_or(DmaError::NoChannel)?
        .enqueue(txn_id)?;

    // Register in completion manager.
    DMA_COMPLETION.register(txn_id, ch_id.0, requester_tid);

    Ok(DmaOpHandle {
        txn_id,
        channel_id: ch_id.0,
    })
}

/// Synchronous (blocking) version — waits for completion or timeout.
///
/// Used by the kernel for DMA transfers initiated before the scheduler
/// is operational.
///
/// # Safety
/// Same preconditions as `dma_memcpy_async`.
pub unsafe fn dma_memcpy_sync(
    src: PhysAddr,
    dst: PhysAddr,
    size: usize,
) -> Result<usize, DmaError> {
    let handle = dma_memcpy_async(src, dst, size, 0 /* tid=0 = kernel */)?;
    // Poll for completion (spin-wait, used only before scheduler).
    let timeout = 10_000_000u64;
    let mut spins = 0u64;
    loop {
        if let Some(result) = DMA_COMPLETION.poll(handle.txn_id) {
            DMA_CHANNELS.free_channel(crate::memory::dma::core::types::DmaChannelId(
                handle.channel_id,
            ));
            return result;
        }
        spins += 1;
        if spins > timeout {
            DMA_CHANNELS.free_channel(crate::memory::dma::core::types::DmaChannelId(
                handle.channel_id,
            ));
            return Err(DmaError::Timeout);
        }
        core::hint::spin_loop();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SOFTWARE FALLBACK (no DMA channel available)
// ─────────────────────────────────────────────────────────────────────────────

/// Software copy via physmap (fallback if DMA is unavailable).
///
/// # Safety
/// `src` and `dst` must be physical addresses mapped in the physmap.
pub unsafe fn sw_memcpy(src: PhysAddr, dst: PhysAddr, size: usize) {
    use crate::memory::core::address::phys_to_virt;
    let src_virt = phys_to_virt(src).as_u64() as *const u8;
    let dst_virt = phys_to_virt(dst).as_u64() as *mut u8;
    // SAFETY: Regions are valid (physmap covers all RAM).
    core::ptr::copy_nonoverlapping(src_virt, dst_virt, size);
}
