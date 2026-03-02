//! GC inline — déclenché sur le chemin d'écriture lors d'un drop de référence.
//!
//! Quand le ref_count d'un P-Blob tombe à 0 pendant un write, on peut
//! l'ajouter immédiatement à la DeferredDeleteQueue sans attendre le thread GC.
//!
//! RÈGLE 12 : panic si underflow détecté.
//! RÈGLE 13 : n'acquiert jamais EPOCH_COMMIT_LOCK.

use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::gc::blob_refcount::BLOB_REFCOUNT;
use crate::fs::exofs::gc::sweeper::DEFERRED_DELETE;

/// Résultat d'un décrément inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineGcResult {
    /// Compteur décrémenté, blob toujours vivant (count > 0).
    Alive(u32),
    /// Compteur tombé à 0, blob enfilé dans DEFERRED_DELETE.
    Deferred,
    /// Blob non trouvé dans la table de référence.
    NotTracked,
}

/// GC inline : décrémente le ref_count et enfile si zéro.
pub struct InlineGc;

impl InlineGc {
    /// Décrémente le ref_count d'un P-Blob.
    ///
    /// Si le compteur tombe à 0, enfile dans DEFERRED_DELETE.
    /// RÈGLE 12 : si le compteur est déjà 0 avant décrémentation → PANIC.
    pub fn dec_ref(blob_id: &BlobId) -> Result<InlineGcResult, FsError> {
        match BLOB_REFCOUNT.get(blob_id) {
            None => Ok(InlineGcResult::NotTracked),
            Some(0) => {
                // RÈGLE 12 : jamais fetch_sub aveugle → panic.
                panic!(
                    "[ExoFS InlineGc] tentative de décrément sur BlobId {:?} avec count=0",
                    blob_id
                );
            }
            Some(_) => {
                let (new_count, _phys_size) = BLOB_REFCOUNT.dec(blob_id)?;
                if new_count == 0 {
                    DEFERRED_DELETE.enqueue(*blob_id)?;
                    Ok(InlineGcResult::Deferred)
                } else {
                    Ok(InlineGcResult::Alive(new_count))
                }
            }
        }
    }

    /// Incrémente le ref_count (ex. : lors d'un clone de référence).
    pub fn inc_ref(blob_id: &BlobId) -> Result<u32, FsError> {
        BLOB_REFCOUNT.inc(blob_id)
    }

    /// Enregistre un nouveau blob avec ref_count = 1.
    pub fn register_new(blob_id: BlobId, phys_size: u64) -> Result<(), FsError> {
        BLOB_REFCOUNT.register(blob_id, 1, phys_size)
    }
}
