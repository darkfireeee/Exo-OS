use alloc::vec::Vec;
use exo_virtio_blk::ExoVirtioBlkDevice;
use crate::fs::exofs::recovery::boot_recovery::BlockDevice;
use crate::fs::exofs::core::ExofsResult;
use spin::Mutex;

pub struct VirtioBlockAdapter {
    pub device: Mutex<ExoVirtioBlkDevice>,
}

impl VirtioBlockAdapter {
    pub fn new(base_address: usize, capacity: usize) -> Self {
        Self {
            device: Mutex::new(ExoVirtioBlkDevice::new(base_address, capacity)),
        }
    }
}

impl BlockDevice for VirtioBlockAdapter {
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> ExofsResult<()> {
        let dev = self.device.lock();
        dev.read_block(lba, buf).map_err(|_| crate::fs::exofs::core::error::ExofsError::IoError)
    }

    fn write_block(&self, lba: u64, buf: &[u8]) -> ExofsResult<()> {
        let dev = self.device.lock();
        dev.write_block(lba, buf).map_err(|_| crate::fs::exofs::core::error::ExofsError::IoError)
    }

    fn block_size(&self) -> u32 {
        self.device.lock().block_size()
    }

    fn total_blocks(&self) -> u64 {
        // En simulant une capacité statique
        1024 * 1024 
    }

    fn flush(&self) -> ExofsResult<()> {
        Ok(())
    }
}

pub static GLOBAL_DISK: Mutex<Option<VirtioBlockAdapter>> = Mutex::new(None);

pub fn init_global_disk() {
    let mut disk = GLOBAL_DISK.lock();
    *disk = Some(VirtioBlockAdapter::new(0x1000_0000, 1024 * 1024 * 512));
}
