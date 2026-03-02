// kernel/src/fs/exofs/objects/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Module objects/ — Objets logiques et physiques ExoFS
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════

pub mod logical_object;
pub mod physical_blob;
pub mod physical_ref;
pub mod object_meta;
pub mod inline_data;
pub mod extent;
pub mod extent_tree;
pub mod object_builder;
pub mod object_loader;
pub mod object_cache;
pub mod object_kind;

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports
// ─────────────────────────────────────────────────────────────────────────────

pub use logical_object::{LogicalObject, LogicalObjectDisk, LogicalObjectRef};
pub use physical_blob::{PhysicalBlobDisk, PhysicalBlobInMemory, PhysicalBlobRef};
pub use physical_ref::PhysicalRef;
pub use object_meta::ObjectMeta;
pub use inline_data::InlineData;
pub use extent::{ObjectExtent, ObjectExtentDisk};
pub use extent_tree::ExtentTree;
pub use object_builder::ObjectBuilder;
pub use object_loader::load_object;
pub use object_cache::ObjectCache;
