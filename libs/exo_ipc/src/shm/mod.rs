// libs/exo_ipc/src/shm/mod.rs
//! Mémoire partagée et pools pour zero-copy IPC

pub mod region;
pub mod pool;

// Réexportations
pub use region::{
    SharedRegion, SharedMapping, RegionId, RegionPermissions,
};
pub use pool::MessagePool;
