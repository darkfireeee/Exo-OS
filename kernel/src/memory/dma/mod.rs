// kernel/src/memory/dma/mod.rs
//
// Complete DMA module — Exo-OS Direct Memory Access subsystem.
//
// Architecture:
//   core/       — types, descriptors, IOVA mapping, wakeup interface
//   iommu/      — domains, IOMMU page tables, Intel VT-d, AMD-Vi
//   channels/   — channel manager
//   ops/        — memcpy, memset, scatter-gather
//   completion/ — completion manager + wakeup
//   engines/    — hardware drivers (I/OAT, DSA, AHCI, NVMe, VirtIO)
//   stats/      — per-engine DMA counters

pub mod channels;
pub mod completion;
pub mod core;
pub mod engines;
pub mod iommu;
pub mod ops;
pub mod stats;

// ─────────────────────────────────────────────────────────────────────────────
// PUBLIC RE-EXPORTS
// ─────────────────────────────────────────────────────────────────────────────

pub use core::descriptor::{DmaDescriptor, SgEntry, DMA_DESCRIPTOR_TABLE};
pub use core::mapping::IOVA_ALLOCATOR;
pub use core::types::{
    DmaCapabilities, DmaChannelId, DmaDirection, DmaError, DmaMapFlags, DmaPriority,
    DmaTransactionId, DmaTransactionState, IommuDomainId, IovaAddr,
};
pub use core::wakeup_iface::{
    register_wakeup_handler, wake_all_on_error, wake_on_completion, DmaWakeupHandler,
};

pub use iommu::amd_iommu::AMD_IOMMU;
pub use iommu::domain::{DomainType, PciBdf, IOMMU_DOMAINS};
pub use iommu::intel_vtd::INTEL_VTD;

pub use channels::manager::DMA_CHANNELS;
pub use completion::handler::DMA_COMPLETION;
pub use ops::memcpy::{dma_memcpy_async, dma_memcpy_sync, sw_memcpy, DmaOpHandle};
pub use ops::memset::{dma_memset, dma_zero, sw_memset};
pub use ops::scatter_gather::{dma_sg_async, sw_sg_copy};

pub use engines::{
    ahci_dma_init, ahci_dma_poll, ahci_dma_read, ahci_dma_write, idxd_init, idxd_poll, idxd_submit,
    ioat_init, ioat_poll, ioat_submit, nvme_dma_init, nvme_poll, nvme_read, nvme_write,
    virtio_dma_init, virtio_dma_poll, virtio_dma_submit, DmaEngine, AHCI_DMA, IDXD_ENGINE,
    IOAT_ENGINE, NVME_DMA, VIRTIO_DMA,
};
pub use stats::counters::{dump_dma_stats, DmaStats, DMA_STATS};

// ─────────────────────────────────────────────────────────────────────────────
// DMA INITIALIZATION
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes the DMA subsystem (Phase 1: static tables).
/// Called from `memory::init()` after physical allocator.
pub fn init() {
    // Initialize IOMMU table (identity domain).
    IOMMU_DOMAINS.init();
    // Initialize descriptor table.
    DMA_DESCRIPTOR_TABLE.init();
    // Channel table is already initialized statically.
    // IOMMU drivers (VT-d, AMD-Vi) are initialized from ACPI parser.
}
