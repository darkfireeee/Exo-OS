// libs/exo_ipc/src/ring/mod.rs
//! Ring buffers lock-free pour IPC

pub mod spsc;
pub mod mpsc;

// Réexportations
pub use spsc::SpscRing;
pub use mpsc::MpscRing;
