//! Module relation/ — relation graph between ExoFS blobs (no_std).

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
pub use relation_batch::{BatchResult, RelationBatch};
pub use relation_cycle::{CycleReport, RelationCycleDetector};
pub use relation_gc::{BlobExistsChecker, RelationGc, RelationGcReport};
pub use relation_graph::{RelationGraph, RELATION_GRAPH};
pub use relation_index::{RelationIndex, RELATION_INDEX};
pub use relation_query::{QueryResult, RelationQuery};
pub use relation_storage::{RelationStorage, RELATION_STORAGE};
pub use relation_type::{RelationKind, RelationType};
pub use relation_walker::{RelationWalker, WalkResult};

// ─────────────────────────────────────────────────────────────────────────────
// Module lifecycle functions
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes the relation module (idempotent).
///
/// To be called when mounting the ExoFS filesystem.
pub fn init() {
    // The stores are statics initialized with `new_const()` —
    // no allocation is required at init. This function
    // is preserved to be called explicitly in `exofs::init()`
    // and indicate initialization order.
}

/// Releases volatile resources and flushes in-memory structures.
///
/// To be called when unmounting the filesystem or during kernel shutdown.
pub fn shutdown() {
    RELATION_GRAPH.flush();
    RELATION_INDEX.flush();
    // `RELATION_STORAGE` flushes on-disk blocks if implemented.
}

/// Verifies consistency of internal structures.
///
/// Returns `true` if all sanity constraints are satisfied.
pub fn verify_health() -> bool {
    use relation_storage::STORAGE_MAX_RELATIONS;
    let n = RELATION_STORAGE.count();
    if n > STORAGE_MAX_RELATIONS {
        return false;
    }

    // The number of edges in the graph must remain consistent with
    // the number of persisted relations.
    let n_edges = RELATION_GRAPH.n_edges() as usize;
    if n_edges > n.saturating_mul(2) {
        return false;
    }

    true
}
