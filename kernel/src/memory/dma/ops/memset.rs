// kernel/src/memory/dma/ops/memset.rs
//
// Opération DMA memset — initialise une région physique avec une valeur donnée.
//
// Stratégie :
//   1. Cherche un canal DMA supportant MEMSET (moteurs Intel I/OAT, DSA…).
//   2. Si disponible : soumet un descripteur MEMSET au canal et retourne.
//   3. Sinon : fallback software via le physmap (write_bytes).
//
// COUCHE 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::types::{
    DmaDirection, DmaMapFlags, DmaCapabilities, DmaPriority, DmaError,
};
use crate::memory::dma::core::descriptor::DMA_DESCRIPTOR_TABLE;
use crate::memory::dma::core::mapping::IOVA_ALLOCATOR;
use crate::memory::dma::channels::manager::DMA_CHANNELS;
use crate::memory::dma::iommu::domain::IDENTITY_DOMAIN_ID;
use crate::memory::dma::completion::handler::DMA_COMPLETION;

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES
// ─────────────────────────────────────────────────────────────────────────────

pub struct DmaMemsetStats {
    /// Appels à dma_memset (total).
    pub calls:        AtomicU64,
    /// Opérations traitées par un canal DMA hardware.
    pub hw_ops:       AtomicU64,
    /// Opérations traitées par le chemin software (fallback).
    pub sw_ops:       AtomicU64,
    /// Octets zéro-fill via DMA hardware.
    pub hw_bytes:     AtomicU64,
    /// Octets zéro-fill via software.
    pub sw_bytes:     AtomicU64,
    /// Erreurs (invalidParams, OutOfMemory, mapping…).
    pub errors:       AtomicU64,
}

impl DmaMemsetStats {
    pub const fn new() -> Self {
        DmaMemsetStats {
            calls:    AtomicU64::new(0),
            hw_ops:   AtomicU64::new(0),
            sw_ops:   AtomicU64::new(0),
            hw_bytes: AtomicU64::new(0),
            sw_bytes: AtomicU64::new(0),
            errors:   AtomicU64::new(0),
        }
    }
}

pub static DMA_MEMSET_STATS: DmaMemsetStats = DmaMemsetStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// SEUIL HW : ne pas engager le DMA pour de très petites régions
// (overhead du descripteur > gain).
// ─────────────────────────────────────────────────────────────────────────────
const HW_MEMSET_THRESHOLD: usize = 4096; // 1 page

// ─────────────────────────────────────────────────────────────────────────────
// API PUBLIQUE
// ─────────────────────────────────────────────────────────────────────────────

/// Remplit physiquement `size` octets à `dst` avec `value`.
///
/// Utilise un canal DMA hardware si disponible et si `size >= HW_MEMSET_THRESHOLD`.
/// Retombe sur software sinon.
///
/// # Safety
/// `dst` doit être une adresse physique valide, mappée dans le physmap kernel.
pub unsafe fn dma_memset(dst: PhysAddr, value: u8, size: usize) -> Result<(), DmaError> {
    DMA_MEMSET_STATS.calls.fetch_add(1, Ordering::Relaxed);

    if size == 0 { return Ok(()); }

    // Petit transfert → SW direct sans overhead de canal.
    if size < HW_MEMSET_THRESHOLD {
        sw_memset(dst, value, size);
        DMA_MEMSET_STATS.sw_ops  .fetch_add(1, Ordering::Relaxed);
        DMA_MEMSET_STATS.sw_bytes.fetch_add(size as u64, Ordering::Relaxed);
        return Ok(());
    }

    // Tenter un canal DMA MEMSET.
    let ch_opt = DMA_CHANNELS.alloc_channel(DmaCapabilities::MEMSET, DmaPriority::Normal);

    if let Some(ch_id) = ch_opt {
        // Canal disponible : soumettre un descripteur MEMSET.
        match submit_hw_memset(dst, value, size, ch_id.0) {
            Ok(_) => {
                // Succès asynchrone — la complétion sera gérée par DMA_COMPLETION.
                // Ici on ne peut pas attendre (Couche 0, pas de scheduler) donc on
                // libère le canal après soumission. Un moteur réel garderait le canal
                // jusqu'à la complétion via interrupt ; cette implémentation simule
                // une soumission best-effort.
                DMA_CHANNELS.free_channel(ch_id);
                DMA_MEMSET_STATS.hw_ops  .fetch_add(1, Ordering::Relaxed);
                DMA_MEMSET_STATS.hw_bytes.fetch_add(size as u64, Ordering::Relaxed);
                return Ok(());
            }
            Err(_) => {
                DMA_CHANNELS.free_channel(ch_id);
                DMA_MEMSET_STATS.errors.fetch_add(1, Ordering::Relaxed);
                // Fallthrough vers SW.
            }
        }
    }

    // Fallback software.
    sw_memset(dst, value, size);
    DMA_MEMSET_STATS.sw_ops  .fetch_add(1, Ordering::Relaxed);
    DMA_MEMSET_STATS.sw_bytes.fetch_add(size as u64, Ordering::Relaxed);
    Ok(())
}

/// Zéro-fill DMA (cas le plus fréquent — initialisations de pages).
///
/// # Safety
/// Même préconditions que `dma_memset`.
#[inline]
pub unsafe fn dma_zero(dst: PhysAddr, size: usize) -> Result<(), DmaError> {
    dma_memset(dst, 0, size)
}

// ─────────────────────────────────────────────────────────────────────────────
// SOUMISSION DESCRIPTEUR HW
// ─────────────────────────────────────────────────────────────────────────────

/// Soumet un descripteur MEMSET au canal `ch_id`.
///
/// # Safety
/// `dst` doit être physiquement valide.
unsafe fn submit_hw_memset(
    dst:   PhysAddr,
    value: u8,
    size:  usize,
    ch_id: u32,
) -> Result<(), DmaError> {
    // Allouer un descripteur de transaction.
    let desc = DMA_DESCRIPTOR_TABLE
        .alloc_descriptor(ch_id, 0 /* requester = kernel */)
        .ok_or(DmaError::OutOfMemory)?;

    // Configurer le descripteur MEMSET.
    // NOTE : setup_fill() encode la valeur de remplissage dans le champ src_phys
    //        (convention Exo-OS pour les moteurs I/OAT/DSA).
    desc.setup_fill(dst, value, size);

    // Mapper la destination en IOVA.
    let dst_iova = IOVA_ALLOCATOR.map(
        dst, size,
        DmaDirection::ToDevice,
        DmaMapFlags::NONE,
        IDENTITY_DOMAIN_ID,
    )?;
    desc.dst_iova = dst_iova;

    let txn_id = desc.txn_id;

    // Soumettre au canal.
    DMA_CHANNELS
        .channel(crate::memory::dma::core::types::DmaChannelId(ch_id))
        .ok_or(DmaError::NoChannel)?
        .enqueue(txn_id)?;

    // Enregistrer pour la complétion (aucun wakeup : nul tid).
    DMA_COMPLETION.register(txn_id, ch_id, 0);

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// FALLBACK SOFTWARE
// ─────────────────────────────────────────────────────────────────────────────

/// Remplit logiciellement via le physmap (fallback ou petites régions).
///
/// # Safety
/// `dst` doit être mappé dans le physmap.
#[inline]
pub unsafe fn sw_memset(dst: PhysAddr, value: u8, size: usize) {
    use crate::memory::core::address::phys_to_virt;
    let ptr = phys_to_virt(dst).as_u64() as *mut u8;
    // SAFETY: physmap couvre toute la RAM.
    core::ptr::write_bytes(ptr, value, size);
}

/// Efface (zero) une zone physique via le physmap.
///
/// # Safety
/// Même préconditions que `sw_memset`.
#[inline]
pub unsafe fn sw_zero(dst: PhysAddr, size: usize) {
    sw_memset(dst, 0, size);
}
