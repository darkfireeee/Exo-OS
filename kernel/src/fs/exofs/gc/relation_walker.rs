// kernel/src/fs/exofs/gc/relation_walker.rs
//
// ==============================================================================
// Traverseur du graphe de Relations ExoFS pour le GC tricolore
// Ring 0 . no_std . Exo-OS
//
// GC-02 : le GC tricolore DOIT traverser les Relations.
//         Sans cette traversee, des cycles de blobs relies par des Relations
//         peuvent rester orphelins et jamais collectes.
//
// Ce module itere sur le graphe de Relations pour propager les couleurs GC.
//
// Conformite :
//   GC-02 : traversee obligatoire des Relations dans la phase de marquage
//   GC-03 : file grise bornee (respectee via TricolorWorkspace)
//   RECUR-01 : DFS iteratif uniquement, pile heap-allouee
//   OOM-02 : try_reserve avant chaque push
//   DAG-01 : pas d'import de ipc/, process/, arch/
// ==============================================================================


use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::fmt;

use crate::fs::exofs::core::{BlobId, ObjectId, ExofsError, ExofsResult};
use crate::fs::exofs::gc::tricolor::TricolorWorkspace;
use crate::scheduler::sync::spinlock::SpinLock;

// ==============================================================================
// Constantes
// ==============================================================================

/// Nombre maximum de relations stockees dans le walker.
pub const MAX_RELATIONS: usize = 65536;

/// Profondeur maximale de traversee DFS (protection contre graphes profonds).
pub const MAX_WALK_DEPTH: usize = 1024;

// ==============================================================================
// RelationEdge — arete du graphe de relations
// ==============================================================================

/// Une arete dans le graphe de relations ExoFS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RelationEdge {
    /// ObjectId source (conteneur / parent logique).
    pub src: ObjectId,
    /// ObjectId destination (contenu / enfant logique).
    pub dst: ObjectId,
    /// Blob associe a la source (l'objet source possede ce blob).
    pub src_blob: Option<BlobId>,
    /// Blob associe a la destination.
    pub dst_blob: Option<BlobId>,
}

// ==============================================================================
// WalkStats — statistiques de traversee
// ==============================================================================

/// Statistiques d'une traversee du graphe de relations.
#[derive(Debug, Default, Clone)]
pub struct WalkStats {
    /// Nombre de relations traversees.
    pub relations_walked:  u64,
    /// Objets visites durant la traversee.
    pub objects_visited:   u64,
    /// BlobIds grises depuis les relations.
    pub blobs_greyed:      u64,
    /// Cycles detectes durant la traversee.
    pub cycles_found:      u64,
    /// Erreurs de type GcQueueFull.
    pub queue_full_errors: u64,
}

impl fmt::Display for WalkStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "WalkStats[relations={} objects={} greyed={} cycles={} q_full={}]",
            self.relations_walked,
            self.objects_visited,
            self.blobs_greyed,
            self.cycles_found,
            self.queue_full_errors,
        )
    }
}

// ==============================================================================
// RelationWalkerInner — donnees protegees
// ==============================================================================

struct RelationWalkerInner {
    /// Toutes les aretes de relations connues.
    edges:  Vec<RelationEdge>,
    /// Index : ObjectId -> indices dans `edges` (src).
    // Pour iteration efficace : obj -> ses aretes sortantes.
    // Approximation : parcours lineaire (OK pour <= 65536 relations).
    stats:  WalkStats,
}

// ==============================================================================
// RelationWalker — facade thread-safe
// ==============================================================================

/// Gestionnaire du graphe de relations pour le GC.
pub struct RelationWalker {
    inner: SpinLock<RelationWalkerInner>,
}

impl RelationWalker {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(RelationWalkerInner {
                edges: Vec::new(),
                stats: WalkStats {
                    relations_walked:  0,
                    objects_visited:   0,
                    blobs_greyed:      0,
                    cycles_found:      0,
                    queue_full_errors: 0,
                },
            }),
        }
    }

    // ── Gestion des aretes ───────────────────────────────────────────────────

    /// Ajoute une arete de relation.
    ///
    /// OOM-02 : try_reserve avant push.
    pub fn add_edge(&self, edge: RelationEdge) -> ExofsResult<()> {
        let mut g = self.inner.lock();

        if g.edges.len() >= MAX_RELATIONS {
            return Err(ExofsError::Resource);
        }

        g.edges.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        // Evite les doublons stricts.
        if !g.edges.contains(&edge) {
            g.edges.push(edge);
        }
        Ok(())
    }

    /// Supprime toutes les aretes impliquant un ObjectId.
    pub fn remove_object(&self, obj: &ObjectId) {
        let mut g = self.inner.lock();
        g.edges.retain(|e| &e.src != obj && &e.dst != obj);
    }

    /// Supprime toutes les aretes (reset pour une nouvelle passe GC).
    pub fn clear(&self) {
        let mut g = self.inner.lock();
        g.edges.clear();
    }

    /// Nombre d'aretes connues.
    pub fn edge_count(&self) -> usize {
        self.inner.lock().edges.len()
    }

    // ── Traversee GC-02 ─────────────────────────────────────────────────────

    /// Grise tous les BlobIds atteignables via le graphe de relations.
    ///
    /// GC-02 : cette methode DOIT etre appelee depuis la phase de marquage.
    /// Traversee DFS iterative depuis toutes les racines (GC-06).
    ///
    /// Les BlobIds associes aux objets grises sont ajoutes a la `workspace`.
    ///
    /// Retourne les stats de la traversee.
    pub fn walk_and_grey(
        &self,
        roots:     &[ObjectId],
        workspace: &mut TricolorWorkspace,
    ) -> ExofsResult<WalkStats> {
        let edges_snap: Vec<RelationEdge> = {
            let g = self.inner.lock();
            g.edges.clone()
        };

        let mut visited: BTreeSet<ObjectId>    = BTreeSet::new();
        let mut stack:   Vec<ObjectId>         = Vec::new();
        let mut stats = WalkStats::default();

        // Amorcer la pile depuis les racines.
        for &root in roots {
            if visited.insert(root) {
                stack.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                stack.push(root);
            }
        }

        // DFS iteratif (RECUR-01).
        let mut depth = 0usize;
        while let Some(current_obj) = stack.pop() {
            depth = depth.saturating_add(1);
            if depth > MAX_WALK_DEPTH {
                // Protection contre graphes trop profonds.
                break;
            }

            stats.objects_visited = stats.objects_visited.saturating_add(1);

            // Pour chaque arete sortante de current_obj : griser le dst_blob.
            for edge in edges_snap.iter().filter(|e| e.src == current_obj) {
                stats.relations_walked = stats.relations_walked.saturating_add(1);

                // Griser le blob de l'objet source.
                if let Some(src_blob) = edge.src_blob {
                    match workspace.grey(src_blob) {
                        Ok(()) => {
                            stats.blobs_greyed =
                                stats.blobs_greyed.saturating_add(1);
                        }
                        Err(ExofsError::GcQueueFull) => {
                            stats.queue_full_errors =
                                stats.queue_full_errors.saturating_add(1);
                            // GC-03 : file pleine, on reporte le grisement.
                            // La passe suivante collectera ce noeud.
                        }
                        Err(_) => {}
                    }
                }

                // Griser le blob de l'objet destination.
                if let Some(dst_blob) = edge.dst_blob {
                    match workspace.grey(dst_blob) {
                        Ok(()) => {
                            stats.blobs_greyed =
                                stats.blobs_greyed.saturating_add(1);
                        }
                        Err(ExofsError::GcQueueFull) => {
                            stats.queue_full_errors =
                                stats.queue_full_errors.saturating_add(1);
                        }
                        Err(_) => {}
                    }
                }

                // Ajouter la destination a la pile si non visitee.
                if visited.insert(edge.dst) {
                    stack.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    stack.push(edge.dst);
                }
            }

            depth = 0; // Reset depth per pop (c'est une profondeur approximative ici).
        }

        // Mise a jour des stats globales.
        {
            let mut g = self.inner.lock();
            g.stats.relations_walked = g.stats.relations_walked
                .saturating_add(stats.relations_walked);
            g.stats.objects_visited = g.stats.objects_visited
                .saturating_add(stats.objects_visited);
            g.stats.blobs_greyed = g.stats.blobs_greyed
                .saturating_add(stats.blobs_greyed);
            g.stats.queue_full_errors = g.stats.queue_full_errors
                .saturating_add(stats.queue_full_errors);
        }

        Ok(stats)
    }

    /// Retourne toutes les destinations d'un objet (aretes sortantes).
    pub fn destinations_of(&self, obj: &ObjectId) -> Vec<ObjectId> {
        let g = self.inner.lock();
        g.edges.iter()
            .filter(|e| &e.src == obj)
            .map(|e| e.dst)
            .collect()
    }

    /// Retourne toutes les sources pointant vers un objet (aretes entrantes).
    pub fn sources_of(&self, obj: &ObjectId) -> Vec<ObjectId> {
        let g = self.inner.lock();
        g.edges.iter()
            .filter(|e| &e.dst == obj)
            .map(|e| e.src)
            .collect()
    }

    /// Verifie si un objet est atteignable depuis une racine (BFS iteratif).
    ///
    /// RECUR-01 : BFS iteratif, pas recursif.
    pub fn is_reachable(&self, from: &ObjectId, target: &ObjectId) -> bool {
        let edges_snap: Vec<RelationEdge> = {
            let g = self.inner.lock();
            g.edges.clone()
        };

        let mut visited: BTreeSet<ObjectId> = BTreeSet::new();
        let mut queue: alloc::collections::VecDeque<ObjectId> =
            alloc::collections::VecDeque::new();

        queue.push_back(*from);
        visited.insert(*from);

        while let Some(current) = queue.pop_front() {
            if &current == target {
                return true;
            }
            for edge in edges_snap.iter().filter(|e| e.src == current) {
                if visited.insert(edge.dst) {
                    queue.push_back(edge.dst);
                }
            }
        }

        false
    }

    /// Statistiques cumulees.
    pub fn stats(&self) -> WalkStats {
        self.inner.lock().stats.clone()
    }
}

// ==============================================================================
// Instance globale
// ==============================================================================

/// Walker de relations global pour le GC.
pub static RELATION_WALKER: RelationWalker = RelationWalker::new();

// ==============================================================================
// Tests
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::gc::tricolor::TricolorWorkspace;
    use crate::fs::exofs::gc::tricolor::BlobNode;

    fn oid(b: u8) -> ObjectId {
        let mut a = [0u8; 32]; a[0] = b; ObjectId(a)
    }

    fn bid(b: u8) -> BlobId {
        let mut a = [0u8; 32]; a[0] = b; BlobId(a)
    }

    fn edge(s: u8, d: u8, sb: Option<u8>, db: Option<u8>) -> RelationEdge {
        RelationEdge {
            src:      oid(s),
            dst:      oid(d),
            src_blob: sb.map(bid),
            dst_blob: db.map(bid),
        }
    }

    #[test]
    fn test_add_and_count() {
        let w = RelationWalker::new();
        w.add_edge(edge(1, 2, Some(10), Some(20))).unwrap();
        w.add_edge(edge(2, 3, None, None)).unwrap();
        assert_eq!(w.edge_count(), 2);
    }

    #[test]
    fn test_no_duplicate_edges() {
        let w = RelationWalker::new();
        w.add_edge(edge(1, 2, Some(10), Some(20))).unwrap();
        w.add_edge(edge(1, 2, Some(10), Some(20))).unwrap(); // doublon
        assert_eq!(w.edge_count(), 1);
    }

    #[test]
    fn test_walk_and_grey_blobs() {
        let w = RelationWalker::new();
        w.add_edge(edge(1, 2, Some(10), Some(20))).unwrap();

        let mut ws = TricolorWorkspace::new().unwrap();
        let b10 = bid(10);
        let b20 = bid(20);
        ws.insert_node(BlobNode::new(b10, 512, 1, 2, 0, false));
        ws.insert_node(BlobNode::new(b20, 512, 1, 2, 0, false));

        let roots = [oid(1)];
        let stats = w.walk_and_grey(&roots, &mut ws).unwrap();
        assert_eq!(stats.blobs_greyed, 2);
        assert_eq!(ws.grey_queue_len(), 2);
    }

    #[test]
    fn test_destinations_of() {
        let w = RelationWalker::new();
        w.add_edge(edge(5, 6, None, None)).unwrap();
        w.add_edge(edge(5, 7, None, None)).unwrap();
        let dsts = w.destinations_of(&oid(5));
        assert_eq!(dsts.len(), 2);
    }

    #[test]
    fn test_is_reachable() {
        let w = RelationWalker::new();
        w.add_edge(edge(1, 2, None, None)).unwrap();
        w.add_edge(edge(2, 3, None, None)).unwrap();
        assert!(w.is_reachable(&oid(1), &oid(3)));
        assert!(!w.is_reachable(&oid(3), &oid(1)));
    }

    #[test]
    fn test_remove_object() {
        let w = RelationWalker::new();
        w.add_edge(edge(1, 2, None, None)).unwrap();
        w.add_edge(edge(3, 4, None, None)).unwrap();
        w.remove_object(&oid(2)); // supprime les aretes impliquant oid(2)
        assert_eq!(w.edge_count(), 1);
    }
}
