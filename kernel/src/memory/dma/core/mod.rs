// kernel/src/memory/dma/core/mod.rs
//
// Sous-module core du DMA — types, descripteurs, mapping, interface de réveil.

pub mod types;
pub mod descriptor;
pub mod mapping;
pub mod wakeup_iface;
pub mod error;

pub use types::{
    DmaChannelId, IommuDomainId, DmaTransactionId, IovaAddr,
    DmaDirection, DmaMapFlags, DmaTransactionState, DmaPriority,
    DmaCapabilities, DmaError,
};
pub use descriptor::{
    SgEntry, DmaDescriptor, DmaDescriptorTable, DMA_DESCRIPTOR_TABLE,
    MAX_SG_ENTRIES, MAX_DMA_TRANSACTIONS,
};
pub use mapping::{DmaMapping, IovaAllocator, IOVA_ALLOCATOR, MAX_DMA_MAPPINGS};
pub use error::{
    DmaErrorContext, DmaErrorCounters, DmaErrorSnapshot,
    DMA_ERROR_COUNTERS, record_error, record_error_ctx,
};
pub use wakeup_iface::{
    DmaWakeupHandler, register_wakeup_handler, wake_on_completion, wake_all_on_error,
    has_real_handler, lost_wakeup_count,
};
