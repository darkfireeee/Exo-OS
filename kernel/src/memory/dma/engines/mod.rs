// kernel/src/memory/dma/engines/mod.rs
//
// Module engines — pilotes des moteurs DMA matériels.
//
//   ioat      : Intel I/OAT (série Xeon)
//   idxd      : Intel DSA/IAX (Data Streaming Accelerator)
//   ahci_dma  : SATA AHCI PRDT
//   nvme_dma  : NVMe PCIe queues
//   virtio_dma: VirtIO virtqueue

pub mod ioat;
pub mod idxd;
pub mod ahci_dma;
pub mod nvme_dma;
pub mod virtio_dma;

// Re-exports principaux
pub use ioat::{IoatEngine, IOAT_ENGINE, ioat_init, ioat_submit, ioat_poll};
pub use idxd::{IdxdEngine, IDXD_ENGINE, idxd_init, idxd_submit, idxd_poll};
pub use ahci_dma::{AhciDmaEngine, AHCI_DMA, ahci_dma_init, ahci_dma_read, ahci_dma_write, ahci_dma_poll};
pub use nvme_dma::{NvmeDmaEngine, NVME_DMA, nvme_dma_init, nvme_read, nvme_write, nvme_poll};
pub use virtio_dma::{VirtioDmaEngine, VIRTIO_DMA, virtio_dma_init, virtio_dma_submit, virtio_dma_poll};

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
