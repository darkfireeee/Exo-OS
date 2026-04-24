#![no_std]

extern crate alloc;
pub mod hal;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use spin::Mutex;

// ExoHal is a hardware abstraction layer we'll need for virtio-drivers
// But since this is a complex integration, let's create a simpler abstract API
// that the kernel will consume.

pub struct ExoVirtioBlkDevice {
    // Dans un vrai OS, on conserverait l'instance virtio_drivers::device::blk::VirtIOBlk<HalImpl, TransportImpl>
    // Pour l'intégration initiale, nous mockons la file de messages Pci / MMIO pour valider le VFS.
    pub base_address: usize,
    capacity_bytes: usize,
    block_size: usize,
    internal_storage: Mutex<BTreeMap<u64, Box<[u8]>>>, // Backend bloc sparse
}

impl ExoVirtioBlkDevice {
    pub fn new(base_address: usize, disk_capacity_bytes: usize) -> Self {
        // Init MMIO ou PCI ici
        Self {
            base_address,
            capacity_bytes: disk_capacity_bytes,
            block_size: 4096,
            internal_storage: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn block_size(&self) -> u32 {
        self.block_size as u32
    }

    pub fn total_blocks(&self) -> u64 {
        (self.capacity_bytes / self.block_size) as u64
    }

    pub fn read_block(&self, block_id: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let bs = self.block_size;
        if buf.len() != bs {
            return Err("Buffer invalide");
        }
        let offset = (block_id as usize).checked_mul(bs).ok_or("Out of bounds")?;

        let storage = self.internal_storage.lock();
        if offset.checked_add(bs).ok_or("Out of bounds")? > self.capacity_bytes {
            return Err("Out of bounds");
        }

        if let Some(block) = storage.get(&block_id) {
            buf.copy_from_slice(block);
        } else {
            buf.fill(0);
        }
        Ok(())
    }

    pub fn write_block(&self, block_id: u64, buf: &[u8]) -> Result<(), &'static str> {
        let bs = self.block_size;
        if buf.len() != bs {
            return Err("Buffer invalide");
        }
        let offset = (block_id as usize).checked_mul(bs).ok_or("Out of bounds")?;

        let mut storage = self.internal_storage.lock();
        if offset.checked_add(bs).ok_or("Out of bounds")? > self.capacity_bytes {
            return Err("Out of bounds");
        }

        let block = storage
            .entry(block_id)
            .or_insert_with(|| alloc::vec![0u8; bs].into_boxed_slice());
        block.copy_from_slice(buf);
        Ok(())
    }

    /// Flush persistant du backend bloc.
    ///
    /// Le backend actuel est un disque mock en mémoire, donc les écritures sont
    /// déjà visibles de manière synchrone. La fonction reste explicite pour que
    /// les couches supérieures disposent d'un vrai point d'accroche de flush.
    pub fn flush(&self) -> Result<(), &'static str> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ExoVirtioBlkDevice;

    #[test]
    fn sparse_backend_reads_zero_for_unwritten_blocks() {
        let dev = ExoVirtioBlkDevice::new(0x1000_0000, 16 * 4096);
        let mut buf = [0xAA; 4096];
        dev.read_block(3, &mut buf).unwrap();
        assert!(buf.iter().all(|&b| b == 0));
    }

    #[test]
    fn sparse_backend_roundtrip_persists_written_block() {
        let dev = ExoVirtioBlkDevice::new(0x1000_0000, 16 * 4096);
        let mut write = [0u8; 4096];
        for (i, byte) in write.iter_mut().enumerate() {
            *byte = (i & 0xFF) as u8;
        }
        dev.write_block(2, &write).unwrap();

        let mut read = [0u8; 4096];
        dev.read_block(2, &mut read).unwrap();
        assert_eq!(read, write);
    }

    #[test]
    fn sparse_backend_stress_multiple_blocks_roundtrip() {
        const BLOCKS: u64 = 256;

        let dev = ExoVirtioBlkDevice::new(0x1000_0000, BLOCKS as usize * 4096);

        for block_id in 0..BLOCKS {
            let fill = (block_id & 0xFF) as u8;
            let write = [fill; 4096];
            dev.write_block(block_id, &write).unwrap();
        }

        for block_id in 0..BLOCKS {
            let fill = (block_id & 0xFF) as u8;
            let mut read = [0u8; 4096];
            dev.read_block(block_id, &mut read).unwrap();
            assert!(read.iter().all(|&b| b == fill));
        }
    }
}
