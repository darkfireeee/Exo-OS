// kernel/src/memory/dma/core/mapping.rs
//
// Mapping DMA — traduit les adresses physiques en IOVA (I/O Virtual Address)
// pour les transferts DMA avec ou sans IOMMU.
//
// COUCHE 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

use crate::memory::core::constants::PAGE_SIZE;
use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::descriptor::SgEntry;
use crate::memory::dma::core::types::{
    DmaDirection, DmaError, DmaMapFlags, IommuDomainId, IovaAddr,
};

// ─────────────────────────────────────────────────────────────────────────────
// ESPACE D'ADRESSAGE IOVA
// ─────────────────────────────────────────────────────────────────────────────

/// Base de l'espace IOVA (identique à phys en mode passthrough).
const IOVA_BASE: u64 = 0x0001_0000_0000; // Par convention: au-delà de 4 GiB
const IOVA_SIZE: u64 = 0x0010_0000_0000; // 64 GiB d'espace IOVA

/// Nom de la zone DMA legacy (< 16 MiB).
const DMA16_LIMIT: u64 = 16 * 1024 * 1024;
const DMA32_LIMIT: u64 = 4 * 1024 * 1024 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// DESCRIPTEUR DE MAPPING
// ─────────────────────────────────────────────────────────────────────────────

/// Un mapping DMA actif (physique → IOVA).
#[repr(C, align(32))]
pub struct DmaMapping {
    /// Adresse physique source du mapping.
    pub phys_base: PhysAddr,
    /// IOVA allouée pour ce mapping.
    pub iova_base: IovaAddr,
    /// Taille du mapping en octets (multiple de PAGE_SIZE).
    pub size: usize,
    /// Direction du transfert.
    pub direction: DmaDirection,
    /// Flags appliqués.
    pub flags: DmaMapFlags,
    /// Domaine IOMMU propriétaire.
    pub domain: IommuDomainId,
    /// Compteur de références (pour les buffers persistants partagés).
    ref_count: AtomicUsize,
    /// Réservé.
    _pad: [u8; 4],
}

impl DmaMapping {
    fn new(
        phys_base: PhysAddr,
        iova_base: IovaAddr,
        size: usize,
        direction: DmaDirection,
        flags: DmaMapFlags,
        domain: IommuDomainId,
    ) -> Self {
        DmaMapping {
            phys_base,
            iova_base,
            size,
            direction,
            flags,
            domain,
            ref_count: AtomicUsize::new(1),
            _pad: [0u8; 4],
        }
    }

    #[inline]
    pub fn iova_end(&self) -> IovaAddr {
        IovaAddr::new(self.iova_base.as_u64() + self.size as u64)
    }
    #[inline]
    pub fn phys_end(&self) -> PhysAddr {
        PhysAddr::new(self.phys_base.as_u64() + self.size as u64)
    }
    #[inline]
    pub fn inc_ref(&self) -> usize {
        self.ref_count.fetch_add(1, Ordering::Relaxed) + 1
    }
    #[inline]
    pub fn dec_ref(&self) -> usize {
        self.ref_count.fetch_sub(1, Ordering::Release) - 1
    }
    #[inline]
    pub fn ref_count(&self) -> usize {
        self.ref_count.load(Ordering::Relaxed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ALLOCATEUR IOVA (BUMP POINTER SIMPLE)
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum de mappings DMA simultanés.
pub const MAX_DMA_MAPPINGS: usize = 4096;

struct IovaAllocatorInner {
    /// Prochain IOVA disponible (bump pointer, page-aligned).
    next: u64,
    /// Mappings actifs (table plate, O(n) mais suffisant pour le boot).
    /// Après activation IOMMU, on utilisera les arbres IOMMU.
    active: [Option<DmaMapping>; MAX_DMA_MAPPINGS],
    count: usize,
}

pub struct IovaAllocator {
    inner: Mutex<IovaAllocatorInner>,
    /// Mode passthrough : pas de translation IOMMU (IOVA == PhysAddr).
    passthrough: AtomicUsize, // 0=IOMMU actif, 1=passthrough
    stats_allocs: AtomicU64,
    stats_frees: AtomicU64,
    stats_bytes: AtomicU64,
}

// SAFETY: IovaAllocator est protégé par Mutex.
unsafe impl Sync for IovaAllocator {}
unsafe impl Send for IovaAllocator {}

impl IovaAllocator {
    const fn new() -> Self {
        const EMPTY_MAPPING: Option<DmaMapping> = None;

        IovaAllocator {
            inner: Mutex::new(IovaAllocatorInner {
                next: IOVA_BASE,
                active: [EMPTY_MAPPING; MAX_DMA_MAPPINGS],
                count: 0,
            }),
            passthrough: AtomicUsize::new(1), // passthrough par défaut (avant IOMMU init)
            stats_allocs: AtomicU64::new(0),
            stats_frees: AtomicU64::new(0),
            stats_bytes: AtomicU64::new(0),
        }
    }

    /// Active le mode IOMMU (désactive le passthrough).
    pub fn enable_iommu(&self) {
        self.passthrough.store(0, Ordering::Release);
    }
    pub fn is_passthrough(&self) -> bool {
        self.passthrough.load(Ordering::Acquire) != 0
    }

    /// Mappe `phys` → IOVA de `size` octets.
    ///
    /// En mode passthrough : IOVA == phys (pas d'allocation réelle).
    /// En mode IOMMU : bump pointer dans [IOVA_BASE, IOVA_BASE+IOVA_SIZE).
    pub fn map(
        &self,
        phys: PhysAddr,
        size: usize,
        direction: DmaDirection,
        flags: DmaMapFlags,
        domain: IommuDomainId,
    ) -> Result<IovaAddr, DmaError> {
        // Valide l'alignement.
        if size == 0 {
            return Err(DmaError::InvalidParams);
        }
        let size_aligned = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        // Contraintes de zone.
        if flags.contains(DmaMapFlags::DMA16) && phys.as_u64() >= DMA16_LIMIT {
            return Err(DmaError::WrongZone);
        }
        if flags.contains(DmaMapFlags::DMA32) && phys.as_u64() >= DMA32_LIMIT {
            return Err(DmaError::WrongZone);
        }

        if self.is_passthrough() {
            // Mode passthrough : IOVA = PhysAddr directement.
            let iova = IovaAddr::new(phys.as_u64());
            self.stats_allocs.fetch_add(1, Ordering::Relaxed);
            self.stats_bytes
                .fetch_add(size_aligned as u64, Ordering::Relaxed);
            return Ok(iova);
        }

        // Mode IOMMU : alloue une plage IOVA.
        let mut inner = self.inner.lock();
        if inner.count >= MAX_DMA_MAPPINGS {
            return Err(DmaError::OutOfMemory);
        }

        let iova = IovaAddr::new(inner.next);
        inner.next = inner.next.wrapping_add(size_aligned as u64);
        if inner.next >= IOVA_BASE + IOVA_SIZE {
            // Wrap-around : on ne supporte pas encore la réutilisation.
            return Err(DmaError::OutOfMemory);
        }

        // Insère dans le premier slot libre.
        let slot = inner
            .active
            .iter_mut()
            .find(|s| s.is_none())
            .ok_or(DmaError::OutOfMemory)?;
        *slot = Some(DmaMapping::new(
            phys,
            iova,
            size_aligned,
            direction,
            flags,
            domain,
        ));
        inner.count += 1;

        drop(inner);
        self.stats_allocs.fetch_add(1, Ordering::Relaxed);
        self.stats_bytes
            .fetch_add(size_aligned as u64, Ordering::Relaxed);
        Ok(iova)
    }

    /// Libère un mapping IOVA.
    pub fn unmap(&self, iova: IovaAddr, domain: IommuDomainId) -> Result<(), DmaError> {
        if self.is_passthrough() {
            self.stats_frees.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }

        let mut inner = self.inner.lock();
        for slot in inner.active.iter_mut() {
            if let Some(ref m) = slot {
                if m.iova_base == iova && m.domain == domain {
                    let rc = m.dec_ref();
                    if rc == 0 {
                        // Invalider le TLB IOMMU pour cette IOVA dans le domaine.
                        // SAFETY: CPL 0; flush_iotlb_domain vérifie sa propre garde VT-d.
                        unsafe {
                            crate::memory::dma::iommu::intel_vtd::INTEL_VTD
                                .flush_iotlb_domain(domain.0 as u16, iova.0);
                        }
                        *slot = None;
                        inner.count -= 1;
                        drop(inner);
                        self.stats_frees.fetch_add(1, Ordering::Relaxed);
                        return Ok(());
                    }
                    return Ok(());
                }
            }
        }
        Err(DmaError::InvalidParams)
    }

    /// Mappe un scatter-gather en IOVA contigüe.
    pub fn map_sg(
        &self,
        entries: &[SgEntry],
        direction: DmaDirection,
        flags: DmaMapFlags,
        domain: IommuDomainId,
        out_iovas: &mut [IovaAddr],
    ) -> Result<usize, DmaError> {
        if entries.len() > out_iovas.len() {
            return Err(DmaError::InvalidParams);
        }
        let mut mapped = 0usize;
        for (entry, iova_out) in entries.iter().zip(out_iovas.iter_mut()) {
            if entry.is_empty() {
                break;
            }
            let iova = self.map(entry.phys, entry.len as usize, direction, flags, domain)?;
            *iova_out = iova;
            mapped += 1;
        }
        Ok(mapped)
    }

    /// Synchronise le cache CPU pour un mapping (for_cpu ou for_device).
    ///
    /// Sur x86_64 avec DMA cohérent (Intel / AMD-Vi) : no-op.
    /// Sur ARM/RISC-V : appellerait les instructions de flush de cache.
    #[inline]
    pub fn sync_for_cpu(&self, _iova: IovaAddr, _size: usize) {
        // x86_64 : cache cohérent avec DMA — aucune action nécessaire.
        // Une barrière mémoire suffit pour les visibilités ordonnées.
        core::sync::atomic::fence(Ordering::Acquire);
    }

    #[inline]
    pub fn sync_for_device(&self, _iova: IovaAddr, _size: usize) {
        core::sync::atomic::fence(Ordering::Release);
    }

    /// Statistiques de l'allocateur IOVA.
    pub fn stats(&self) -> (u64, u64, u64) {
        (
            self.stats_allocs.load(Ordering::Relaxed),
            self.stats_frees.load(Ordering::Relaxed),
            self.stats_bytes.load(Ordering::Relaxed),
        )
    }
}

/// Allocateur IOVA global (système).
pub static IOVA_ALLOCATOR: IovaAllocator = IovaAllocator::new();
