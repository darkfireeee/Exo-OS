//! # fs/exofs_hal — ExoFS Hardware & Platform Adapters (Ring 0)
//!
//! Isolates all platform-specific storage backends (VirtIO-Blk, ATA PIO,
//! Multiboot2 ramdisk) and x86_64 platform hooks (boot trace, TSC clock, RDRAND)
//! outside `kernel/src/fs/exofs/` so ExoFS core contains zero inline assembly
//! and zero direct hardware port/MMIO/PCI logic.

pub mod ata_pio;
pub mod ramdisk;
pub mod virtio_adapter;
pub mod x86_platform;
