// kernel/src/memory/physical/stats.rs
//
//
// STATISTIQUES PHYSIQUES  wrappers vers le buddy allocator
//
//
// Ce module expose les statistiques globales du sous-système physical memory.
// Utilisé par syscall pour implémenter sysinfo/mem_info.
//
// COUCHE 0  aucune dépendance externe.

use crate::memory::physical::BUDDY;

pub use crate::memory::core::PAGE_SIZE;

/// Retourne le nombre de pages physiques libres.
#[inline]
pub fn free_pages() -> usize {
    BUDDY.total_free_frames()
}

/// Retourne le nombre total de pages physiques gérées par le noyau.
#[inline]
pub fn total_pages() -> usize {
    BUDDY.total_frames()
}

/// Retourne la quantité de mémoire libre en octets.
#[inline]
pub fn free_bytes() -> usize {
    free_pages() * PAGE_SIZE
}

/// Retourne la quantité totale de mémoire physique en octets.
#[inline]
pub fn total_bytes() -> usize {
    total_pages() * PAGE_SIZE
}

/// Retourne le nombre de pages utilisées (allouées).
#[inline]
pub fn used_pages() -> usize {
    total_pages().saturating_sub(free_pages())
}
