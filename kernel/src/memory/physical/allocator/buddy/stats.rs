//! # memory/physical/allocator/buddy/stats.rs — Statistiques et métriques de fragmentation
//!
//! Analyse de l'état de fragmentation et calcul des ratios de mémoire libre.

use crate::memory::core::{ZoneType, BUDDY_ORDER_COUNT};

/// Statistiques d'une zone buddy.
#[derive(Copy, Clone, Debug)]
pub struct BuddyZoneStats {
    pub zone_type: ZoneType,
    pub numa_node: u8,
    pub total_frames: usize,
    pub free_frames: usize,
    pub alloc_count: usize,
    pub free_count: usize,
    pub fail_count: usize,
    pub split_count: usize,
    pub merge_count: usize,
    /// Nombre de blocs libres par ordre (0..BUDDY_ORDER_COUNT).
    pub free_by_order: [usize; BUDDY_ORDER_COUNT],
}

impl BuddyZoneStats {
    /// Pourcentage de mémoire libre (0.0-100.0).
    pub fn free_percent(&self) -> f32 {
        if self.total_frames == 0 {
            return 0.0;
        }
        self.free_frames as f32 * 100.0 / self.total_frames as f32
    }

    /// Taux de fragmentation (0.0 = aucune, 1.0 = maximale).
    /// Mesure l'écart entre la mémoire libre totale et les blocs de grand ordre.
    pub fn fragmentation_ratio(&self) -> f32 {
        if self.free_frames == 0 {
            return 0.0;
        }
        let largest_order = (0..BUDDY_ORDER_COUNT)
            .rev()
            .find(|&o| self.free_by_order[o] > 0)
            .unwrap_or(0);
        let max_contiguous = self.free_by_order[largest_order] * (1 << largest_order);
        1.0 - (max_contiguous as f32 / self.free_frames as f32)
    }
}
