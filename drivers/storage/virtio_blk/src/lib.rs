#![no_std]

extern crate alloc;

pub mod hal;
pub mod legacy_pci;
pub mod virtqueue;

use hal::ExoHal;
use legacy_pci::LegacyPciTransport;
use virtio_drivers::device::blk::{VirtIOBlk, SECTOR_SIZE};
use virtio_drivers::Error as VirtioError;

const EXOFS_BLOCK_SIZE: usize = 4096;
const SECTORS_PER_EXOFS_BLOCK: u64 = (EXOFS_BLOCK_SIZE / SECTOR_SIZE) as u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExoVirtioBlkError {
    InvalidBuffer,
    OutOfBounds,
    ReadOnly,
    CapacityOverflow,
    Transport(VirtioError),
}

impl From<VirtioError> for ExoVirtioBlkError {
    fn from(value: VirtioError) -> Self {
        Self::Transport(value)
    }
}

enum Backend {
    LegacyPci(VirtIOBlk<ExoHal, LegacyPciTransport>),
    #[cfg(test)]
    Sparse(test_backend::SparseBackend),
}

pub struct ExoVirtioBlkDevice {
    backend: Backend,
}

impl ExoVirtioBlkDevice {
    pub fn new_legacy_pci(io_base: u16) -> Result<Self, ExoVirtioBlkError> {
        let transport = LegacyPciTransport::new(io_base);
        let block = VirtIOBlk::<ExoHal, _>::new(transport)?;
        if block.readonly() {
            return Err(ExoVirtioBlkError::ReadOnly);
        }
        Ok(Self {
            backend: Backend::LegacyPci(block),
        })
    }

    #[cfg(test)]
    pub fn new_test_sparse(base_address: usize, disk_capacity_bytes: usize) -> Self {
        Self {
            backend: Backend::Sparse(test_backend::SparseBackend::new(
                base_address,
                disk_capacity_bytes,
            )),
        }
    }

    pub fn block_size(&self) -> u32 {
        EXOFS_BLOCK_SIZE as u32
    }

    pub fn total_blocks(&self) -> u64 {
        match &self.backend {
            Backend::LegacyPci(block) => block.capacity() / SECTORS_PER_EXOFS_BLOCK,
            #[cfg(test)]
            Backend::Sparse(sparse) => sparse.total_blocks(),
        }
    }

    pub fn read_block(&mut self, block_id: u64, buf: &mut [u8]) -> Result<(), ExoVirtioBlkError> {
        if buf.len() != EXOFS_BLOCK_SIZE {
            return Err(ExoVirtioBlkError::InvalidBuffer);
        }
        if block_id >= self.total_blocks() {
            return Err(ExoVirtioBlkError::OutOfBounds);
        }
        match &mut self.backend {
            Backend::LegacyPci(block) => {
                let sector = block_id
                    .checked_mul(SECTORS_PER_EXOFS_BLOCK)
                    .ok_or(ExoVirtioBlkError::CapacityOverflow)?;
                let sector = usize::try_from(sector).map_err(|_| ExoVirtioBlkError::OutOfBounds)?;
                block.read_blocks(sector, buf)?;
                Ok(())
            }
            #[cfg(test)]
            Backend::Sparse(sparse) => sparse.read_block(block_id, buf),
        }
    }

    pub fn write_block(&mut self, block_id: u64, buf: &[u8]) -> Result<(), ExoVirtioBlkError> {
        if buf.len() != EXOFS_BLOCK_SIZE {
            return Err(ExoVirtioBlkError::InvalidBuffer);
        }
        if block_id >= self.total_blocks() {
            return Err(ExoVirtioBlkError::OutOfBounds);
        }
        match &mut self.backend {
            Backend::LegacyPci(block) => {
                let sector = block_id
                    .checked_mul(SECTORS_PER_EXOFS_BLOCK)
                    .ok_or(ExoVirtioBlkError::CapacityOverflow)?;
                let sector = usize::try_from(sector).map_err(|_| ExoVirtioBlkError::OutOfBounds)?;
                block.write_blocks(sector, buf)?;
                Ok(())
            }
            #[cfg(test)]
            Backend::Sparse(sparse) => sparse.write_block(block_id, buf),
        }
    }

    pub fn flush(&mut self) -> Result<(), ExoVirtioBlkError> {
        match &mut self.backend {
            Backend::LegacyPci(block) => {
                block.flush()?;
                Ok(())
            }
            #[cfg(test)]
            Backend::Sparse(_) => Ok(()),
        }
    }
}

#[cfg(test)]
mod test_backend {
    use super::{ExoVirtioBlkError, EXOFS_BLOCK_SIZE};
    use alloc::boxed::Box;
    use alloc::collections::BTreeMap;
    use spin::Mutex;

    pub struct SparseBackend {
        capacity_bytes: usize,
        block_size: usize,
        internal_storage: Mutex<BTreeMap<u64, Box<[u8]>>>,
        _base_address: usize,
    }

    impl SparseBackend {
        pub fn new(base_address: usize, disk_capacity_bytes: usize) -> Self {
            Self {
                capacity_bytes: disk_capacity_bytes,
                block_size: EXOFS_BLOCK_SIZE,
                internal_storage: Mutex::new(BTreeMap::new()),
                _base_address: base_address,
            }
        }

        pub fn total_blocks(&self) -> u64 {
            (self.capacity_bytes / self.block_size) as u64
        }

        pub fn read_block(
            &mut self,
            block_id: u64,
            buf: &mut [u8],
        ) -> Result<(), ExoVirtioBlkError> {
            let bs = self.block_size;
            if buf.len() != bs {
                return Err(ExoVirtioBlkError::InvalidBuffer);
            }
            let offset = (block_id as usize)
                .checked_mul(bs)
                .ok_or(ExoVirtioBlkError::OutOfBounds)?;

            let storage = self.internal_storage.lock();
            if offset
                .checked_add(bs)
                .ok_or(ExoVirtioBlkError::OutOfBounds)?
                > self.capacity_bytes
            {
                return Err(ExoVirtioBlkError::OutOfBounds);
            }

            if let Some(block) = storage.get(&block_id) {
                buf.copy_from_slice(block);
            } else {
                buf.fill(0);
            }
            Ok(())
        }

        pub fn write_block(&mut self, block_id: u64, buf: &[u8]) -> Result<(), ExoVirtioBlkError> {
            let bs = self.block_size;
            if buf.len() != bs {
                return Err(ExoVirtioBlkError::InvalidBuffer);
            }
            let offset = (block_id as usize)
                .checked_mul(bs)
                .ok_or(ExoVirtioBlkError::OutOfBounds)?;

            let mut storage = self.internal_storage.lock();
            if offset
                .checked_add(bs)
                .ok_or(ExoVirtioBlkError::OutOfBounds)?
                > self.capacity_bytes
            {
                return Err(ExoVirtioBlkError::OutOfBounds);
            }

            let block = storage
                .entry(block_id)
                .or_insert_with(|| alloc::vec![0u8; bs].into_boxed_slice());
            block.copy_from_slice(buf);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ExoVirtioBlkDevice;

    #[test]
    fn sparse_backend_reads_zero_for_unwritten_blocks() {
        let mut dev = ExoVirtioBlkDevice::new_test_sparse(0x1000_0000, 16 * 4096);
        let mut buf = [0xAA; 4096];
        dev.read_block(3, &mut buf).unwrap();
        assert!(buf.iter().all(|&b| b == 0));
    }

    #[test]
    fn sparse_backend_roundtrip_persists_written_block() {
        let mut dev = ExoVirtioBlkDevice::new_test_sparse(0x1000_0000, 16 * 4096);
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

        let mut dev = ExoVirtioBlkDevice::new_test_sparse(0x1000_0000, BLOCKS as usize * 4096);

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
