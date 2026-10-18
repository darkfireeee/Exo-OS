//! # memory/physical/allocator/buddy/mod.rs — Allocateur physique principal Buddy O(log n)
//!
//! Implémentation classique du système de buddy avec free-lists par ordre.
//! Couche 0 — aucune dépendance externe.
//!
//! Principe :
//!   La RAM est divisée en blocs de taille 2^order × PAGE_SIZE.
//!   Chaque ordre dispose d'une free-list doublement chaînée.
//!   Allocation  : parcours des ordres de bas en haut jusqu'au premier
//!                 bloc disponible, puis division des blocs supérieurs.
//!   Libération  : recherche du buddy et fusion récursive (coalescing).
//!
//! Invariants :
//!   - Un bloc d'ordre k a une adresse alignée sur 2^k pages.
//!   - Le buddy d'un bloc d'adresse P est P ⊕ (1 << k).
//!   - Jamais de fragmentation externe au-delà de 1 bloc de chaque ordre.

pub mod node;
pub mod stats;
pub mod zone;

pub use stats::BuddyZoneStats;
pub use zone::{reserve_frame_descriptor, unreserve_frame_descriptor, BuddyZone};

use core::sync::atomic::{AtomicBool, Ordering};
use crate::memory::core::{AllocError, AllocFlags, Frame, PhysAddr, ZoneType, PAGE_SIZE};

// ─────────────────────────────────────────────────────────────────────────────
// ALLOCATEUR BUDDY GLOBAL — couvre toutes les zones
// ─────────────────────────────────────────────────────────────────────────────

/// Allocateur buddy global couvrant toutes les zones et tous les nœuds NUMA.
///
/// Routage des allocations selon les flags :
///   - AllocFlags::DMA    → BuddyZone DMA
///   - AllocFlags::DMA32  → BuddyZone DMA32
///   - Default           → BuddyZone Normal (NUMA-preferred)
pub struct GlobalBuddyAllocator {
    // Une zone par ZoneType par nœud NUMA
    // Simplifié : tableau [ZoneType::COUNT] (NUMA node 0 par défaut pour l'instant)
    zones: [BuddyZone; 4], // DMA, DMA32, Normal, Movable
    initialized: AtomicBool,
}

// SAFETY: GlobalBuddyAllocator est thread-safe via ses spinlocks internes.
unsafe impl Sync for GlobalBuddyAllocator {}
unsafe impl Send for GlobalBuddyAllocator {}

impl GlobalBuddyAllocator {
    pub const fn new() -> Self {
        GlobalBuddyAllocator {
            zones: [const { BuddyZone::new_uninit() }; 4],
            initialized: AtomicBool::new(false),
        }
    }

    /// Alloue `2^order` pages contiguës selon les flags.
    /// Sélectionne automatiquement la zone appropriée.
    #[inline]
    pub fn alloc_pages(&self, order: usize, flags: AllocFlags) -> Result<Frame, AllocError> {
        debug_assert!(
            self.initialized.load(Ordering::Acquire),
            "GlobalBuddyAllocator non initialisé"
        );

        let zone_idx = self.zone_index_for(flags);
        let result = self.zones[zone_idx].alloc_pages(order, flags);

        // Fallback vers zone supérieure si la zone cible est épuisée
        if result.is_err() && !flags.contains(AllocFlags::DMA) {
            for fallback_idx in (zone_idx + 1)..4 {
                let r = self.zones[fallback_idx].alloc_pages(order, flags);
                if r.is_ok() {
                    return r;
                }
            }
        }

        // Fallback vers zone inférieure (Normal → DMA32 sur systèmes avec < 4 GiB RAM).
        if result.is_err() && zone_idx > 1 {
            for fallback_idx in (1..zone_idx).rev() {
                let r = self.zones[fallback_idx].alloc_pages(order, flags);
                if r.is_ok() {
                    return r;
                }
            }
        }

        result
    }

    /// Libère un bloc de `2^order` pages.
    #[inline]
    pub fn free_pages(&self, frame: Frame, order: usize) -> Result<(), AllocError> {
        let phys = frame.start_address();
        // Déterminer la zone d'appartenance
        for zone in &self.zones {
            if zone.is_initialized() && zone.phys_start <= phys && phys < zone.phys_end {
                return zone.free_pages(frame, order);
            }
        }
        Err(AllocError::ZoneUnavailable)
    }

    /// Alloue depuis le nœud NUMA demandé.
    pub fn alloc_on_node(
        &self,
        order: usize,
        flags: AllocFlags,
        numa_node: u8,
    ) -> Result<Frame, AllocError> {
        debug_assert!(
            self.initialized.load(Ordering::Acquire),
            "GlobalBuddyAllocator non initialisé"
        );

        // Phase 1 : essai dans les zones NUMA-locales.
        for zone in &self.zones {
            if !zone.is_initialized() {
                continue;
            }
            if zone.numa_node != numa_node {
                continue;
            }
            if let Ok(frame) = zone.alloc_pages(order, flags) {
                return Ok(frame);
            }
        }

        // Phase 2 : fallback global (allocation cross-NUMA).
        self.alloc_pages(order, flags)
    }

    #[inline]
    fn zone_index_for(&self, flags: AllocFlags) -> usize {
        match flags.required_zone() {
            ZoneType::Dma => 0,
            ZoneType::Dma32 => 1,
            ZoneType::Movable => 3,
            _ => 2, // Normal par défaut
        }
    }

    /// Retourne le nombre total de frames libres dans toutes les zones.
    pub fn total_free_frames(&self) -> usize {
        self.zones
            .iter()
            .filter(|z| z.is_initialized())
            .map(|z| z.free_frames())
            .sum()
        }

    /// Retourne le nombre total de frames physiques gérées (libres + utilisées).
    pub fn total_frames(&self) -> usize {
        self.zones
            .iter()
            .filter(|z| z.is_initialized())
            .map(|z| z.total_frames_count())
            .sum()
    }

    /// Marque l'allocateur comme initialisé.
    pub fn mark_initialized(&self) {
        self.initialized.store(true, Ordering::Release);
    }

    /// Initialise une zone de l'allocateur (appelé au boot, single-CPU).
    ///
    /// # Safety
    /// - Doit être appelé UNE SEULE FOIS par zone, avant SMP.
    /// - `bitmap_buf` doit être valide pour `bitmap_words` × 8 bytes.
    pub unsafe fn init_zone(
        &self,
        zone_type: ZoneType,
        phys_start: PhysAddr,
        phys_end: PhysAddr,
        bitmap_buf: *mut u64,
        bitmap_words: usize,
    ) {
        let idx = (zone_type.index()).min(3);
        self.zones[idx].init(
            zone_type,
            0, /* NUMA node 0 */
            phys_start,
            phys_end,
            bitmap_buf,
            bitmap_words,
        );
    }

    /// Ajoute une plage de frames libres aux zones initialisées qui couvrent cette plage.
    ///
    /// # Safety
    /// - Zones concernées doivent être initialisées.
    /// - La plage ne doit pas contenir de mémoire kernel active.
    pub unsafe fn add_free_zone_region(&self, start: PhysAddr, end: PhysAddr) {
        for zone in &self.zones {
            if !zone.is_initialized() {
                continue;
            }
            let zs = zone.phys_start.as_u64();
            let ze = zone.phys_end.as_u64();
            let rs = start.as_u64();
            let re = end.as_u64();
            if rs >= ze || re <= zs {
                continue;
            } // pas d'overlap
            let cs = rs.max(zs);
            let ce = re.min(ze);
            if cs >= ce {
                continue;
            }
            let first_pfn = ((cs - zs) / PAGE_SIZE as u64) as usize;
            let last_pfn = ((ce - zs) / PAGE_SIZE as u64) as usize;
            zone.add_free_range(first_pfn, last_pfn);
        }
    }

    /// Retourne une référence à la zone pour un type donné.
    pub fn zone(&self, zone_type: ZoneType) -> Option<&BuddyZone> {
        let idx = match zone_type {
            ZoneType::Dma => 0,
            ZoneType::Dma32 => 1,
            ZoneType::Normal => 2,
            ZoneType::Movable => 3,
            ZoneType::High => return None,
        };
        let z = &self.zones[idx];
        if z.is_initialized() {
            Some(z)
        } else {
            None
        }
    }

    /// Accès mutant à une zone (initialisation uniquement).
    ///
    /// SAFETY: Doit être appelé uniquement pendant l'init, en single-CPU.
    pub unsafe fn zone_mut(&self, zone_type: ZoneType) -> Option<&BuddyZone> {
        self.zone(zone_type)
    }
}

/// Allocateur buddy global.
pub static BUDDY: GlobalBuddyAllocator = GlobalBuddyAllocator::new();

/// Alloue `2^order` pages physiques contiguës.
#[inline(always)]
pub fn alloc_pages(order: usize, flags: AllocFlags) -> Result<Frame, AllocError> {
    BUDDY.alloc_pages(order, flags)
}

/// Libère un bloc de `2^order` pages.
#[inline(always)]
pub fn free_pages(frame: Frame, order: usize) -> Result<(), AllocError> {
    BUDDY.free_pages(frame, order)
}

/// Alloue exactement 1 page physique (ordre 0).
#[inline(always)]
pub fn alloc_page(flags: AllocFlags) -> Result<Frame, AllocError> {
    alloc_pages(0, flags)
}

/// Libère exactement 1 page physique.
#[inline(always)]
pub fn free_page(frame: Frame) -> Result<(), AllocError> {
    free_pages(frame, 0)
}

/// Réserve un frame physique contre toute libération/réallocation buddy.
#[inline(always)]
pub fn reserve_frame(frame: Frame) {
    reserve_frame_descriptor(frame);
}

/// Réserve une série de frames physiques contiguës.
#[inline(always)]
pub fn reserve_frames(frames: &[Frame]) {
    for &frame in frames {
        reserve_frame_descriptor(frame);
    }
}

/// Désépingle un frame physique précédemment réservé.
#[inline(always)]
pub fn unreserve_frame(frame: Frame) {
    unreserve_frame_descriptor(frame);
}

/// Désépingle une série de frames physiques.
#[inline(always)]
pub fn unreserve_frames(frames: &[Frame]) {
    for &frame in frames {
        unreserve_frame_descriptor(frame);
    }
}
