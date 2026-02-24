// kernel/src/memory/dma/completion/mod.rs
//
// Module completion DMA.

pub mod handler;
pub mod wakeup;
pub mod polling;

pub use wakeup::{
    DmaWakeupTable, WaiterSlot, DMA_WAKEUP_TABLE,
    register_wakeup, notify_completion,
};
pub use polling::{
    PollResult, POLL_STATS,
    spin_poll, bounded_poll, poll, poll_long,
};
pub use handler::{
    DmaCompletionManager, DMA_COMPLETION, MAX_PENDING_COMPLETIONS,
};
