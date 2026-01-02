//! Driver Compatibility Layers
//!
//! Provides compatibility shims for integrating external drivers

pub mod linux;

// Re-exports
pub use linux::{Device, Driver, Bus, PowerState, Resource};
pub use linux::{dma_alloc_coherent, dma_free_coherent};
pub use linux::{request_irq, free_irq, IrqHandler, IrqReturn};
