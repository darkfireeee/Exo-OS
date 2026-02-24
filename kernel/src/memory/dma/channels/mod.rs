// kernel/src/memory/dma/channels/mod.rs
//
// Module canaux DMA.

pub mod manager;
pub mod channel;
pub mod priority;
pub mod affinity;

pub use channel::{
    DmaCommand, DmaChannelRing, CHANNEL_RING_SIZE,
};
pub use priority::{
    PriorityScheduler, DMA_PRIORITY_SCHEDULER, STARVATION_THRESHOLD,
};
pub use affinity::{
    DmaAffinityTable, DMA_AFFINITY, CPU_AFFINITY_NONE, NUMA_NODE_NONE,
};
pub use manager::{
    DmaChannel, DmaChannelManager, DMA_CHANNELS,
    ChannelState, ChannelStats,
    MAX_DMA_CHANNELS, CHANNEL_QUEUE_DEPTH,
};
