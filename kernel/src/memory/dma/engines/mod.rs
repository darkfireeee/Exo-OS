// kernel/src/memory/dma/engines/mod.rs
//
// Engines module — drivers for hardware DMA engines.
//
//   ioat      : Intel I/OAT (Xeon series)
//   idxd      : Intel DSA/IAX (Data Streaming Accelerator)
//   ahci_dma  : SATA AHCI PRDT
//   nvme_dma  : NVMe PCIe queues
//   virtio_dma: VirtIO virtqueue

pub mod ahci_dma;
pub mod idxd;
pub mod ioat;
pub mod nvme_dma;
pub mod virtio_dma;

// Main re-exports
pub use ahci_dma::{
    ahci_dma_init, ahci_dma_poll, ahci_dma_read, ahci_dma_write, AhciDmaEngine, AHCI_DMA,
};
pub use idxd::{idxd_init, idxd_poll, idxd_submit, IdxdEngine, IDXD_ENGINE};
pub use ioat::{ioat_init, ioat_poll, ioat_submit, IoatEngine, IOAT_ENGINE};
pub use nvme_dma::{nvme_dma_init, nvme_poll, nvme_read, nvme_write, NvmeDmaEngine, NVME_DMA};
pub use virtio_dma::{
    virtio_dma_init, virtio_dma_poll, virtio_dma_submit, VirtioDmaEngine, VIRTIO_DMA,
};

/// Common trait for all hardware DMA engines.
pub trait DmaEngine: Send + Sync {
    /// Unique engine identifier (index in DMA_STATS).
    fn engine_id(&self) -> usize;
    /// Checks if the hardware is present/active.
    fn is_present(&self) -> bool;
    /// Initializes the engine (MMIO, rings, etc.).
    ///
    /// # Safety
    /// CPL 0, valid MMIO address.
    unsafe fn init(&self, mmio_base: u64) -> bool;
    /// Returns the number of available slots in the submission queue.
    fn available_slots(&self) -> usize;
}
