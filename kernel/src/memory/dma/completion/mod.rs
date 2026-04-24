// kernel/src/memory/dma/completion/mod.rs
//
// Module completion DMA.

pub mod handler;
pub mod polling;
pub mod wakeup;

pub use handler::{DmaCompletionManager, DMA_COMPLETION, MAX_PENDING_COMPLETIONS};
pub use polling::{bounded_poll, poll, poll_long, spin_poll, PollResult, POLL_STATS};
pub use wakeup::{
    notify_completion, register_wakeup, DmaWakeupTable, WaiterSlot, DMA_WAKEUP_TABLE,
};
