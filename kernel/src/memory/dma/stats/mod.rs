// kernel/src/memory/dma/stats/mod.rs
//
// Module stats DMA — compteurs de débit, latence, erreurs.

pub mod counters;

pub use counters::{
    dma_bytes_transferred, dma_stat_complete, dma_stat_error, dma_stat_submit, dma_stat_timeout,
    dump_dma_stats, DmaEngineStats, DmaStats, DMA_STATS,
};
