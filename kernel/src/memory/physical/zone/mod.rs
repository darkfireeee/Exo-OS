// kernel/src/memory/physical/zone/mod.rs
//
// Zones mémoire physiques — Exo-OS Couche 0.
// Chaque zone est une partition de la RAM avec ses propres contraintes.

pub mod dma;
pub mod dma32;
pub mod normal;
pub mod high;
pub mod movable;

use core::sync::atomic::{AtomicUsize, AtomicU64, Ordering};
use crate::memory::core::{
    PhysAddr, Frame, AllocFlags, ZoneType,
    ZONE_DMA_END, ZONE_DMA32_END,
};

// ─────────────────────────────────────────────────────────────────────────────
// ZONE DESCRIPTOR — structure commune à toutes les zones
// ─────────────────────────────────────────────────────────────────────────────

/// Descripteur d'une zone mémoire — statistiques et limites.
///
/// Partagé entre toutes les zones, complété par leur module spécifique.
#[repr(C, align(64))]
pub struct ZoneDescriptor {
    /// Type de cette zone.
    pub zone_type:    ZoneType,
    /// Nœud NUMA d'appartenance.
    pub numa_node:    u8,
    /// Padding.
    _pad0:            [u8; 6],
    /// Adresse physique de début de zone.
    pub phys_start:   PhysAddr,
    /// Adresse physique de fin de zone (exclusive).
    pub phys_end:     PhysAddr,
    /// Nombre total de frames dans cette zone.
    pub total_frames: usize,
    /// Nombre de frames libres actuellement.
    free_frames:      AtomicUsize,
    /// Nombre de frames réservés (firmware/BIOS).
    pub reserved_frames: usize,
    /// Nombre d'allocations réussies depuis cette zone.
    alloc_success:    AtomicU64,
    /// Nombre d'échecs d'allocation dans cette zone.
    alloc_failures:   AtomicU64,
    /// Nombre de libérations effectuées dans cette zone.
    free_count:       AtomicU64,
    /// Watermark bas : sous ce seuil, kswapd se réveille.
    pub watermark_low:  usize,
    /// Watermark minimum : sous ce seuil, seulement les allocs urgentes passent.
    pub watermark_min:  usize,
    /// Watermark haut : au-dessus, kswapd s'arrête.
    pub watermark_high: usize,
    /// Padding pour atteindre 128 bytes.
    /// Calcul : 128 - 1(zone_type) - 1(numa_node) - 6(_pad0)
    ///          - 16(phys_start+phys_end) - 24(total+free+reserved frames)
    ///          - 24(alloc_success+alloc_failures+free_count)
    ///          - 24(watermark_low+min+high) = 128 - 96 = 32
    _pad1: [u8; 32],
}

const _: () = assert!(
    core::mem::size_of::<ZoneDescriptor>() == 128,
    "ZoneDescriptor doit faire exactement 128 bytes (2 cache lines)"
);

impl ZoneDescriptor {
    /// Crée un descripteur de zone.
    pub const fn new(
        zone_type:      ZoneType,
        numa_node:      u8,
        phys_start:     PhysAddr,
        phys_end:       PhysAddr,
        total_frames:   usize,
        reserved_frames: usize,
    ) -> Self {
        let wm_min  = total_frames / 100;       // 1%
        let wm_low  = total_frames * 3 / 100;   // 3%
        let wm_high = total_frames * 5 / 100;   // 5%

        ZoneDescriptor {
            zone_type,
            numa_node,
            _pad0: [0u8; 6],
            phys_start,
            phys_end,
            total_frames,
            free_frames:     AtomicUsize::new(total_frames - reserved_frames),
            reserved_frames,
            alloc_success:   AtomicU64::new(0),
            alloc_failures:  AtomicU64::new(0),
            free_count:      AtomicU64::new(0),
            watermark_low:   wm_low,
            watermark_min:   wm_min,
            watermark_high:  wm_high,
            _pad1: [0u8; 32],
        }
    }

    /// Retourne le nombre de frames libres.
    #[inline(always)]
    pub fn free_frames(&self) -> usize {
        self.free_frames.load(Ordering::Relaxed)
    }

    /// Décrémente le compteur de frames libres (après allocation).
    #[inline(always)]
    pub fn dec_free(&self, count: usize) {
        self.free_frames.fetch_sub(count, Ordering::Relaxed);
        self.alloc_success.fetch_add(1, Ordering::Relaxed);
    }

    /// Incrémente le compteur de frames libres (après libération).
    #[inline(always)]
    pub fn inc_free(&self, count: usize) {
        self.free_frames.fetch_add(count, Ordering::Relaxed);
        self.free_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Signale un échec d'allocation.
    #[inline(always)]
    pub fn record_failure(&self) {
        self.alloc_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Vérifie si un frame appartient à cette zone.
    #[inline(always)]
    pub fn contains(&self, frame: Frame) -> bool {
        let addr = frame.start_address();
        addr >= self.phys_start && addr < self.phys_end
    }

    /// Vérifie si la zone est sous le watermark minimum.
    #[inline(always)]
    pub fn is_below_watermark_min(&self) -> bool {
        self.free_frames() < self.watermark_min
    }

    /// Vérifie si la zone est sous le watermark bas (kswapd doit se réveiller).
    #[inline(always)]
    pub fn is_below_watermark_low(&self) -> bool {
        self.free_frames() < self.watermark_low
    }

    /// Statistiques de cette zone.
    pub fn stats(&self) -> ZoneStats {
        ZoneStats {
            zone_type:      self.zone_type,
            numa_node:      self.numa_node,
            total_frames:   self.total_frames,
            free_frames:    self.free_frames(),
            reserved_frames: self.reserved_frames,
            alloc_success:  self.alloc_success.load(Ordering::Relaxed),
            alloc_failures: self.alloc_failures.load(Ordering::Relaxed),
            free_count:     self.free_count.load(Ordering::Relaxed),
        }
    }
}

/// Statistiques d'une zone mémoire.
#[derive(Copy, Clone, Debug)]
pub struct ZoneStats {
    pub zone_type:      ZoneType,
    pub numa_node:      u8,
    pub total_frames:   usize,
    pub free_frames:    usize,
    pub reserved_frames: usize,
    pub alloc_success:  u64,
    pub alloc_failures: u64,
    pub free_count:     u64,
}

impl ZoneStats {
    /// Pourcentage de mémoire libre (0-100).
    pub fn free_percent(&self) -> u32 {
        if self.total_frames == 0 { return 0; }
        (self.free_frames * 100 / self.total_frames) as u32
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ZONE REGISTRY — registre global des zones
// ─────────────────────────────────────────────────────────────────────────────

/// Sélectionne la zone appropriée pour les flags d'allocation donnés.
#[inline]
pub fn zone_for_flags(flags: AllocFlags) -> ZoneType {
    flags.required_zone()
}

/// Détermine si une adresse physique peut satisfaire des flags d'allocation.
///
/// Par exemple, une allocation DMA exige une adresse < 16 MiB.
#[inline]
pub fn addr_satisfies_flags(addr: PhysAddr, flags: AllocFlags) -> bool {
    let required = zone_for_flags(flags);
    match required {
        ZoneType::Dma     => addr.as_usize() < ZONE_DMA_END,
        ZoneType::Dma32   => addr.as_usize() < ZONE_DMA32_END,
        ZoneType::Movable => true, // Any address can be movable
        _                 => true, // Normal/High : pas de contrainte d'adresse basse
    }
}

pub use dma::DmaZone;
pub use dma32::Dma32Zone;
pub use normal::NormalZone;
pub use high::HighZone;
pub use movable::MovableZone;
