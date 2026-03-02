// kernel/src/memory/physical/numa/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Module NUMA — topologie mémoire NUMA, nœuds, distances, politique, migration
// ═══════════════════════════════════════════════════════════════════════════════
//
// Sous-modules :
//   node       — descripteurs de nœuds, table NUMA_NODES, compteurs
//   distance   — table de distances SLIT, closest_node(), numa_distance()
//   policy     — NumaPolicy, NumaNodeMask, politiques par thread
//   migration  — migration de pages entre nœuds
//
// Re-exports vers memory::numa (via memory/numa.rs qui fait `pub use crate::memory::physical::numa::*;`).
//
// COUCHE 0 — aucune dépendance scheduler/process/ipc/fs.
// ═══════════════════════════════════════════════════════════════════════════════

pub mod distance;
pub mod migration;
pub mod node;
pub mod policy;

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports publics
// ─────────────────────────────────────────────────────────────────────────────

// Table globale des nœuds NUMA actifs.
pub use node::{NUMA_NODES, NumaNodeTable, NumaNode, NumaNodeStats, NumaPhysRange,
               NumaGlobalStats, NUMA_GLOBAL_STATS, MAX_NUMA_NODES, NUMA_NODE_INVALID};

// Distances inter-nœuds et helper de nœud le plus proche.
pub use distance::{numa_distance, numa_same_node, closest_node, NumaDistanceTable,
                   NUMA_DISTANCE, NUMA_DISTANCE_LOCAL, NUMA_DISTANCE_REMOTE, NUMA_DISTANCE_FAR};

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise la topologie NUMA : nœuds + table de distances.
///
/// - Enregistre le nœud 0 (BSP) par défaut.
/// - Initialise la table de distances avec les valeurs SLIT par défaut
///   (distance locale = 10, distante = 20).
///
/// # Safety
/// Appelé une seule fois depuis `memory::init()` (Phase 8), en mode Ring 0,
/// avant l'activation des APs.
pub unsafe fn init() {
    distance::init();
}
