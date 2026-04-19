use exo_virtio_blk::ExoVirtioBlkDevice;
use crate::fs::exofs::recovery::boot_recovery::BlockDevice;
use crate::fs::exofs::core::ExofsResult;
use spin::Mutex;

pub const DEFAULT_GLOBAL_DISK_BASE_ADDRESS: usize = 0x1000_0000;
pub const DEFAULT_GLOBAL_DISK_CAPACITY_BYTES: u64 = 1024 * 1024 * 512;

pub struct VirtioBlockAdapter {
    pub device: Mutex<ExoVirtioBlkDevice>,
    capacity_bytes: u64,
}

impl VirtioBlockAdapter {
    pub fn new(base_address: usize, capacity: usize) -> Self {
        Self {
            device: Mutex::new(ExoVirtioBlkDevice::new(base_address, capacity)),
            capacity_bytes: capacity as u64,
        }
    }

    pub fn size_bytes(&self) -> u64 {
        self.capacity_bytes
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
        let block_size = self.device.lock().block_size() as u64;
        if block_size == 0 {
            return 0;
        }
        self.capacity_bytes / block_size
    }

    fn flush(&self) -> ExofsResult<()> {
        let dev = self.device.lock();
        dev.flush().map_err(|_| crate::fs::exofs::core::error::ExofsError::NvmeFlushFailed)
    }
}

pub static GLOBAL_DISK: Mutex<Option<VirtioBlockAdapter>> = Mutex::new(None);

pub fn init_global_disk() {
    let mut disk = GLOBAL_DISK.lock();
    *disk = Some(VirtioBlockAdapter::new(
        DEFAULT_GLOBAL_DISK_BASE_ADDRESS,
        DEFAULT_GLOBAL_DISK_CAPACITY_BYTES as usize,
    ));
}

pub fn default_global_disk_size_bytes() -> u64 {
    DEFAULT_GLOBAL_DISK_CAPACITY_BYTES
}

pub fn global_disk_size_bytes() -> Option<u64> {
    GLOBAL_DISK.lock().as_ref().map(|disk| disk.size_bytes())
}

pub fn flush_global_disk() -> ExofsResult<()> {
    let disk = GLOBAL_DISK.lock();
    let disk = disk
        .as_ref()
        .ok_or(crate::fs::exofs::core::error::ExofsError::InvalidState)?;
    disk.flush()
}
