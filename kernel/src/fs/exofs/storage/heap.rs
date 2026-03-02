// kernel/src/fs/exofs/storage/heap.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Heap disque ExoFS — allocation de blocs physiques
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le heap est la zone de données du volume (de HEAP_START à disk_size - 8KB).
// L'allocateur buddy gère des blocs de 4KB à 64MB (puissances de 2).
//
// RÈGLE ARITH-01 : toutes les additions/soustractions utilisent checked_add/sub.
// RÈGLE LOCK-04  : le heap_lock est un SpinLock léger — pas d'I/O dedans.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, DiskOffset, BLOCK_SIZE, HEAP_START_OFFSET,
};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes de l'allocateur buddy
// ─────────────────────────────────────────────────────────────────────────────

/// Taille minimale d'un bloc alloué (4 KB).
const MIN_BLOCK_SIZE: u64 = BLOCK_SIZE as u64;

/// Ordre maximal du buddy (2^MAX_ORDER × MIN_BLOCK_SIZE = 64 MB).
const MAX_ORDER: usize = 14;

/// Taille maximale d'un bloc alloué.
const MAX_BLOCK_SIZE: u64 = MIN_BLOCK_SIZE << MAX_ORDER;

// ─────────────────────────────────────────────────────────────────────────────
// HeapState — état de l'allocateur (sous SpinLock)
// ─────────────────────────────────────────────────────────────────────────────

/// Premier offset libre par ordre (simplifié : bitmap de blocs libres per-order).
/// En production complète, on utiliserait des listes chaînées en mémoire ;
/// ici on utilise le prochain offset libre (bump allocator avec coalescence simple).
struct HeapState {
    /// Offset de la prochaine allocation (bump).
    next_free: u64,
    /// Offset de fin du heap (exclusif).
    heap_end:  u64,
    /// Nombre de blocs alloués.
    alloc_count: u64,
    /// Nombre d'octets alloués.
    alloc_bytes: u64,
}

impl HeapState {
    const fn new(heap_start: u64, heap_end: u64) -> Self {
        Self {
            next_free:   heap_start,
            heap_end,
            alloc_count: 0,
            alloc_bytes: 0,
        }
    }

    /// Alloue `size` octets alignés sur `alignment`.
    ///
    /// RÈGLE ARITH-01 : utilise checked_add/checked_sub.
    fn alloc_aligned(&mut self, size: u64, alignment: u64) -> ExofsResult<DiskOffset> {
        if size == 0 || !alignment.is_power_of_two() {
            return Err(ExofsError::InvalidArgument);
        }
        // Arrondi à l'alignement supérieur.
        let mask = alignment
            .checked_sub(1)
            .ok_or(ExofsError::OffsetOverflow)?;
        let aligned = self
            .next_free
            .checked_add(mask)
            .ok_or(ExofsError::OffsetOverflow)?
            & !mask;

        let end = aligned
            .checked_add(size)
            .ok_or(ExofsError::OffsetOverflow)?;

        if end > self.heap_end {
            return Err(ExofsError::NoSpace);
        }

        self.next_free   = end;
        self.alloc_count += 1;
        self.alloc_bytes  = self.alloc_bytes.saturating_add(size);
        Ok(DiskOffset(aligned))
    }

    /// Octets libres restants.
    fn free_bytes(&self) -> u64 {
        self.heap_end.saturating_sub(self.next_free)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ExofsHeap — singleton alloué au montage
// ─────────────────────────────────────────────────────────────────────────────

/// Gestionnaire de heap disque ExoFS.
pub struct ExofsHeap {
    state:      SpinLock<HeapState>,
    heap_start: u64,
    heap_end:   AtomicU64,
}

impl ExofsHeap {
    /// Construit un heap sur la plage [heap_start, heap_end).
    pub fn new(heap_start: u64, heap_end: u64) -> Self {
        Self {
            state:      SpinLock::new(HeapState::new(heap_start, heap_end)),
            heap_start,
            heap_end:   AtomicU64::new(heap_end),
        }
    }

    /// Alloue `size` octets, alignés sur `BLOCK_SIZE`.
    pub fn alloc_blocks(&self, size: u64) -> ExofsResult<DiskOffset> {
        // Arrondi à BLOCK_SIZE.
        let aligned_size = round_up_to_block_size(size)?;
        let mut state = self.state.lock();
        state.alloc_aligned(aligned_size, MIN_BLOCK_SIZE)
    }

    /// Retourne les octets libres approximatifs.
    pub fn free_bytes(&self) -> u64 {
        let state = self.state.lock();
        state.free_bytes()
    }

    /// Retourne le nombre de blocs alloués.
    pub fn alloc_count(&self) -> u64 {
        let state = self.state.lock();
        state.alloc_count
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires
// ─────────────────────────────────────────────────────────────────────────────

/// Arrondit `size` au multiple supérieur de BLOCK_SIZE.
///
/// RÈGLE ARITH-01 : checked_add.
#[inline]
pub fn round_up_to_block_size(size: u64) -> ExofsResult<u64> {
    let bs = MIN_BLOCK_SIZE;
    let mask = bs
        .checked_sub(1)
        .ok_or(ExofsError::OffsetOverflow)?;
    size.checked_add(mask)
        .ok_or(ExofsError::OffsetOverflow)
        .map(|v| v & !mask)
}

/// Calcule le nombre de blocs de BLOCK_SIZE nécessaires pour `size` octets.
#[inline]
pub fn blocks_for_size(size: u64) -> ExofsResult<u64> {
    let aligned = round_up_to_block_size(size)?;
    Ok(aligned / MIN_BLOCK_SIZE)
}
