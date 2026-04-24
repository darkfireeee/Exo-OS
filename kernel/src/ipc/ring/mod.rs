// kernel/src/ipc/ring/mod.rs
//
// Module ring — rings de communication IPC.

pub mod batch;
pub mod fusion;
pub mod mpmc;
pub mod slot;
pub mod spsc;
pub mod zerocopy;

pub use batch::{batch_receive, BatchBuffer, BatchReceiveResult, MAX_BATCH_SIZE};
pub use fusion::{FusionMode, FusionRing};
pub use mpmc::MpmcRing;
pub use spsc::SpscRing;
pub use zerocopy::{ZeroCopyBuffer, ZeroCopyRing};
