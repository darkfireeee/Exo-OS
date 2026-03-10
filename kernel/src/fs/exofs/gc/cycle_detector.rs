// kernel/src/fs/exofs/gc/cycle_detector.rs
//
// ==============================================================================
// Détecteur de Cycles dans le Graphe de Blobs ExoFS
// Ring 0 . no_std . Exo-OS
//
// Ce module implémente la détection de cycles dans le graphe de références
// de blobs (blob -> sous-blobs), en utilisant un DFS iteratif coloré.
//
// Pour chaque sobblob, on explore ses enfants. Une arête de retour vers un
// noeud en cours de visite (DFS_GREY) indique un cycle.
//
// Conformite :
//   GC-02 : les cycles de blobs reliés par des Relations sont détectés
//   RECUR-01 : DFS iteratif uniquement, pile heap-allouee
//   OOM-02 : try_reserve avant chaque push
//   ARITH-02 : checked_add / saturating_*
//   DAG-01 : pas d'import de ipc/, process/, arch/
// ==============================================================================


use alloc::collections::BTreeMap;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::fmt;

use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};
use crate::fs::exofs::gc::reference_tracker::REFERENCE_TRACKER;
use crate::scheduler::sync::spinlock::SpinLock;

// ==============================================================================
// Constantes
// ==============================================================================

/// Limite maximale de noeuds dans un cycle detecte.
pub const MAX_CYCLE_MEMBERS: usize = 4096;

/// Limite de cycles a retourner par passe.
pub const MAX_CYCLES_PER_PASS: usize = 256;

/// Profondeur maximale de la pile DFS.
pub const MAX_DFS_STACK_DEPTH: usize = 65536;

// ==============================================================================
// DfsColor — couleur DFS (différente de TriColor)
// ==============================================================================

/// Couleur utilisée pour le DFS de detection de cycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DfsColor {
    /// Non visite.
    White,
    /// En cours de visite (sur la pile DFS).
    Grey,
    /// Visite complet.
    Black,
}

// ==============================================================================
// DetectedCycle — un cycle détecté
// ==============================================================================

/// Un cycle détecté dans le graphe de blobs.
#[derive(Debug, Clone)]
pub struct DetectedCycle {
    /// Membres du cycle (dans l'ordre de détection).
    pub members: Vec<BlobId>,
    /// Noeud d'entrée du cycle (le blob qui ferme la boucle).
    pub entry:   BlobId,
    /// Taille totale en octets des blobs dans le cycle
    /// (approximation — somme des tailles si connues).
    pub total_size_hint: u64,
}

impl DetectedCycle {
    /// Longueur du cycle.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Contient un blob donné ?
    pub fn contains(&self, blob_id: &BlobId) -> bool {
        self.members.contains(blob_id)
    }
}

impl fmt::Display for DetectedCycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Cycle[entry={:?} len={} total_hint={}]",
            self.entry,
            self.members.len(),
            self.total_size_hint,
        )
    }
}

// ==============================================================================
// CycleDetectStats — statistiques
// ==============================================================================

/// Statistiques de détection de cycles.
#[derive(Debug, Default, Clone)]
pub struct CycleDetectStats {
    /// Noeuds visités.
    pub nodes_visited:     u64,
    /// Arêtes traversées.
    pub edges_traversed:   u64,
    /// Cycles détectés.
    pub cycles_found:      u64,
    /// Total de membres de cycles.
    pub total_cycle_nodes: u64,
    /// Passes de détection.
    pub passes:            u64,
    /// Erreurs OOM (try_reserve).
    pub oom_errors:        u64,
    /// Noeuds depasses (profondeur stack max).
    pub stack_overflows:   u64,
}

impl fmt::Display for CycleDetectStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CycleStats[visited={} edges={} cycles={} nodes_in_cycles={} passes={}]",
            self.nodes_visited,
            self.edges_traversed,
            self.cycles_found,
            self.total_cycle_nodes,
            self.passes,
        )
    }
}

// ==============================================================================
// DfsFrame — element de la pile DFS
// ==============================================================================

/// Entree sur la pile DFS iterative.
#[derive(Debug, Clone)]
struct DfsFrame {
    /// BlobId en cours.
    blob_id:       BlobId,
    /// Index dans la liste des enfants (pour reentrée).
    child_index:   usize,
}

// ==============================================================================
// CycleDetectorInner — état interne
// ==============================================================================

struct CycleDetectorInner {
    total_stats:   CycleDetectStats,
    pass_count:    u64,
    /// Derniers cycles détectés.
    last_cycles:   Vec<DetectedCycle>,
}

// ==============================================================================
// CycleDetector — facade thread-safe
// ==============================================================================

/// Détecteur de cycles dans le graphe de blobs.
pub struct CycleDetector {
    inner: SpinLock<CycleDetectorInner>,
}

impl CycleDetector {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(CycleDetectorInner {
                total_stats: CycleDetectStats {
                    nodes_visited:     0,
                    edges_traversed:   0,
                    cycles_found:      0,
                    total_cycle_nodes: 0,
                    passes:            0,
                    oom_errors:        0,
                    stack_overflows:   0,
                },
                pass_count:  0,
                last_cycles: Vec::new(),
            }),
        }
    }

    // ── Détection principale ─────────────────────────────────────────────────

    /// Detecte les cycles dans le graphe de blobs fourni en parametre.
    ///
    /// RECUR-01 : DFS iteratif avec pile heap-allouee.
    ///
    /// # Arguments
    /// - `blob_ids` : ensemble de tous les BlobIds connus.
    ///
    /// # Returns
    /// Vecteur de cycles détectés (au max `MAX_CYCLES_PER_PASS`).
    pub fn detect_cycles(
        &self,
        blob_ids: &[BlobId],
    ) -> ExofsResult<Vec<DetectedCycle>> {
        let mut colors:  BTreeMap<BlobId, DfsColor> = BTreeMap::new();
        let mut parent:  BTreeMap<BlobId, BlobId>   = BTreeMap::new();
        let mut cycles:  Vec<DetectedCycle>          = Vec::new();
        let mut stats = CycleDetectStats::default();

        // Initialiser toutes les couleurs a Blanc.
        for &bid in blob_ids {
            colors.insert(bid, DfsColor::White);
        }

        // Lancer un DFS depuis chaque noeud non visite.
        // RECUR-01 : iteration sur la liste des noeuds.
        for &start in blob_ids {
            if cycles.len() >= MAX_CYCLES_PER_PASS {
                break;
            }

            if colors.get(&start) == Some(&DfsColor::Black) {
                continue;
            }

            // DFS iteratif depuis `start`.
            self.dfs_iterative(
                start,
                &mut colors,
                &mut parent,
                &mut cycles,
                &mut stats,
            )?;
        }

        stats.passes = 1;

        // Mise a jour des stats internes.
        {
            let mut g = self.inner.lock();
            g.pass_count = g.pass_count.saturating_add(1);
            let t = &mut g.total_stats;
            t.nodes_visited = t.nodes_visited
                .saturating_add(stats.nodes_visited);
            t.edges_traversed = t.edges_traversed
                .saturating_add(stats.edges_traversed);
            t.cycles_found = t.cycles_found
                .saturating_add(cycles.len() as u64);
            t.total_cycle_nodes = t.total_cycle_nodes
                .saturating_add(
                    cycles.iter().map(|c| c.len() as u64).sum::<u64>()
                );
            t.passes = t.passes.saturating_add(1);
            t.oom_errors = t.oom_errors.saturating_add(stats.oom_errors);
            t.stack_overflows = t.stack_overflows.saturating_add(stats.stack_overflows);
            g.last_cycles = cycles.clone();
        }

        Ok(cycles)
    }

    /// DFS iteratif depuis un noeud source.
    ///
    /// Utilise une pile explicite (RECUR-01) — jamais recursive.
    fn dfs_iterative(
        &self,
        start:   BlobId,
        colors:  &mut BTreeMap<BlobId, DfsColor>,
        parent:  &mut BTreeMap<BlobId, BlobId>,
        cycles:  &mut Vec<DetectedCycle>,
        stats:   &mut CycleDetectStats,
    ) -> ExofsResult<()> {
        // Pile DFS.
        let mut stack: Vec<DfsFrame> = Vec::new();
        stack.try_reserve(64).map_err(|_| ExofsError::NoMemory)?;

        // Precharger les enfants de `start`.
        let children_start = REFERENCE_TRACKER.get_refs(&start);

        // Griser le noeud de depart.
        colors.insert(start, DfsColor::Grey);
        stats.nodes_visited = stats.nodes_visited.saturating_add(1);

        stack.push(DfsFrame { blob_id: start, child_index: 0 });

        // RECUR-01 : boucle iterative.
        while !stack.is_empty() {
            if stack.len() > MAX_DFS_STACK_DEPTH {
                stats.stack_overflows = stats.stack_overflows.saturating_add(1);
                // Retailer la pile pour eviter le depassement.
                stack.pop();
                continue;
            }
            let frame = match stack.last_mut() {
                Some(f) => f,
                None => break,
            };

            let current = frame.blob_id;

            // Obtenir les enfants du noeud courant.
            let children = REFERENCE_TRACKER.get_refs(&current);
            let child_idx = frame.child_index;

            if child_idx >= children.len() {
                // Tous les enfants traites : noircir ce noeud et depiler.
                colors.insert(current, DfsColor::Black);
                stack.pop();
                continue;
            }

            // Avancer l'index enfant.
            // NB: on doit reemprunter `stack.last_mut()` apres la borrow
            // — on change l'index independamment.
            let child = children[child_idx];
            // Incrementer l'index de l'enfant courant.
            if let Some(f) = stack.last_mut() {
                f.child_index = f.child_index.saturating_add(1);
            }

            stats.edges_traversed = stats.edges_traversed.saturating_add(1);

            match colors.get(&child).copied().unwrap_or(DfsColor::White) {
                DfsColor::White => {
                    // Noeud non visite : continuer le DFS.
                    colors.insert(child, DfsColor::Grey);
                    stats.nodes_visited = stats.nodes_visited.saturating_add(1);
                    parent.insert(child, current);

                    if stack.len() < MAX_DFS_STACK_DEPTH {
                        stack.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                        stack.push(DfsFrame { blob_id: child, child_index: 0 });
                    }
                }
                DfsColor::Grey => {
                    // Arête de retour -> CYCLE détecté !
                    if cycles.len() < MAX_CYCLES_PER_PASS {
                        let cycle = self.extract_cycle(
                            child,
                            current,
                            parent,
                        )?;
                        cycles.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                        cycles.push(cycle);
                    }
                }
                DfsColor::Black => {
                    // Noeud deja completement visite : pas de cycle ici.
                }
            }

            // Eviter les borrow doubles — les `children` sont re-fetches a chaque tour.
            let _ = children_start.len(); // Silence unused warning.
        }

        Ok(())
    }

    /// Extrait les membres d'un cycle detecte depuis le graphe des parents.
    ///
    /// RECUR-01 : remontee iterative du parent chain.
    fn extract_cycle(
        &self,
        cycle_entry: BlobId,
        cycle_close: BlobId,
        parent:      &BTreeMap<BlobId, BlobId>,
    ) -> ExofsResult<DetectedCycle> {
        let mut members: Vec<BlobId> = Vec::new();
        let mut visited: BTreeSet<BlobId> = BTreeSet::new();

        // Remonter depuis cycle_close jusqu'a retrouver cycle_entry.
        let mut current = cycle_close;
        loop {
            if members.len() >= MAX_CYCLE_MEMBERS {
                break;
            }
            if !visited.insert(current) {
                break; // Boucle détectée dans la remontée.
            }
            members.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            members.push(current);

            if current == cycle_entry {
                break;
            }

            match parent.get(&current) {
                Some(&p) => current = p,
                None => break,
            }
        }

        members.reverse();

        Ok(DetectedCycle {
            entry:           cycle_entry,
            total_size_hint: 0, // A remplir si les tailles sont connues.
            members,
        })
    }

    // ── Blobs membres de cycles ─────────────────────────────────────────────

    /// Collecte tous les BlobIds qui sont dans au moins un cycle.
    pub fn all_cycle_members(&self, cycles: &[DetectedCycle]) -> BTreeSet<BlobId> {
        let mut set = BTreeSet::new();
        for cycle in cycles {
            for &bid in &cycle.members {
                set.insert(bid);
            }
        }
        set
    }

    /// Vérifie si un BlobId est membre d'un cycle connu.
    pub fn is_in_cycle(&self, blob_id: &BlobId, cycles: &[DetectedCycle]) -> bool {
        cycles.iter().any(|c| c.contains(blob_id))
    }

    // ── Accesseurs ──────────────────────────────────────────────────────────

    /// Stats cumulees.
    pub fn total_stats(&self) -> CycleDetectStats {
        self.inner.lock().total_stats.clone()
    }

    /// Derniers cycles detectes.
    pub fn last_cycles(&self) -> Vec<DetectedCycle> {
        self.inner.lock().last_cycles.clone()
    }

    /// Nombre de passes.
    pub fn pass_count(&self) -> u64 {
        self.inner.lock().pass_count
    }

    /// Reset des stats.
    pub fn reset_stats(&self) {
        let mut g = self.inner.lock();
        g.total_stats = CycleDetectStats::default();
        g.pass_count = 0;
        g.last_cycles.clear();
    }
}

// ==============================================================================
// Instance globale
// ==============================================================================

/// Détecteur de cycles GC global.
pub static CYCLE_DETECTOR: CycleDetector = CycleDetector::new();

// ==============================================================================
// Tests
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::core::BlobId;

    fn bid(b: u8) -> BlobId {
        let mut a = [0u8; 32]; a[0] = b; BlobId(a)
    }

    #[test]
    fn test_empty_graph_no_cycles() {
        let detector = CycleDetector::new();
        let cycles = detector.detect_cycles(&[]).unwrap();
        assert!(cycles.is_empty());
    }

    #[test]
    fn test_single_node_no_cycle() {
        let detector = CycleDetector::new();
        let cycles = detector.detect_cycles(&[bid(1)]).unwrap();
        // Pas d'aretes = pas de cycle.
        assert!(cycles.is_empty());
    }

    #[test]
    fn test_detected_cycle_display() {
        let c = DetectedCycle {
            members:         alloc::vec![bid(1), bid(2)],
            entry:           bid(1),
            total_size_hint: 1024,
        };
        assert!(c.contains(&bid(1)));
        assert!(!c.contains(&bid(5)));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn test_all_cycle_members() {
        let detector = CycleDetector::new();
        let c1 = DetectedCycle {
            members:         alloc::vec![bid(1), bid(2)],
            entry:           bid(1),
            total_size_hint: 0,
        };
        let c2 = DetectedCycle {
            members:         alloc::vec![bid(3), bid(4)],
            entry:           bid(3),
            total_size_hint: 0,
        };
        let members = detector.all_cycle_members(&[c1, c2]);
        assert_eq!(members.len(), 4);
        assert!(members.contains(&bid(1)));
        assert!(members.contains(&bid(4)));
    }

    #[test]
    fn test_stats_initial() {
        let detector = CycleDetector::new();
        let stats = detector.total_stats();
        assert_eq!(stats.cycles_found, 0);
        assert_eq!(stats.passes, 0);
    }
}
