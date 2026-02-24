// kernel/src/memory/physical/numa/distance.rs
//
// Table des distances NUMA — coût d'accès entre nœuds.
//
// La table est issue du ACPI SLIT (System Locality Information Table).
// En l'absence de SLIT, la table est initialisée avec des valeurs par défaut :
//   • distance(i, i) = 10 (local)
//   • distance(i, j) = 20 (distant, horizon 1 hop)
//
// RÈGLE IA-KERNEL-01 : la table est stockée en .rodata-like via un static ;
// aucune génération runtime de politique d'allocation.
//
// Couche 0 — pas de dépendance scheduler/process/ipc/fs.

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use spin::Once;

use super::node::{MAX_NUMA_NODES, NUMA_NODES};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes de distance
// ─────────────────────────────────────────────────────────────────────────────

/// Distance locale (même nœud) selon ACPI NUMA standard.
pub const NUMA_DISTANCE_LOCAL: u8 = 10;
/// Distance distante (1 hop) par défaut.
pub const NUMA_DISTANCE_REMOTE: u8 = 20;
/// Distance maximale réaliste (2 hops sur une topologie ring).
pub const NUMA_DISTANCE_FAR: u8 = 40;
/// Valeur sentinelle : nœud inaccessible.
pub const NUMA_DISTANCE_UNREACHABLE: u8 = u8::MAX;

// ─────────────────────────────────────────────────────────────────────────────
// Table des distances
// ─────────────────────────────────────────────────────────────────────────────

/// Table symétrique MAX_NUMA_NODES × MAX_NUMA_NODES de distances uint8.
pub struct NumaDistanceTable {
    /// Valeurs : `dist[from][to]`.
    dist: [[u8; MAX_NUMA_NODES]; MAX_NUMA_NODES],
    initialized: AtomicBool,
}

impl NumaDistanceTable {
    /// Construit la table avec les valeurs par défaut.
    const fn default_init() -> Self {
        // Initialisation : locale=10, distante=20.
        let mut dist = [[NUMA_DISTANCE_REMOTE; MAX_NUMA_NODES]; MAX_NUMA_NODES];
        let mut i = 0;
        while i < MAX_NUMA_NODES {
            dist[i][i] = NUMA_DISTANCE_LOCAL;
            i += 1;
        }
        Self { dist, initialized: AtomicBool::new(true) }
    }

    /// Retourne la distance entre `from` et `to`.
    #[inline]
    pub fn get(&self, from: u32, to: u32) -> u8 {
        if from as usize >= MAX_NUMA_NODES || to as usize >= MAX_NUMA_NODES {
            return NUMA_DISTANCE_UNREACHABLE;
        }
        self.dist[from as usize][to as usize]
    }

    /// Met à jour la distance (depuis le parseur ACPI SLIT).
    ///
    /// # Safety : appelé en initialisation single-threaded.
    pub unsafe fn set(&mut self, from: u32, to: u32, distance: u8) {
        if (from as usize) < MAX_NUMA_NODES && (to as usize) < MAX_NUMA_NODES {
            self.dist[from as usize][to as usize] = distance;
            self.dist[to as usize][from as usize] = distance; // symétrie
        }
    }

    /// Retourne l'identifiant du nœud le plus proche de `origin`
    /// parmi les nœuds actifs ayant au moins `min_free_pages` pages libres.
    pub fn closest_node_with_memory(&self, origin: u32, min_free_pages: u64) -> u32 {
        let node_count = NUMA_NODES.count();
        let mut best_dist = NUMA_DISTANCE_UNREACHABLE;
        let mut best_node = super::node::NUMA_NODE_INVALID;

        for nid in 0..node_count {
            if let Some(node) = NUMA_NODES.get(nid) {
                if node.free_pages() >= min_free_pages {
                    let d = self.get(origin, nid);
                    if d < best_dist {
                        best_dist = d;
                        best_node = nid;
                    }
                }
            }
        }
        best_node
    }

    /// Retourne la liste triée des nœuds par distance croissante depuis `origin`.
    /// Retourne un tableau de (node_id, distance) et sa longueur valide.
    pub fn sorted_nodes_from(&self, origin: u32) -> ([u32; MAX_NUMA_NODES], usize) {
        let node_count = NUMA_NODES.count() as usize;
        let mut ids: [u32; MAX_NUMA_NODES] = [super::node::NUMA_NODE_INVALID; MAX_NUMA_NODES];
        let mut dists: [u8; MAX_NUMA_NODES] = [NUMA_DISTANCE_UNREACHABLE; MAX_NUMA_NODES];
        let mut len = 0;

        for nid in 0..node_count {
            if NUMA_NODES.get(nid as u32).is_some() {
                ids[len] = nid as u32;
                dists[len] = self.get(origin, nid as u32);
                len += 1;
            }
        }

        // Tri à bulles (len ≤ 8 → O(n²) acceptable).
        for i in 0..len {
            for j in i + 1..len {
                if dists[j] < dists[i] {
                    ids.swap(i, j);
                    dists.swap(i, j);
                }
            }
        }

        (ids, len)
    }
}

unsafe impl Sync for NumaDistanceTable {}

/// Table des distances statique globale.
pub static NUMA_DISTANCE: NumaDistanceTable = NumaDistanceTable::default_init();

// ─────────────────────────────────────────────────────────────────────────────
// Helpers publics
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne la distance entre deux nœuds NUMA.
#[inline]
pub fn numa_distance(from: u32, to: u32) -> u8 {
    NUMA_DISTANCE.get(from, to)
}

/// Retourne `true` si `from` et `to` sont le même nœud (distance locale).
#[inline]
pub fn numa_same_node(from: u32, to: u32) -> bool {
    from == to
}

/// Retourne le nœud le plus proche disposant de mémoire.
#[inline]
pub fn closest_node(origin: u32, min_free: u64) -> u32 {
    NUMA_DISTANCE.closest_node_with_memory(origin, min_free)
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation
// ─────────────────────────────────────────────────────────────────────────────

/// Init table des distances — déjà initialisée avec default_init().
/// Le parseur ACPI SLIT peut appeler `NUMA_DISTANCE.set()` pour mettre à jour.
pub fn init() {
    // Table déjà initialisée constexpr.
    // Si ACPI SLIT présent : appeler NUMA_DISTANCE.set() pour chaque entrée.
}
