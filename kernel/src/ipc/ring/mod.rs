// kernel/src/ipc/ring/mod.rs
//
// Module ring — rings de communication IPC.

pub mod slot;
pub mod spsc;
pub mod mpmc;
pub mod fusion;
pub mod batch;
pub mod zerocopy;

pub use spsc::SpscRing;
pub use mpmc::MpmcRing;
pub use fusion::{FusionRing, FusionMode};
pub use zerocopy::{ZeroCopyRing, ZeroCopyBuffer};
pub use batch::{BatchBuffer, BatchReceiveResult, batch_receive, MAX_BATCH_SIZE};
