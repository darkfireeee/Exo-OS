// kernel/src/memory/dma/core/mod.rs
//
// Sous-module core du DMA — types, descripteurs, mapping, interface de réveil.

pub mod descriptor;
pub mod error;
pub mod mapping;
pub mod types;
pub mod wakeup_iface;

pub use descriptor::{
    DmaDescriptor, DmaDescriptorTable, SgEntry, DMA_DESCRIPTOR_TABLE, MAX_DMA_TRANSACTIONS,
    MAX_SG_ENTRIES,
};
pub use error::{
    record_error, record_error_ctx, DmaErrorContext, DmaErrorCounters, DmaErrorSnapshot,
    DMA_ERROR_COUNTERS,
};
pub use mapping::{DmaMapping, IovaAllocator, IOVA_ALLOCATOR, MAX_DMA_MAPPINGS};
pub use types::{
    DmaCapabilities, DmaChannelId, DmaDirection, DmaError, DmaMapFlags, DmaPriority,
    DmaTransactionId, DmaTransactionState, IommuDomainId, IovaAddr,
};
pub use wakeup_iface::{
    has_real_handler, lost_wakeup_count, register_wakeup_handler, wake_all_on_error,
    wake_on_completion, DmaWakeupHandler,
};
