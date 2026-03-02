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
