//! heap_allocator.rs — Allocateur de heap ExoFS (gestion des blocs libres, no_std).

use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::FsError;
use super::heap_free_map::HeapFreeMap;

/// Résultat d'une allocation heap.
#[derive(Clone, Copy, Debug)]
pub struct HeapAlloc {
    pub offset: u64,   // Offset en octets depuis le début de la zone heap.
    pub size:   u64,   // Taille réellement allouée (peut être supérieure à la demande, alignée).
}

/// Allocateur de heap ExoFS.
///
/// Utilise une carte de bits (`HeapFreeMap`) pour suivre les blocs libres.
/// L'allocation cherche un run contigu de blocs libres (first-fit).
pub struct HeapAllocator {
    free_map:       SpinLock<HeapFreeMap>,
    total_blocks:   u64,
    block_size:     u32,
    allocated:      AtomicU64,
    n_allocs:       AtomicU64,
    n_frees:        AtomicU64,
}

impl HeapAllocator {
    pub fn new(total_blocks: u64, block_size: u32) -> Result<Self, FsError> {
        let free_map = HeapFreeMap::new(total_blocks)?;
        Ok(Self {
            free_map:     SpinLock::new(free_map),
            total_blocks,
            block_size,
            allocated:    AtomicU64::new(0),
            n_allocs:     AtomicU64::new(0),
            n_frees:      AtomicU64::new(0),
        })
    }

    /// Alloue `size` octets — arrondit vers le haut au prochain multiple de block_size.
    /// RÈGLE 14 : tous les calculs d'offset utilisent checked_add.
    pub fn alloc(&self, size: u64) -> Result<HeapAlloc, FsError> {
        if size == 0 { return Err(FsError::InvalidArgument); }
        let bs = self.block_size as u64;
        let n_blocks = size.checked_add(bs - 1).ok_or(FsError::Overflow)? / bs;

        let mut map = self.free_map.lock();
        let start   = map.find_free_run(n_blocks).ok_or(FsError::OutOfMemory)?;
        map.mark_used(start, n_blocks);

        let offset = start.checked_mul(bs).ok_or(FsError::Overflow)?;
        let alloc_size = n_blocks.checked_mul(bs).ok_or(FsError::Overflow)?;

        self.allocated.fetch_add(alloc_size, Ordering::Relaxed);
        self.n_allocs.fetch_add(1, Ordering::Relaxed);
        Ok(HeapAlloc { offset, size: alloc_size })
    }

    /// Libère un bloc précédemment alloué.
    pub fn free(&self, offset: u64, size: u64) -> Result<(), FsError> {
        let bs = self.block_size as u64;
        let start    = offset / bs;
        let n_blocks = size.checked_add(bs - 1).ok_or(FsError::Overflow)? / bs;

        self.free_map.lock().mark_free(start, n_blocks);
        self.allocated.fetch_sub(
            n_blocks * bs,
            Ordering::Relaxed,
        );
        self.n_frees.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn total_blocks(&self) -> u64 { self.total_blocks }
    pub fn block_size(&self)   -> u32 { self.block_size }
    pub fn allocated_bytes(&self) -> u64 { self.allocated.load(Ordering::Relaxed) }
    pub fn free_bytes(&self) -> u64 {
        (self.total_blocks * self.block_size as u64)
            .saturating_sub(self.allocated.load(Ordering::Relaxed))
    }
    pub fn n_allocs(&self) -> u64 { self.n_allocs.load(Ordering::Relaxed) }
    pub fn n_frees(&self)  -> u64 { self.n_frees.load(Ordering::Relaxed) }
}
