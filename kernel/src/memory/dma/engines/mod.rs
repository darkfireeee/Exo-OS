// kernel/src/memory/dma/engines/mod.rs
//
// Module engines — pilotes des moteurs DMA matériels.
//
//   ioat      : Intel I/OAT (série Xeon)
//   idxd      : Intel DSA/IAX (Data Streaming Accelerator)
//   ahci_dma  : SATA AHCI PRDT
//   nvme_dma  : NVMe PCIe queues
//   virtio_dma: VirtIO virtqueue

pub mod ahci_dma;
pub mod idxd;
pub mod ioat;
pub mod nvme_dma;
pub mod virtio_dma;

// Re-exports principaux
pub use ahci_dma::{
    ahci_dma_init, ahci_dma_poll, ahci_dma_read, ahci_dma_write, AhciDmaEngine, AHCI_DMA,
};
pub use idxd::{idxd_init, idxd_poll, idxd_submit, IdxdEngine, IDXD_ENGINE};
pub use ioat::{ioat_init, ioat_poll, ioat_submit, IoatEngine, IOAT_ENGINE};
pub use nvme_dma::{nvme_dma_init, nvme_poll, nvme_read, nvme_write, NvmeDmaEngine, NVME_DMA};
pub use virtio_dma::{
    virtio_dma_init, virtio_dma_poll, virtio_dma_submit, VirtioDmaEngine, VIRTIO_DMA,
};

/// Trait commun à tous les moteurs DMA matériels.
pub trait DmaEngine: Send + Sync {
    /// Identifiant unique du moteur (index dans DMA_STATS).
    fn engine_id(&self) -> usize;
    /// Vérifie si le matériel est présent/actif.
    fn is_present(&self) -> bool;
    /// Initialise le moteur (MMIO, rings, etc.).
    ///
    /// # Safety : CPL 0, adresse MMIO valide.
    unsafe fn init(&self, mmio_base: u64) -> bool;
    /// Retourne le nombre de slots disponibles dans la file de soumission.
    fn available_slots(&self) -> usize;
}
