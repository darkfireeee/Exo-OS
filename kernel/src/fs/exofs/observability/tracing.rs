//! tracing.rs — Traçage des événements ExoFS (no_std).

use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::time::read_ticks;
use crate::fs::exofs::core::{BlobId, EpochId};

const TRACE_RING_SIZE: usize = 4096;

pub static EXOFS_TRACER: ExofsTracer = ExofsTracer::new_const();

/// Type d'événement tracé.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceKind {
    BlobRead       = 0,
    BlobWrite      = 1,
    BlobDelete     = 2,
    EpochCommit    = 3,
    GcRun          = 4,
    CacheHit       = 5,
    CacheMiss      = 6,
    SnapshotCreate = 7,
    SnapshotDelete = 8,
    RelationAdd    = 9,
    RelationRemove = 10,
    ExportStart    = 11,
    ImportStart    = 12,
    FsckStart      = 13,
    FsckResult     = 14,
    QuotaDenied    = 15,
}

/// Entrée de trace stockée dans l'anneau.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TraceEvent {
    pub tick:    u64,
    pub blob_id: [u8; 32],   // Rempli si pertinent, zéro sinon.
    pub epoch:   u64,
    pub kind:    TraceKind,
    pub _pad:    [u8; 7],
}

const _: () = assert!(core::mem::size_of::<TraceEvent>() == 56);

pub struct ExofsTracer {
    ring:  [core::cell::UnsafeCell<TraceEvent>; TRACE_RING_SIZE],
    head:  AtomicU64,
    total: AtomicU64,
}

// SAFETY: accès concurrent géré par fetch_add atomique.
unsafe impl Sync for ExofsTracer {}

impl ExofsTracer {
    pub const fn new_const() -> Self {
        const ZERO: core::cell::UnsafeCell<TraceEvent> = core::cell::UnsafeCell::new(TraceEvent {
            tick: 0, blob_id: [0;32], epoch: 0, kind: TraceKind::BlobRead, _pad: [0;7],
        });
        Self { ring: [ZERO; TRACE_RING_SIZE], head: AtomicU64::new(0), total: AtomicU64::new(0) }
    }

    pub fn record(&self, kind: TraceKind, blob_id: Option<BlobId>, epoch: EpochId) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize % TRACE_RING_SIZE;
        let entry = TraceEvent {
            tick:    read_ticks(),
            blob_id: blob_id.map(|b| b.as_bytes()).unwrap_or([0u8; 32]),
            epoch:   epoch.0,
            kind,
            _pad:    [0; 7],
        };
        // SAFETY: index tournant atomique garantit l'exclusivité de slot.
        unsafe { *self.ring[idx].get() = entry; }
        self.total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn total_events(&self) -> u64 { self.total.load(Ordering::Relaxed) }

    /// Retourne les N derniers événements (au plus TRACE_RING_SIZE).
    pub fn last_n<const N: usize>(&self) -> [TraceEvent; N] {
        let head = self.head.load(Ordering::Relaxed) as usize;
        let mut out = [TraceEvent {
            tick: 0, blob_id: [0;32], epoch: 0, kind: TraceKind::BlobRead, _pad: [0;7],
        }; N];
        for (i, slot) in out.iter_mut().enumerate() {
            let idx = (head + TRACE_RING_SIZE - N + i) % TRACE_RING_SIZE;
            // SAFETY: lecture sans lock acceptable pour diagnostic.
            *slot = unsafe { *self.ring[idx].get() };
        }
        out
    }
}
