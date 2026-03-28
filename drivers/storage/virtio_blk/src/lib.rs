#![no_std]

extern crate alloc;
pub mod hal;

use alloc::vec::Vec;
use spin::Mutex;

// ExoHal is a hardware abstraction layer we'll need for virtio-drivers
// But since this is a complex integration, let's create a simpler abstract API 
// that the kernel will consume.

pub struct ExoVirtioBlkDevice {
    // Dans un vrai OS, on conserverait l'instance virtio_drivers::device::blk::VirtIOBlk<HalImpl, TransportImpl>
    // Pour l'intégration initiale, nous mockons la file de messages Pci / MMIO pour valider le VFS.
    pub base_address: usize,
    internal_storage: Mutex<Vec<u8>>, // Mock "Disque QEMU"
}

impl ExoVirtioBlkDevice {
    pub fn new(base_address: usize, disk_capacity_bytes: usize) -> Self {
        // Init MMIO ou PCI ici
        Self {
            base_address,
            internal_storage: Mutex::new(alloc::vec![0; disk_capacity_bytes]),
        }
    }

    pub fn block_size(&self) -> u32 {
        4096 // Taille typique d'un bloc virtio-blk (ou 512)
    }

    pub fn read_block(&self, block_id: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let bs = self.block_size() as usize;
        if buf.len() != bs {
            return Err("Buffer invalide");
        }
        let offset = (block_id as usize) * bs;
        
        // Simuler un accès DMA via VirtQueue
        let storage = self.internal_storage.lock();
        if offset + bs > storage.len() {
            return Err("Out of bounds");
        }
        buf.copy_from_slice(&storage[offset..offset+bs]);
        Ok(())
    }

    pub fn write_block(&self, block_id: u64, buf: &[u8]) -> Result<(), &'static str> {
        let bs = self.block_size() as usize;
        if buf.len() != bs {
            return Err("Buffer invalide");
        }
        let offset = (block_id as usize) * bs;
        
        let mut storage = self.internal_storage.lock();
        if offset + bs > storage.len() {
            return Err("Out of bounds");
        }
        // Écriture via DMA VirtQueue
        storage[offset..offset+bs].copy_from_slice(buf);
        Ok(())
    }
}
