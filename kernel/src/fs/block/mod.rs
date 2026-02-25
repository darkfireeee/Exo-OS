// kernel/src/fs/block/mod.rs
//
// Block layer — bio, queue, scheduler I/O deadline, abstraction device.

pub mod bio;
pub mod queue;
pub mod scheduler;
pub mod device;

pub use bio::{
    Bio, BioOp, BioFlags, BioStatus, BioVec, BioStats, BIO_STATS,
};
pub use queue::{
    RequestQueue, QueueStats, QUEUE_STATS,
    submit_bio, flush_block_queue,
};
pub use scheduler::{
    DeadlineScheduler, IO_SCHEDULER,
    SCHED_TICK, tick, advance_tick, schedule_io, schedule_dispatch,
};
pub use device::{
    BlockDevice, BlockDevInfo, BlockDevStats,
    BlockDevRegistry, BLOCK_DEV_REGISTRY,
    DevRegistryStats, DEV_STATS,
};
