//! Détecteur de cycles dans le graphe de relations ExoFS.
//!
//! ExoFS utilise un DAG conceptuel, mais des bugs ou corruptions peuvent
//! introduire des cycles. Ce module les détecte via DFS + coloration.
//!
//! Résultat : liste des ensembles de blobs formant un cycle (pour rapport FSCK).
//! RÈGLE 14 : checked_add pour tous les compteurs.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::gc::reference_tracker::REFERENCE_TRACKER;

/// Couleur DFS.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DfsColor {
    White, // Non visité.
    Grey,  // En cours de visite (dans la pile).
    Black, // Visité complètement.
}

/// Un cycle détecté : ensemble des BlobIds participants.
#[derive(Debug)]
pub struct DetectedCycle {
    pub members: Vec<BlobId>,
}

/// Résultat de la détection de cycles.
#[derive(Debug, Default)]
pub struct CycleReport {
    pub cycles: Vec<DetectedCycle>,
    pub nodes_visited: u64,
}

/// Détecteur de cycles via DFS itératif sur le graphe de références.
pub struct CycleDetector;

impl CycleDetector {
    /// Lance la détection sur un ensemble de racines.
    pub fn detect(roots: &[BlobId]) -> Result<CycleReport, FsError> {
        let mut colors: BTreeMap<BlobId, DfsColor> = BTreeMap::new();
        let mut report = CycleReport::default();
        // Encode la pile : (blob_id, index_dans_children)
        let mut stack: Vec<(BlobId, Vec<BlobId>, usize)> = Vec::new();

        for root in roots {
            if colors.get(root).copied() == Some(DfsColor::Black) {
                continue;
            }

            // Amorçage DFS.
            let children = REFERENCE_TRACKER.get_refs(root);
            stack.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
            stack.push((*root, children, 0));
            colors.insert(*root, DfsColor::Grey);

            while let Some((node, children, idx)) = stack.last_mut() {
                if *idx < children.len() {
                    let child = children[*idx];
                    *idx = idx.checked_add(1).ok_or(FsError::Overflow)?;

                    match colors.get(&child).copied() {
                        Some(DfsColor::Grey) => {
                            // Cycle détecté !
                            let cycle_members: Vec<BlobId> = stack
                                .iter()
                                .map(|(id, _, _)| *id)
                                .chain(core::iter::once(child))
                                .collect();
                            let mut members = Vec::new();
                            members
                                .try_reserve(cycle_members.len())
                                .map_err(|_| FsError::OutOfMemory)?;
                            members.extend_from_slice(&cycle_members);
                            report.cycles.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                            report.cycles.push(DetectedCycle { members });
                        }
                        None | Some(DfsColor::White) => {
                            colors.insert(child, DfsColor::Grey);
                            let grand_children = REFERENCE_TRACKER.get_refs(&child);
                            stack.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                            stack.push((child, grand_children, 0));
                            report.nodes_visited = report
                                .nodes_visited
                                .checked_add(1)
                                .ok_or(FsError::Overflow)?;
                        }
                        Some(DfsColor::Black) => {} // Déjà traité, safe.
                    }
                } else {
                    // Tous les enfants traités → noir.
                    let done_node = *node;
                    stack.pop();
                    colors.insert(done_node, DfsColor::Black);
                }
            }
        }

        Ok(report)
    }
}

use alloc::collections::BTreeMap;
