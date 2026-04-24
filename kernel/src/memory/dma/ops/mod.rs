// kernel/src/memory/dma/ops/mod.rs
//
// Module ops DMA — memcpy, memset, scatter_gather.

pub mod cyclic;
pub mod interleaved;
pub mod memcpy;
pub mod memset;
pub mod scatter_gather;

pub use cyclic::{
    CyclicConfig, CyclicManager, DMA_CYCLIC, MAX_CYCLIC_CHANNELS, MAX_CYCLIC_PERIODS,
};
pub use interleaved::{
    sw_interleaved_copy, InterleavedConfig, InterleavedManager, InterleavedStride, DMA_INTERLEAVED,
    MAX_INTERLEAVED_CHUNKS,
};
pub use memcpy::{dma_memcpy_async, dma_memcpy_sync, sw_memcpy, DmaOpHandle};
pub use memset::{dma_memset, dma_zero, sw_memset};
pub use scatter_gather::{dma_sg_async, sw_sg_copy};
