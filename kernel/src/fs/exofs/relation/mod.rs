//! Module relation/ — graphe de relations entre blobs ExoFS (no_std).

pub mod relation;
pub mod relation_batch;
pub mod relation_cycle;
pub mod relation_gc;
pub mod relation_graph;
pub mod relation_index;
pub mod relation_query;
pub mod relation_storage;
pub mod relation_type;
pub mod relation_walker;

pub use relation::{Relation, RelationId};
pub use relation_batch::{RelationBatch, BatchResult};
pub use relation_cycle::{RelationCycleDetector, CycleReport};
pub use relation_gc::{RelationGc, RelationGcReport, BlobExistsChecker};
pub use relation_graph::{RelationGraph, RELATION_GRAPH};
pub use relation_index::{RelationIndex, RELATION_INDEX};
pub use relation_query::{RelationQuery, QueryResult};
pub use relation_storage::{RelationStorage, RELATION_STORAGE};
pub use relation_type::{RelationType, RelationKind};
pub use relation_walker::{RelationWalker, WalkResult};

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions de cycle de vie du module
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise le module relation (idempotent).
///
/// À appeler lors du montage du système de fichiers ExoFS.
pub fn init() {
    // Les stores sont des statics initialisés avec `new_const()` —
    // aucune allocation n'est nécessaire à l'init. Cette fonction
    // est conservée pour être appelée explicitement dans `exofs::init()`
    // et indiquer l'ordre d'initialisation.
}

/// Libère les ressources volatiles et vide les structures en mémoire.
///
/// À appeler lors du démontage du système de fichiers ou shutdown noyau.
pub fn shutdown() {
    RELATION_GRAPH.flush();
    RELATION_INDEX.flush();
    // `RELATION_STORAGE` flush les blocs on-disk si implémenté.
}

/// Vérifie la cohérence des structures internes.
///
/// Retourne `true` si toutes les contraintes de sanité sont respectées.
pub fn verify_health() -> bool {
    use relation_storage::STORAGE_MAX_RELATIONS;
    let n = RELATION_STORAGE.count();
    if n > STORAGE_MAX_RELATIONS { return false; }

    // Le nombre d arêtes dans le graphe doit rester cohérent avec
    // le nombre de relations persistées.
    let n_edges = RELATION_GRAPH.n_edges() as usize;
    if n_edges > n.saturating_mul(2) { return false; }

    true
}
