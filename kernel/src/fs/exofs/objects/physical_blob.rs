// kernel/src/fs/exofs/objects/physical_blob.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// PhysicalBlob — représentation d'un P-Blob ExoFS
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Un P-Blob (Physical Blob) est le contenu immutable d'un objet Blob.
// Plusieurs LogicalObjects peuvent partager le même P-Blob (déduplication).
//
// RÈGLE REFCNT-01 : compare_exchange + panic si underflow.
// RÈGLE ONDISK-03 : AtomicU32 INTERDIT dans les structs on-disk.

use core::sync::atomic::{AtomicU32, Ordering};
use alloc::sync::Arc;

use crate::fs::exofs::core::{BlobId, DiskOffset, EpochId};
use crate::fs::exofs::core::stats::EXOFS_STATS;

// ─────────────────────────────────────────────────────────────────────────────
// PhysicalBlobDisk — 80 octets, on-disk (types plain)
// ─────────────────────────────────────────────────────────────────────────────

/// Structure on-disk d'un P-Blob (entrée dans la table de blobs).
#[derive(Copy, Clone)]
#[repr(C, packed)]
pub struct PhysicalBlobDisk {
    /// BlobId (32 octets).
    pub blob_id:       [u8; 32],
    /// Offset disque du contenu du blob.
    pub data_offset:   u64,
    /// Taille des données en octets.
    pub data_size:     u64,
    /// Référence count au dernier commit (plain u32 — RÈGLE ONDISK-03).
    pub ref_count:     u32,
    /// Flags de compression (0 = pas de compression).
    pub compress_type: u8,
    /// _pad.
    pub _pad:          [u8; 3],
    /// Epoch création.
    pub epoch_create:  u64,
    /// Checksum.
    pub checksum:      [u8; 32],
}

const _: () = assert!(
    core::mem::size_of::<PhysicalBlobDisk>() == 120,
    "PhysicalBlobDisk doit être exactement 120 octets"
);

// ─────────────────────────────────────────────────────────────────────────────
// PhysicalBlobInMemory — version RAM avec AtomicU32 ref_count
// ─────────────────────────────────────────────────────────────────────────────

/// P-Blob in-memory.
pub struct PhysicalBlobInMemory {
    pub blob_id:       BlobId,
    pub data_offset:   DiskOffset,
    pub data_size:     u64,
    /// Compteur de références atomique (RÈGLE REFCNT-01).
    pub ref_count:     AtomicU32,
    pub compress_type: u8,
    pub epoch_create:  EpochId,
}

impl PhysicalBlobInMemory {
    /// Construit depuis le on-disk.
    pub fn from_disk(d: &PhysicalBlobDisk) -> Self {
        Self {
            blob_id:       BlobId({ d.blob_id }),
            data_offset:   DiskOffset({ d.data_offset }),
            data_size:     { d.data_size },
            ref_count:     AtomicU32::new({ d.ref_count }),
            compress_type: { d.compress_type },
            epoch_create:  EpochId({ d.epoch_create }),
        }
    }

    /// Incrémente le compteur de références.
    #[inline]
    pub fn inc_ref(&self) {
        self.ref_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Décrémente le compteur de références.
    ///
    /// RÈGLE REFCNT-01 : panic sur underflow.
    #[inline]
    pub fn dec_ref(&self) -> u32 {
        loop {
            let cur = self.ref_count.load(Ordering::Acquire);
            if cur == 0 {
                panic!("ExoFS: PhysicalBlob ref_count underflow pour BlobId {:?}", self.blob_id.0);
            }
            let prev = cur;
            match self.ref_count.compare_exchange_weak(
                cur, cur - 1, Ordering::AcqRel, Ordering::Relaxed,
            ) {
                Ok(_) => {
                    if prev == 1 {
                        // Dernier référençant — le GC pourra collecter ce blob.
                        EXOFS_STATS.inc_blobs_gc_collected();
                    }
                    return prev - 1;
                }
                Err(_) => continue,
            }
        }
    }

    /// Vrai si le blob n'est plus référencé (candidat GC).
    #[inline]
    pub fn is_orphan(&self) -> bool {
        self.ref_count.load(Ordering::Acquire) == 0
    }
}

/// Type de référence partagée à un P-Blob.
pub type PhysicalBlobRef = Arc<PhysicalBlobInMemory>;
