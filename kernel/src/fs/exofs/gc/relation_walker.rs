//! Parcours du graphe de relations pour le GC ExoFS.
//!
//! Les relations ExoFS (parent-enfant, hard-link, data-ref) forment un DAG.
//! Le RelationWalker traverse ce DAG depuis un blob racine pour alimenter
//! le phase de marquage.
//!
//! RÈGLE 6  : ce module n'importe pas scheduler/ ipc/ process/ directement.
//! RÈGLE 14 : checked_add pour les compteurs de profondeur.

use alloc::vec::Vec;

use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::gc::reference_tracker::REFERENCE_TRACKER;
use crate::fs::exofs::gc::tricolor::{BlobIndex, TricolorSet};

/// Résultat d'un parcours de relation.
#[derive(Debug, Default)]
pub struct WalkStats {
    pub nodes_visited: u64,
    pub max_depth: u32,
    pub edges_followed: u64,
}

/// Parcourt récursivement le DAG depuis les blobs racines.
pub struct RelationWalker {
    /// Profondeur max pour éviter les stack overflows (RÈGLE 10 : pas de récursion infinie).
    max_depth: u32,
}

impl RelationWalker {
    pub fn new(max_depth: u32) -> Self {
        Self { max_depth }
    }

    /// Parcours BFS depuis toutes les racines de l'index marquées grises.
    pub fn walk(
        &self,
        index: &BlobIndex,
        colors: &TricolorSet,
    ) -> Result<WalkStats, FsError> {
        let mut stats = WalkStats::default();
        let mut queue: Vec<(BlobId, u32)> = Vec::new(); // (blob_id, profondeur)

        // Amorçage depuis les racines grises.
        for i in 0..index.len() {
            if colors.get(i).needs_visit() {
                if let Some(id) = index.blob_at(i) {
                    queue.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                    queue.push((*id, 0));
                }
            }
        }

        while let Some((blob_id, depth)) = queue.pop() {
            stats.nodes_visited = stats
                .nodes_visited
                .checked_add(1)
                .ok_or(FsError::Overflow)?;

            if depth > stats.max_depth {
                stats.max_depth = depth;
            }

            if depth >= self.max_depth {
                continue; // Profondeur maximale atteinte.
            }

            let children = REFERENCE_TRACKER.get_refs(&blob_id);
            let next_depth = depth.checked_add(1).ok_or(FsError::Overflow)?;

            for child_id in children {
                stats.edges_followed = stats
                    .edges_followed
                    .checked_add(1)
                    .ok_or(FsError::Overflow)?;

                if let Some(child_idx) = index.index_of(&child_id) {
                    if colors.mark_grey(child_idx) {
                        queue.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                        queue.push((child_id, next_depth));
                    }
                }
            }
        }

        Ok(stats)
    }
}
