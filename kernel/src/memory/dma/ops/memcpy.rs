// kernel/src/memory/dma/ops/memcpy.rs
//
// Opération DMA memcpy — copie physique src → dst via un canal DMA.
// COUCHE 0 — aucune dépendance externe.

use core::sync::atomic::Ordering;
use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::types::{
    DmaDirection, DmaMapFlags, DmaCapabilities, DmaPriority, DmaError,
    DmaTransactionId,
};
use crate::memory::dma::core::descriptor::{DMA_DESCRIPTOR_TABLE, DmaDescriptor};
use crate::memory::dma::core::mapping::IOVA_ALLOCATOR;
use crate::memory::dma::channels::manager::DMA_CHANNELS;
use crate::memory::dma::iommu::domain::IDENTITY_DOMAIN_ID;
use crate::memory::dma::completion::handler::DMA_COMPLETION;

// ─────────────────────────────────────────────────────────────────────────────
// API : SOUMETTRE UNE OPÉRATION MEMCPY DMA
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat de la soumission d'une opération DMA.
pub struct DmaOpHandle {
    pub txn_id:    DmaTransactionId,
    pub channel_id: u32,
}

/// Soumet une opération de copie DMA asynchrone (physmem src → physmem dst).
///
/// Retourne un handle utilisé pour attendre la complétion.
///
/// # Safety
/// `src` et `dst` doivent être des régions physiques valides et non-overlapping.
/// `size` doit être > 0.
pub unsafe fn dma_memcpy_async(
    src:           PhysAddr,
    dst:           PhysAddr,
    size:          usize,
    requester_tid: u64,
) -> Result<DmaOpHandle, DmaError> {
    if size == 0 { return Err(DmaError::InvalidParams); }

    // Alloue un canal supportant MEMCPY.
    let ch_id = DMA_CHANNELS.alloc_channel(DmaCapabilities::MEMCPY, DmaPriority::Normal)
        .ok_or(DmaError::NoChannel)?;

    // Alloue un descripteur de transaction.
    let desc = DMA_DESCRIPTOR_TABLE.alloc_descriptor(ch_id.0, requester_tid)
        .ok_or(DmaError::OutOfMemory)?;

    // Configure la transaction.
    desc.setup_simple(src, dst, size, DmaDirection::Bidirection);

    // Mappe src et dst en IOVA (passthrough ou IOMMU selon config).
    let src_iova = IOVA_ALLOCATOR.map(src, size, DmaDirection::FromDevice, DmaMapFlags::NONE, IDENTITY_DOMAIN_ID)?;
    let dst_iova = IOVA_ALLOCATOR.map(dst, size, DmaDirection::ToDevice,   DmaMapFlags::NONE, IDENTITY_DOMAIN_ID)?;
    desc.src_iova = src_iova;
    desc.dst_iova = dst_iova;

    let txn_id = desc.txn_id;

    // Enfile dans le canal.
    DMA_CHANNELS.channel(ch_id)
        .ok_or(DmaError::NoChannel)?
        .enqueue(txn_id)?;

    // Enregistre dans le gestionnaire de complétion.
    DMA_COMPLETION.register(txn_id, ch_id.0, requester_tid);

    Ok(DmaOpHandle { txn_id, channel_id: ch_id.0 })
}

/// Version synchrone (bloquante) — attend la complétion ou timeout.
///
/// Utilisée par le kernel pour les transferts DMA initialisés avant que le
/// scheduler ne soit opérationnel.
///
/// # Safety
/// Mêmes préconditions que `dma_memcpy_async`.
pub unsafe fn dma_memcpy_sync(
    src:  PhysAddr,
    dst:  PhysAddr,
    size: usize,
) -> Result<usize, DmaError> {
    let handle = dma_memcpy_async(src, dst, size, 0 /* tid=0 = kernel */)?;
    // Poll sur la complétion (spin-wait, utilisé seulement avant scheduler).
    let timeout = 10_000_000u64;
    let mut spins = 0u64;
    loop {
        if let Some(result) = DMA_COMPLETION.poll(handle.txn_id) {
            DMA_CHANNELS.free_channel(crate::memory::dma::core::types::DmaChannelId(handle.channel_id));
            return result;
        }
        spins += 1;
        if spins > timeout {
            DMA_CHANNELS.free_channel(crate::memory::dma::core::types::DmaChannelId(handle.channel_id));
            return Err(DmaError::Timeout);
        }
        core::hint::spin_loop();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FALLBACK SOFTWARE (pas de canal DMA disponible)
// ─────────────────────────────────────────────────────────────────────────────

/// Copie logicielle via le physmap (fallback si DMA indisponible).
///
/// # Safety
/// `src` et `dst` doivent être des adresses physiques mappées dans le physmap.
pub unsafe fn sw_memcpy(src: PhysAddr, dst: PhysAddr, size: usize) {
    use crate::memory::core::address::phys_to_virt;
    let src_virt = phys_to_virt(src).as_u64() as *const u8;
    let dst_virt = phys_to_virt(dst).as_u64() as *mut u8;
    // SAFETY: Les régions sont valides (physmap couvre toute la RAM).
    core::ptr::copy_nonoverlapping(src_virt, dst_virt, size);
}
