// kernel/src/memory/dma/channels/mod.rs
//
// Module canaux DMA.

pub mod affinity;
pub mod channel;
pub mod manager;
pub mod priority;

pub use affinity::{DmaAffinityTable, CPU_AFFINITY_NONE, DMA_AFFINITY, NUMA_NODE_NONE};
pub use channel::{DmaChannelRing, DmaCommand, CHANNEL_RING_SIZE};
pub use manager::{
    ChannelState, ChannelStats, DmaChannel, DmaChannelManager, CHANNEL_QUEUE_DEPTH, DMA_CHANNELS,
    MAX_DMA_CHANNELS,
};
pub use priority::{PriorityScheduler, DMA_PRIORITY_SCHEDULER, STARVATION_THRESHOLD};
