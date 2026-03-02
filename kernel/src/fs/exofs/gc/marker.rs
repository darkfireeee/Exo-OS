//! Marqueur GC — propagation gris→noir dans le graphe de blobs.
//!
//! Itère sur l'ensemble gris, visite les enfants (références de chaque blob),
//! et les passe du blanc au gris puis du gris au noir.
//!
//! RÈGLE 13 : n'acquiert jamais EPOCH_COMMIT_LOCK.
//! RÈGLE 2  : try_reserve avant tout push.

use alloc::vec::Vec;

use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::gc::tricolor::{BlobIndex, TricolorSet};
use crate::fs::exofs::storage::BlobStore;

/// Résultat d'une passe de marquage.
#[derive(Debug, Default)]
pub struct MarkStats {
    /// Nombre de blobs passés gris→noir.
    pub marked_black: u64,
    /// Nombre d'arêtes parcourues.
    pub edges_traversed: u64,
    /// Nombre d'itérations de la boucle fixpoint.
    pub iterations: u64,
}

/// Marqueur tricolore.
pub struct Marker<'store> {
    store: &'store BlobStore,
}

impl<'store> Marker<'store> {
    pub fn new(store: &'store BlobStore) -> Self {
        Self { store }
    }

    /// Exécute la propagation complète jusqu'à ce que l'ensemble gris soit vide.
    pub fn run(
        &self,
        index: &BlobIndex,
        colors: &TricolorSet,
    ) -> Result<MarkStats, FsError> {
        let mut stats = MarkStats::default();
        let mut grey_queue: Vec<usize> = Vec::new();

        // Initialisation : met tous les gris dans la queue.
        for i in 0..index.len() {
            if colors.get(i).needs_visit() {
                grey_queue
                    .try_reserve(1)
                    .map_err(|_| FsError::OutOfMemory)?;
                grey_queue.push(i);
            }
        }

        while !grey_queue.is_empty() {
            stats.iterations += 1;
            let mut next_queue: Vec<usize> = Vec::new();

            for grey_idx in grey_queue.drain(..) {
                let blob_id = match index.blob_at(grey_idx) {
                    Some(id) => *id,
                    None => continue,
                };

                // Récupère les références sortantes de ce blob.
                let children = self.store.get_blob_refs(&blob_id)?;
                stats.edges_traversed = stats
                    .edges_traversed
                    .checked_add(children.len() as u64)
                    .ok_or(FsError::Overflow)?;

                for child_id in &children {
                    if let Some(child_idx) = index.index_of(child_id) {
                        if colors.mark_grey(child_idx) {
                            // Était blanc, maintenant gris → à visiter.
                            next_queue
                                .try_reserve(1)
                                .map_err(|_| FsError::OutOfMemory)?;
                            next_queue.push(child_idx);
                        }
                    }
                }

                // Passe ce nœud de gris à noir.
                if colors.mark_black(grey_idx) {
                    stats.marked_black = stats
                        .marked_black
                        .checked_add(1)
                        .ok_or(FsError::Overflow)?;
                }
            }

            grey_queue = next_queue;
        }

        Ok(stats)
    }
}
