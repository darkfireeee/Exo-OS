// kernel/src/process/state/mod.rs
//
// Machine à états du processus et pont DMA Wakeup.

pub mod transitions;
pub mod wakeup;

pub use transitions::{transition, StateTransition, TransitionError};
pub use wakeup::{PROCESS_WAKEUP_HANDLER, register_with_dma};
