//! Collecteur d'objets orphelins ExoFS.
//!
//! Un objet est orphelin si aucun EpochRecord ne le référence mais qu'il
//! existe encore dans le BlobStore (fichier créé mais jamais commité, crash, etc.)
//!
//! RÈGLE 13 : n'acquiert pas EPOCH_COMMIT_LOCK.
//! RÈGLE 14 : checked_add pour les compteurs.

use crate::fs::exofs::core::FsError;
use crate::fs::exofs::gc::blob_refcount::BLOB_REFCOUNT;
use crate::fs::exofs::gc::sweeper::DEFERRED_DELETE;
use crate::fs::exofs::storage::BlobStore;

/// Collecte les blobs présents dans le BlobStore mais non référencés
/// dans aucun epoch (orphelins permanents).
pub struct OrphanCollector;

impl OrphanCollector {
    /// Itère sur tous les blobs du store avec ref_count == 0 et les enfile
    /// dans DEFERRED_DELETE.
    ///
    /// Retourne le nombre d'orphelins détectés.
    pub fn collect(store: &BlobStore) -> Result<u64, FsError> {
        let mut count: u64 = 0;

        // La table de référence est la source de vérité.
        BLOB_REFCOUNT.collect_zero_refs(|blob_id, _phys_size| {
            // Vérifie que le blob existe vraiment dans le store
            // (évite les fantômes dans la table de ref).
            if store.blob_exists(&blob_id) {
                if DEFERRED_DELETE.enqueue(blob_id).is_ok() {
                    count = count.wrapping_add(1);
                }
            }
        });

        Ok(count)
    }

    /// Vérifie si un objet spécifique est orphelin.
    pub fn is_orphan(blob_id: &crate::fs::exofs::core::BlobId) -> bool {
        matches!(BLOB_REFCOUNT.get(blob_id), Some(0) | None)
    }
}
