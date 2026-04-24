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

pub mod channels;
pub mod completion;
pub mod core;
pub mod engines;
pub mod iommu;
pub mod ops;
pub mod stats;

// ─────────────────────────────────────────────────────────────────────────────
// RE-EXPORTS PUBLIQUES
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
