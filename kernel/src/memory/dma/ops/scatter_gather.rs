// kernel/src/memory/dma/ops/scatter_gather.rs
//
// Opération DMA Scatter-Gather — transfert depuis/vers plusieurs fragments.
// COUCHE 0 — aucune dépendance externe.

use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::types::{
    DmaDirection, DmaMapFlags, DmaCapabilities, DmaPriority, DmaError,
    DmaTransactionId, IovaAddr,
};
use crate::memory::dma::core::descriptor::{SgEntry, DmaDescriptor, DMA_DESCRIPTOR_TABLE, MAX_SG_ENTRIES};
use crate::memory::dma::core::mapping::IOVA_ALLOCATOR;
use crate::memory::dma::channels::manager::DMA_CHANNELS;
use crate::memory::dma::iommu::domain::IDENTITY_DOMAIN_ID;
use crate::memory::dma::completion::handler::DMA_COMPLETION;
use crate::memory::dma::ops::memcpy::DmaOpHandle;

// ─────────────────────────────────────────────────────────────────────────────
// API SCATTER-GATHER
// ─────────────────────────────────────────────────────────────────────────────

/// Soumet un transfert scatter-gather asynchrone.
///
/// `srcs` : fragments source (physiques).
/// `dsts` : fragments destination (physiques).
/// Les tailles doivent correspondre entry-à-entry.
///
/// # Safety
/// Tous les fragments doivent pointer de la mémoire physique valide.
pub unsafe fn dma_sg_async(
    srcs:          &[SgEntry],
    dsts:          &[SgEntry],
    requester_tid: u64,
) -> Result<DmaOpHandle, DmaError> {
    if srcs.is_empty() || dsts.is_empty() { return Err(DmaError::InvalidParams); }
    if srcs.len() > MAX_SG_ENTRIES || dsts.len() > MAX_SG_ENTRIES {
        return Err(DmaError::InvalidParams);
    }

    let ch_id = DMA_CHANNELS.alloc_channel(
        DmaCapabilities::SCATTER_GATHER, DmaPriority::Normal
    ).ok_or(DmaError::NoChannel)?;

    let desc = DMA_DESCRIPTOR_TABLE.alloc_descriptor(ch_id.0, requester_tid)
        .ok_or(DmaError::OutOfMemory)?;

    // Copie les SG entries.
    let src_count = srcs.len().min(MAX_SG_ENTRIES);
    let dst_count = dsts.len().min(MAX_SG_ENTRIES);
    desc.src_sg[..src_count].copy_from_slice(&srcs[..src_count]);
    desc.dst_sg[..dst_count].copy_from_slice(&dsts[..dst_count]);
    desc.src_sg_count = src_count as u16;
    desc.dst_sg_count = dst_count as u16;
    desc.direction = DmaDirection::Bidirection;

    // Calcul de la taille totale.
    let total: usize = srcs.iter().map(|e| e.len as usize).sum();
    desc.transfer_size = total;

    // Mapping IOVA pour chaque fragment src.
    let mut src_iovas = [IovaAddr::zero(); MAX_SG_ENTRIES];
    let n_mapped = IOVA_ALLOCATOR.map_sg(
        &desc.src_sg[..src_count],
        DmaDirection::FromDevice,
        DmaMapFlags::NONE,
        IDENTITY_DOMAIN_ID,
        &mut src_iovas[..src_count],
    )?;

    // Mapping IOVA pour chaque fragment dst.
    let mut dst_iovas = [IovaAddr::zero(); MAX_SG_ENTRIES];
    IOVA_ALLOCATOR.map_sg(
        &desc.dst_sg[..dst_count],
        DmaDirection::ToDevice,
        DmaMapFlags::NONE,
        IDENTITY_DOMAIN_ID,
        &mut dst_iovas[..dst_count],
    )?;

    let txn_id = desc.txn_id;
    DMA_CHANNELS.channel(ch_id).ok_or(DmaError::NoChannel)?.enqueue(txn_id)?;
    DMA_COMPLETION.register(txn_id, ch_id.0, requester_tid);

    Ok(DmaOpHandle { txn_id, channel_id: ch_id.0 })
}

/// Fallback software pour scatter-gather (concatène les fragments via physmap).
///
/// # Safety
/// Tous les fragments srcss et dsts doivent être mappés dans le physmap.
pub unsafe fn sw_sg_copy(srcs: &[SgEntry], dsts: &[SgEntry]) {
    use crate::memory::core::address::phys_to_virt;

    let mut dst_iter = dsts.iter();
    let mut dst_entry = match dst_iter.next() { Some(e) => *e, None => return };
    let mut dst_off = 0usize;

    for src in srcs {
        if src.is_empty() { break; }
        let mut src_off = 0usize;
        let mut src_rem = src.len as usize;

        while src_rem > 0 {
            if dst_off >= dst_entry.len as usize {
                dst_entry = match dst_iter.next() { Some(e) => *e, None => return };
                dst_off = 0;
            }
            let dst_rem = dst_entry.len as usize - dst_off;
            let copy = src_rem.min(dst_rem);

            let src_ptr = phys_to_virt(src.phys.add(src_off as u64)).as_u64() as *const u8;
            let dst_ptr = phys_to_virt(dst_entry.phys.add(dst_off as u64)).as_u64() as *mut u8;
            // SAFETY: Physmap valide, tailles contrôlées.
            core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, copy);

            src_off += copy;
            src_rem -= copy;
            dst_off += copy;
        }
    }
}
