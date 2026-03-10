// kernel/src/fs/exofs/storage/block_allocator.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Allocateur de blocs ExoFS — interface de bas niveau pour l'objet/blob writer
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le BlockAllocator est la couche de bas niveau utilisée par object_writer et
// blob_writer pour obtenir des extents disque contiguement alloués avant l'écriture.
//
// Il s'appuie sur le ExofsHeap mais expose une interface simplifiée centrée
// sur les extents et leur cycle de vie (alloc → write → commit).
//
// Règles :
// - ARITH-02 : checked_add/mul pour TOUS les calculs d'offset.
// - LOCK-04  : SpinLock uniquement pour les listes partagées.
// - WRITE-02 : la vérification bytes_written est la responsabilité du caller.

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::vec::Vec;
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{ExofsError, ExofsResult, DiskOffset};
use crate::fs::exofs::storage::heap_allocator::{Extent, HeapAllocator, AllocationPolicy};
use crate::fs::exofs::storage::storage_stats::STORAGE_STATS;

// ─────────────────────────────────────────────────────────────────────────────
// ExtentHandle — extent avec état de cycle de vie
// ─────────────────────────────────────────────────────────────────────────────

/// État d'un extent alloué.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExtentState {
    /// Alloué, pas encore écrit.
    Reserved,
    /// Écrit avec succès, données persistées.
    Committed,
    /// Libéré (sera récupéré par le GC).
    Freed,
}

/// Extent avec suivi de l'état.
#[derive(Clone, Debug)]
pub struct ExtentHandle {
    pub extent: Extent,
    pub state:  ExtentState,
    /// Nombre d'octets effectivement écrits dans cet extent.
    pub written: u64,
}

impl ExtentHandle {
    pub fn new(extent: Extent) -> Self {
        Self { extent, state: ExtentState::Reserved, written: 0 }
    }

    /// Marque l'extent comme commité.
    #[inline]
    pub fn commit(&mut self, bytes_written: u64) {
        self.written = bytes_written;
        self.state   = ExtentState::Committed;
    }

    /// Marque l'extent comme libéré.
    #[inline]
    pub fn free_it(&mut self) {
        self.state = ExtentState::Freed;
    }

    /// Taux d'utilisation de l'extent (0..=100).
    pub fn fill_pct(&self) -> u64 {
        if self.extent.size == 0 { return 0; }
        (self.written as u128 * 100 / self.extent.size as u128).min(100) as u64
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BlockAllocRequest — paramètres d'une demande d'allocation
// ─────────────────────────────────────────────────────────────────────────────

/// Paramètres d'une allocation de blocs.
#[derive(Clone, Debug)]
pub struct BlockAllocRequest {
    /// Taille demandée en octets (arrondie automatiquement à BLOCK_SIZE).
    pub size:     u64,
    /// Politique d'allocation.
    pub policy:   AllocationPolicy,
    /// Forcer l'alignement sur cette puissance de 2 (0 = pas d'alignement spécial).
    pub align:    u64,
}

impl Default for BlockAllocRequest {
    fn default() -> Self {
        Self {
            size:   0,
            policy: AllocationPolicy::NextFit,
            align:  0,
        }
    }
}

impl BlockAllocRequest {
    pub fn of_size(size: u64) -> Self {
        Self { size, ..Default::default() }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BlockAllocatorState — état interne protégé par SpinLock
// ─────────────────────────────────────────────────────────────────────────────

struct BlockAllocatorState {
    /// Liste des extents en cours (Reserved ou Committed).
    handles: Vec<ExtentHandle>,
}

impl BlockAllocatorState {
    fn new() -> Self {
        Self { handles: Vec::new() }
    }

    /// Ajoute un handle (OOM-02 : try_reserve).
    fn push(&mut self, handle: ExtentHandle) -> ExofsResult<()> {
        self.handles.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.handles.push(handle);
        Ok(())
    }

    /// Retire et retourne les handles dont l'état est `Freed`.
    fn drain_freed(&mut self) -> Vec<Extent> {
        let mut freed = Vec::new();
        let mut i = 0;
        while i < self.handles.len() {
            if self.handles[i].state == ExtentState::Freed {
                freed.push(self.handles.swap_remove(i).extent);
            } else {
                i += 1;
            }
        }
        freed
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BlockAllocator
// ─────────────────────────────────────────────────────────────────────────────

/// Allocateur de blocs ExoFS.
///
/// Interface de bas niveau pour object_writer et blob_writer.
pub struct BlockAllocator {
    heap:       HeapAllocator,
    state:      SpinLock<BlockAllocatorState>,
    // ── Statistiques ───────────────────────────────────────────────────────
    n_allocs:   AtomicU64,
    n_commits:  AtomicU64,
    n_rollbacks: AtomicU64,
    bytes_commit: AtomicU64,
}

impl BlockAllocator {
    // ── Constructeur ─────────────────────────────────────────────────────────

    /// Crée un allocateur de blocs.
    pub fn new(heap_start: u64, total_blocks: u64) -> ExofsResult<Self> {
        let heap = HeapAllocator::new(heap_start, total_blocks)?;
        Ok(Self {
            heap,
            state:        SpinLock::new(BlockAllocatorState::new()),
            n_allocs:     AtomicU64::new(0),
            n_commits:    AtomicU64::new(0),
            n_rollbacks:  AtomicU64::new(0),
            bytes_commit: AtomicU64::new(0),
        })
    }

    // ── Allocation ────────────────────────────────────────────────────────────

    /// Alloue un extent et retourne un `ExtentHandle` en état `Reserved`.
    pub fn alloc_extent(&self, size: u64) -> ExofsResult<Extent> {
        let req = BlockAllocRequest::of_size(size);
        self.alloc_with_request(&req)
    }

    /// Alloue via une requête explicite.
    pub fn alloc_with_request(&self, req: &BlockAllocRequest) -> ExofsResult<Extent> {
        if req.size == 0 {
            return Err(ExofsError::InvalidArgument);
        }

        // Allouer dans le heap.
        let extent = self.heap.alloc_with_policy(req.size, req.policy)?;

        // Tracker dans la liste.
        let handle = ExtentHandle::new(extent);
        {
            let mut st = self.state.lock();
            st.push(handle)?;
        }

        self.n_allocs.fetch_add(1, Ordering::Relaxed);
        STORAGE_STATS.inc_heap_alloc(extent.size);

        Ok(extent)
    }

    // ── Commit ────────────────────────────────────────────────────────────────

    /// Marque un extent comme commité (données écrites avec succès).
    pub fn commit_extent(&self, offset: DiskOffset, bytes_written: u64) -> ExofsResult<()> {
        let mut st = self.state.lock();
        for h in st.handles.iter_mut() {
            if h.extent.offset == offset {
                if h.state == ExtentState::Reserved {
                    h.commit(bytes_written);
                    self.n_commits.fetch_add(1, Ordering::Relaxed);
                    self.bytes_commit.fetch_add(bytes_written, Ordering::Relaxed);
                    return Ok(());
                } else {
                    return Err(ExofsError::InvalidArgument);
                }
            }
        }
        Err(ExofsError::NotFound)
    }

    // ── Rollback ─────────────────────────────────────────────────────────────

    /// Annule une allocation (libère le bloc sans commit).
    pub fn rollback_extent(&self, offset: DiskOffset) -> ExofsResult<()> {
        let mut st = self.state.lock();
        let mut found_extent: Option<Extent> = None;

        for h in st.handles.iter_mut() {
            if h.extent.offset == offset && h.state == ExtentState::Reserved {
                found_extent = Some(h.extent);
                h.free_it();
                break;
            }
        }

        drop(st);

        if let Some(ext) = found_extent {
            self.heap.free_extent(ext)?;
            self.n_rollbacks.fetch_add(1, Ordering::Relaxed);
            STORAGE_STATS.inc_heap_free(ext.size);
            Ok(())
        } else {
            Err(ExofsError::NotFound)
        }
    }

    // ── Libération ────────────────────────────────────────────────────────────

    /// Libère un extent commité (lors de la suppression d'un blob/objet).
    pub fn free_committed(&self, offset: DiskOffset) -> ExofsResult<()> {
        let mut st = self.state.lock();
        let mut found: Option<Extent> = None;

        for h in st.handles.iter_mut() {
            if h.extent.offset == offset && h.state == ExtentState::Committed {
                found = Some(h.extent);
                h.free_it();
                break;
            }
        }

        // Nettoyage des handles freed.
        let freed = st.drain_freed();
        drop(st);

        for ext in freed {
            let _ = self.heap.free_extent(ext);
        }

        if found.is_some() {
            Ok(())
        } else {
            Err(ExofsError::NotFound)
        }
    }

    // ── Nettoyage périodique ──────────────────────────────────────────────────

    /// Nettoie les handles freed accumulés.
    pub fn gc_freed_handles(&self) -> ExofsResult<u64> {
        let freed = {
            let mut st = self.state.lock();
            st.drain_freed()
        };
        let count = freed.len() as u64;
        for ext in freed {
            let _ = self.heap.free_extent(ext);
            STORAGE_STATS.inc_heap_free(ext.size);
        }
        Ok(count)
    }

    // ── Requêtes ─────────────────────────────────────────────────────────────

    /// Octets libres dans le heap.
    #[inline]
    pub fn free_bytes(&self) -> u64 {
        self.heap.free_bytes()
    }

    /// Nombre total de blocs.
    #[inline]
    pub fn total_blocks(&self) -> u64 {
        self.heap.total_blocks()
    }

    pub fn n_allocs(&self)    -> u64 { self.n_allocs.load(Ordering::Relaxed) }
    pub fn n_commits(&self)   -> u64 { self.n_commits.load(Ordering::Relaxed) }
    pub fn n_rollbacks(&self) -> u64 { self.n_rollbacks.load(Ordering::Relaxed) }
    pub fn bytes_committed(&self) -> u64 { self.bytes_commit.load(Ordering::Relaxed) }

    /// Nombre d'handles actifs (Reserved ou Committed).
    pub fn active_handles(&self) -> usize {
        self.state.lock().handles.len()
    }

    /// Fragmentation du heap.
    pub fn fragmentation_pct(&self) -> u8 {
        self.heap.fragmentation_pct()
    }

    /// Usage du heap en pourcentage.
    pub fn usage_pct(&self) -> u64 {
        self.heap.usage_pct()
    }

    // ── Coalescence ───────────────────────────────────────────────────────────

    /// Force une passe de coalescence.
    pub fn coalesce(&self) -> ExofsResult<()> {
        self.heap.coalesce()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests unitaires
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const HEAP_START: u64 = 1024 * 1024;
    const N_BLOCKS:   u64 = 512;

    fn make() -> BlockAllocator {
        BlockAllocator::new(HEAP_START, N_BLOCKS).unwrap()
    }

    #[test]
    fn test_alloc_commit() {
        let a   = make();
        let ext = a.alloc_extent(4096).unwrap();
        a.commit_extent(ext.offset, 4096).unwrap();
        assert_eq!(a.n_commits(), 1);
    }

    #[test]
    fn test_alloc_rollback() {
        let a   = make();
        let ext = a.alloc_extent(4096).unwrap();
        a.rollback_extent(ext.offset).unwrap();
        assert_eq!(a.n_rollbacks(), 1);
    }

    #[test]
    fn test_commit_not_found() {
        let a = make();
        let r = a.commit_extent(DiskOffset(0), 4096);
        assert!(r.is_err());
    }

    #[test]
    fn test_free_committed() {
        let a   = make();
        let ext = a.alloc_extent(4096).unwrap();
        a.commit_extent(ext.offset, 4096).unwrap();
        a.free_committed(ext.offset).unwrap();
    }

    #[test]
    fn test_zero_size_fails() {
        let a = make();
        assert!(a.alloc_extent(0).is_err());
    }

    #[test]
    fn test_gc_freed() {
        let a   = make();
        let ext = a.alloc_extent(4096).unwrap();
        a.commit_extent(ext.offset, 4096).unwrap();
        a.free_committed(ext.offset).unwrap();
        let freed = a.gc_freed_handles().unwrap();
        assert_eq!(freed, 0); // déjà drainé lors du free
    }
}
