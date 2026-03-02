// path/mod.rs — API résolution chemins ExoFS
// Ring 0, no_std — buffer per-CPU, jamais récursif

pub mod resolver;
pub mod path_index;
pub mod path_index_tree;
pub mod path_index_split;
pub mod path_index_merge;
pub mod path_component;
pub mod symlink;
pub mod mount_point;
pub mod namespace;
pub mod canonicalize;
pub mod path_cache;
pub mod path_walker;

pub use resolver::resolve_path;
pub use path_index::{PathIndex, PathIndexEntry};
pub use path_component::{PathComponent, validate_component};
pub use path_cache::PathCache;
pub use canonicalize::canonicalize_path;
