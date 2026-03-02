// kernel/src/fs/exofs/storage/block_allocator.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Allocateur de blocs disque — interface de haut niveau
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Interface unifiée d'allocation/libération de blocs utilisée par :
// - object_writer.rs pour allouer des blocs de données
// - blob_writer.rs pour les P-Blobs
// - path_index.rs pour les pages d'index de chemins
//
// RÈGLE ARITH-01 : checked_add/sub sur tout offset.
// RÈGLE LOCK-04  : pas d'I/O dans la section critique.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, DiskOffset, Extent, BLOCK_SIZE,
};
use crate::fs::exofs::storage::heap::{ExofsHeap, blocks_for_size};
use crate::fs::exofs::core::stats::EXOFS_STATS;

// ─────────────────────────────────────────────────────────────────────────────
// BlockAllocator — wraps ExofsHeap avec métriques
// ─────────────────────────────────────────────────────────────────────────────

/// Allocation de blocs physiques sur le volume ExoFS.
pub struct BlockAllocator {
    heap: ExofsHeap,
    /// Nombre total de blocs libérés (référence pour le GC).
    freed_blocks: AtomicU64,
}

impl BlockAllocator {
    /// Initialise l'allocateur sur la plage [heap_start, heap_end).
    pub fn new(heap_start: u64, heap_end: u64) -> Self {
        Self {
            heap: ExofsHeap::new(heap_start, heap_end),
            freed_blocks: AtomicU64::new(0),
        }
    }

    /// Alloue un Extent contigu de `byte_count` octets.
    ///
    /// L'Extent retourné est aligné sur BLOCK_SIZE et sa longueur est
    /// un multiple de BLOCK_SIZE (arrondi supérieur).
    ///
    /// RÈGLE ARITH-01 : pas de débordement d'offset.
    pub fn alloc_extent(&self, byte_count: u64) -> ExofsResult<Extent> {
        if byte_count == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        let offset = self.heap.alloc_blocks(byte_count)?;
        let actual_len = {
            let block_count = blocks_for_size(byte_count)?;
            block_count
                .checked_mul(BLOCK_SIZE as u64)
                .ok_or(ExofsError::OffsetOverflow)?
        };
        // Écriture des statistiques.
        EXOFS_STATS.add_io_write(actual_len);
        Ok(Extent { offset, len: actual_len })
    }

    /// Libère un Extent (marque les blocs comme libres).
    ///
    /// Pour l'instant le bump allocator ne réutilise pas les blocs —
    /// c'est le GC qui recalcule la position next_free après collection.
    pub fn free_extent(&self, extent: Extent) {
        // Incrémentation du compteur de blocs libérés.
        let blocks = extent.len / BLOCK_SIZE as u64;
        self.freed_blocks.fetch_add(blocks, Ordering::Relaxed);
        EXOFS_STATS.add_io_write(0); // pas d'écriture : marquage logique seulement
    }

    /// Octets libres restants sur le heap.
    pub fn free_bytes(&self) -> u64 {
        self.heap.free_bytes()
    }

    /// Nombre de blocs libérés depuis le montage (pour le GC).
    pub fn freed_blocks(&self) -> u64 {
        self.freed_blocks.load(Ordering::Relaxed)
    }
}
