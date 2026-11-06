// kernel/src/process/state/mod.rs
//
// Process state machine and DMA Wakeup bridge.

pub mod transitions;
pub mod wakeup;

pub use transitions::{transition, StateTransition, TransitionError};
pub use wakeup::{register_with_dma, PROCESS_WAKEUP_HANDLER};
