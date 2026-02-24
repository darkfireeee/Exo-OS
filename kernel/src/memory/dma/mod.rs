// kernel/src/memory/dma/mod.rs
//
// Module DMA complet — sous-système de Direct Memory Access Exo-OS.
//
// Architecture :
//   core/      — types, descripteurs, mapping IOVA, interface wakeup
//   iommu/     — domaines, tables de pages IOMMU, Intel VT-d, AMD-Vi
//   channels/  — gestionnaire de canaux
//   ops/       — memcpy, memset, scatter-gather
//   completion/ — gestionnaire de complétion + réveil
//   engines/   — pilotes matériels (I/OAT, DSA, AHCI, NVMe, VirtIO)
//   stats/     — compteurs DMA par moteur

pub mod core;
pub mod iommu;
pub mod channels;
pub mod ops;
pub mod completion;
pub mod engines;
pub mod stats;

// ─────────────────────────────────────────────────────────────────────────────
// RE-EXPORTS PUBLIQUES
// ─────────────────────────────────────────────────────────────────────────────

pub use core::types::{
    DmaChannelId, IommuDomainId, DmaTransactionId, IovaAddr,
    DmaDirection, DmaMapFlags, DmaTransactionState, DmaPriority,
    DmaCapabilities, DmaError,
};
pub use core::descriptor::{SgEntry, DmaDescriptor, DMA_DESCRIPTOR_TABLE};
pub use core::mapping::{IOVA_ALLOCATOR};
pub use core::wakeup_iface::{
    DmaWakeupHandler, register_wakeup_handler, wake_on_completion, wake_all_on_error,
};

pub use iommu::domain::{IOMMU_DOMAINS, DomainType, PciBdf};
pub use iommu::intel_vtd::INTEL_VTD;
pub use iommu::amd_iommu::AMD_IOMMU;

pub use channels::manager::DMA_CHANNELS;
pub use ops::memcpy::{DmaOpHandle, dma_memcpy_async, dma_memcpy_sync, sw_memcpy};
pub use ops::memset::{dma_memset, dma_zero, sw_memset};
pub use ops::scatter_gather::{dma_sg_async, sw_sg_copy};
pub use completion::handler::DMA_COMPLETION;

pub use stats::counters::{DmaStats, DMA_STATS, dump_dma_stats};
pub use engines::{
    DmaEngine,
    IOAT_ENGINE, ioat_init, ioat_submit, ioat_poll,
    IDXD_ENGINE, idxd_init, idxd_submit, idxd_poll,
    AHCI_DMA,   ahci_dma_init, ahci_dma_read, ahci_dma_write, ahci_dma_poll,
    NVME_DMA,   nvme_dma_init, nvme_read, nvme_write, nvme_poll,
    VIRTIO_DMA, virtio_dma_init, virtio_dma_submit, virtio_dma_poll,
};

// ─────────────────────────────────────────────────────────────────────────────
// INITIALISATION DMA
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise le sous-système DMA (Phase 1 : tables statiques).
/// Appelé depuis `memory::init()` après l'allocateur physique.
pub fn init() {
    // Initialise la table IOMMU (domaine identité).
    IOMMU_DOMAINS.init();
    // Initialise la table de descripteurs.
    DMA_DESCRIPTOR_TABLE.init();
    // La table des canaux est déjà initialisée statiquement.
    // Les pilotes IOMMU (VT-d, AMD-Vi) sont initialisés depuis le parseur ACPI.
}
