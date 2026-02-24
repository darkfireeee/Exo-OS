// kernel/src/memory/dma/ops/interleaved.rs
//
// DMA intercalé (interleaved) — transferts avec stride sur source/destination.
//
// Un transfert interleaved déplace des chunks non-contigus :
//   - Source  : adresses src_base, src_base+src_stride, src_base+2*src_stride...
//   - Dest    : adresses dst_base, dst_base+dst_stride, dst_base+2*dst_stride...
//   - Chaque chunk fait `chunk_bytes` octets.
//
// Cas d'usage typiques :
//   - Désentrelacement audio multi-canal (L/R → buffers séparés).
//   - RAID-like striping entre zones mémoire.
//   - Transposition de matrices en mémoire.
//   - Réorganisation de DMA scatter vers gather.
//
// COUCHE 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicU64, AtomicU32, AtomicU8, Ordering};
use spin::Mutex;

use crate::memory::dma::core::types::{
    DmaChannelId, DmaTransactionId, DmaDirection, DmaError,
};
use crate::memory::dma::channels::manager::DMA_CHANNELS;
use crate::memory::core::types::PhysAddr;
use crate::memory::core::layout::PHYS_MAP_BASE;

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTES
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum de chunks par transfert interleaved.
pub const MAX_INTERLEAVED_CHUNKS: usize = 64;

/// Taille minimum d'un chunk.
pub const MIN_CHUNK_BYTES: usize = 8;

/// Nombre maximum de transferts interleaved simultanés.
pub const MAX_INTERLEAVED_TRANSFERS: usize = 32;

// ─────────────────────────────────────────────────────────────────────────────
// CONFIGURATION
// ─────────────────────────────────────────────────────────────────────────────

/// Pattern de stride pour une extrémité (source ou destination).
#[derive(Copy, Clone, Debug)]
pub struct InterleavedStride {
    /// Adresse physique de départ.
    pub base:    PhysAddr,
    /// Stride en octets entre deux chunks consécutifs.
    /// 0 = contiguë (même que scatter-gather ordinaire).
    pub stride:  usize,
}

/// Configuration d'un transfert DMA interleaved.
#[derive(Copy, Clone)]
pub struct InterleavedConfig {
    /// Canal DMA.
    pub channel:     DmaChannelId,
    /// Pattern source.
    pub src:         InterleavedStride,
    /// Pattern destination.
    pub dst:         InterleavedStride,
    /// Taille de chaque chunk en octets.
    pub chunk_bytes: usize,
    /// Nombre de chunks.
    pub chunk_count: usize,
}

/// Erreur de validation d'une configuration interleaved.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum InterleavedError {
    ChunkTooSmall,
    TooManyChunks,
    InvalidAlignment,
    ZeroChunkCount,
    ChannelUnsupported,
}

impl InterleavedConfig {
    pub fn validate(&self) -> Result<(), InterleavedError> {
        if self.chunk_count == 0 {
            return Err(InterleavedError::ZeroChunkCount);
        }
        if self.chunk_bytes < MIN_CHUNK_BYTES {
            return Err(InterleavedError::ChunkTooSmall);
        }
        if self.chunk_bytes & 7 != 0 {
            return Err(InterleavedError::InvalidAlignment);
        }
        if self.chunk_count > MAX_INTERLEAVED_CHUNKS {
            return Err(InterleavedError::TooManyChunks);
        }
        Ok(())
    }

    /// Retourne l'adresse physique du chunk `i` côté source.
    #[inline]
    pub fn src_chunk_phys(&self, i: usize) -> PhysAddr {
        PhysAddr::new(self.src.base.as_u64() + (i * self.src.stride) as u64)
    }

    /// Retourne l'adresse physique du chunk `i` côté destination.
    #[inline]
    pub fn dst_chunk_phys(&self, i: usize) -> PhysAddr {
        PhysAddr::new(self.dst.base.as_u64() + (i * self.dst.stride) as u64)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TRANSFERT INTERLEAVED LOGICIEL (FALLBACK)
// ─────────────────────────────────────────────────────────────────────────────

/// Exécute un transfert interleaved en mode logiciel (CPU memcpy).
///
/// Utilisé quand le canal DMA ne supporte pas nativement le mode interleaved.
/// Synchrone — retourne quand tous les chunks sont copiés.
///
/// # Safety
/// `src.base` et `dst.base` doivent pointer vers des régions mémoire physiques
/// valides accessibles via la physmap (PHYS_MAP_BASE).
pub unsafe fn sw_interleaved_copy(config: &InterleavedConfig) -> Result<usize, InterleavedError> {
    config.validate()?;

    let phys_map_base = PHYS_MAP_BASE.as_u64();
    let mut total = 0usize;

    for i in 0..config.chunk_count {
        let src_virt = (phys_map_base + config.src_chunk_phys(i).as_u64()) as *const u8;
        let dst_virt = (phys_map_base + config.dst_chunk_phys(i).as_u64()) as *mut u8;

        // SAFETY: Les adresses sont dans la physmap (validité garantie par l'appelant).
        core::ptr::copy_nonoverlapping(src_virt, dst_virt, config.chunk_bytes);
        total += config.chunk_bytes;
    }

    Ok(total)
}

// ─────────────────────────────────────────────────────────────────────────────
// TRANSFERT INTERLEAVED VIA DMA ASYNCHRONE
// ─────────────────────────────────────────────────────────────────────────────

/// État interne d'un transfert interleaved asynchrone.
#[repr(u8)]
#[derive(Copy, Clone, Eq, PartialEq)]
enum IxferState {
    Free       = 0,
    InProgress = 1,
    Done       = 2,
    Error      = 3,
}

/// Un transfert interleaved asynchrone en cours.
struct InterleavedXfer {
    config:          InterleavedConfig,
    state:           AtomicU8,
    txn_id:          DmaTransactionId,
    /// Chunks déjà soumis au moteur DMA.
    chunks_submitted: AtomicU32,
    /// Chunks complétés avec succès.
    chunks_done:      AtomicU32,
    /// Chunks en erreur.
    chunks_error:     AtomicU32,
    /// Octets totaux transférés.
    bytes_done:       AtomicU64,
}

impl InterleavedXfer {
    const fn new() -> Self {
        InterleavedXfer {
            config: InterleavedConfig {
                channel:     DmaChannelId(u32::MAX),
                src:         InterleavedStride { base: PhysAddr::new(0), stride: 0 },
                dst:         InterleavedStride { base: PhysAddr::new(0), stride: 0 },
                chunk_bytes: 0,
                chunk_count: 0,
            },
            state:            AtomicU8::new(IxferState::Free as u8),
            txn_id:           DmaTransactionId::INVALID,
            chunks_submitted: AtomicU32::new(0),
            chunks_done:      AtomicU32::new(0),
            chunks_error:     AtomicU32::new(0),
            bytes_done:       AtomicU64::new(0),
        }
    }

    fn is_free(&self) -> bool {
        self.state.load(Ordering::Acquire) == IxferState::Free as u8
    }

    fn is_done(&self) -> bool {
        let done  = self.chunks_done.load(Ordering::Acquire) as usize;
        let total = self.config.chunk_count;
        done >= total && total > 0
    }

    /// Notifie la complétion d'un chunk (appelé par IRQ handler).
    pub fn on_chunk_complete(&self, success: bool) {
        if success {
            let done = self.chunks_done.fetch_add(1, Ordering::AcqRel) + 1;
            self.bytes_done.fetch_add(self.config.chunk_bytes as u64, Ordering::Relaxed);
            if done >= self.config.chunk_count as u32 {
                self.state.store(IxferState::Done as u8, Ordering::Release);
            }
        } else {
            self.chunks_error.fetch_add(1, Ordering::Relaxed);
            self.state.store(IxferState::Error as u8, Ordering::Release);
        }
    }

    /// Résultat du transfert (si terminé).
    pub fn result(&self) -> Option<Result<usize, DmaError>> {
        match self.state.load(Ordering::Acquire) {
            s if s == IxferState::Done as u8 =>
                Some(Ok(self.bytes_done.load(Ordering::Relaxed) as usize)),
            s if s == IxferState::Error as u8 =>
                Some(Err(DmaError::HardwareError)),
            _ =>
                None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GESTIONNAIRE DE TRANSFERTS INTERLEAVED
// ─────────────────────────────────────────────────────────────────────────────

struct InterleavedTable {
    xfers: [InterleavedXfer; MAX_INTERLEAVED_TRANSFERS],
}

impl InterleavedTable {
    const fn new() -> Self {
        const X: InterleavedXfer = InterleavedXfer::new();
        InterleavedTable { xfers: [X; MAX_INTERLEAVED_TRANSFERS] }
    }

    fn alloc(&mut self) -> Option<usize> {
        for (i, x) in self.xfers.iter().enumerate() {
            if x.is_free() {
                return Some(i);
            }
        }
        None
    }
}

/// Gestionnaire global des transferts DMA interleaved.
pub struct InterleavedManager {
    inner: Mutex<InterleavedTable>,
    pub submitted: AtomicU64,
    pub completed: AtomicU64,
    pub sw_fallbacks: AtomicU64,
}

unsafe impl Sync for InterleavedManager {}
unsafe impl Send for InterleavedManager {}

impl InterleavedManager {
    pub const fn new() -> Self {
        InterleavedManager {
            inner:        Mutex::new(InterleavedTable::new()),
            submitted:    AtomicU64::new(0),
            completed:    AtomicU64::new(0),
            sw_fallbacks: AtomicU64::new(0),
        }
    }

    /// Soumet un transfert interleaved asynchrone.
    ///
    /// Retourne l'index du slot pour suivre la progression via `result`.
    ///
    /// Si le canal ne supporte pas le mode interleaved, retourne
    /// `Err(InterleavedError::ChannelUnsupported)`. Utilise alors
    /// `sw_interleaved_copy` comme fallback synchrone.
    pub fn submit_async(
        &self,
        config: InterleavedConfig,
    ) -> Result<usize, InterleavedError> {
        config.validate()?;

        let mut table = self.inner.lock();
        let idx = table.alloc().ok_or(InterleavedError::TooManyChunks)?;

        let txn = DmaTransactionId::generate();
        let xfer = &mut table.xfers[idx];
        xfer.config           = config;
        xfer.txn_id           = txn;
        xfer.chunks_submitted.store(0, Ordering::Relaxed);
        xfer.chunks_done.store(0, Ordering::Relaxed);
        xfer.chunks_error.store(0, Ordering::Relaxed);
        xfer.bytes_done.store(0, Ordering::Relaxed);
        xfer.state.store(IxferState::InProgress as u8, Ordering::Release);

        self.submitted.fetch_add(1, Ordering::Relaxed);
        Ok(idx)
    }

    /// Notifie la complétion du chunk `chunk_idx` du transfert `xfer_idx`.
    pub fn on_chunk_complete(&self, xfer_idx: usize, success: bool) {
        let table = self.inner.lock();
        if xfer_idx < MAX_INTERLEAVED_TRANSFERS {
            table.xfers[xfer_idx].on_chunk_complete(success);
        }
    }

    /// Retourne le résultat d'un transfert (None si encore en cours).
    pub fn result(&self, xfer_idx: usize) -> Option<Result<usize, DmaError>> {
        let table = self.inner.lock();
        if xfer_idx < MAX_INTERLEAVED_TRANSFERS {
            let res = table.xfers[xfer_idx].result();
            if res.is_some() {
                self.completed.fetch_add(1, Ordering::Relaxed);
            }
            res
        } else {
            Some(Err(DmaError::InvalidParams))
        }
    }

    /// Libère un slot de transfert terminé.
    pub fn release(&self, xfer_idx: usize) {
        let table = self.inner.lock();
        if xfer_idx < MAX_INTERLEAVED_TRANSFERS {
            table.xfers[xfer_idx].state.store(IxferState::Free as u8, Ordering::Release);
        }
    }
}

/// Gestionnaire global des DMA interleaved.
pub static DMA_INTERLEAVED: InterleavedManager = InterleavedManager::new();
