extern crate alloc;

use crate::fs::exofs::core::ExofsError;
use crate::fs::exofs::core::ExofsResult;
use crate::fs::exofs::recovery::boot_recovery::BlockDevice;
use alloc::sync::Arc;
use exo_virtio_blk::ExoVirtioBlkDevice;
use spin::Mutex;

pub const DEFAULT_VIRTIO_BLK_MMIO_BASE: usize = 0x1000_0000;
pub const DEFAULT_VIRTIO_BLK_CAPACITY_BYTES: usize = 512 * 1024 * 1024;

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
        dev.read_block(lba, buf)
            .map_err(|_| crate::fs::exofs::core::error::ExofsError::IoError)
    }

    fn write_block(&self, lba: u64, buf: &[u8]) -> ExofsResult<()> {
        let dev = self.device.lock();
        dev.write_block(lba, buf)
            .map_err(|_| crate::fs::exofs::core::error::ExofsError::IoError)
    }

    fn block_size(&self) -> u32 {
        self.device.lock().block_size()
    }

    fn total_blocks(&self) -> u64 {
        self.device.lock().total_blocks()
    }

    fn flush(&self) -> ExofsResult<()> {
        self.device
            .lock()
            .flush()
            .map_err(|_| crate::fs::exofs::core::error::ExofsError::IoError)
    }
}

pub static GLOBAL_DISK: Mutex<Option<Arc<dyn BlockDevice>>> = Mutex::new(None);

pub fn register_global_disk(device: Arc<dyn BlockDevice>) -> bool {
    let mut disk = GLOBAL_DISK.lock();
    if disk.is_some() {
        return false;
    }
    *disk = Some(device);
    true
}

pub fn init_global_disk_with_mmio(base_address: usize, capacity_bytes: usize) {
    let _ = register_global_disk(Arc::new(VirtioBlockAdapter::new(
        base_address,
        capacity_bytes,
    )));
}

pub fn init_global_disk() {
    init_global_disk_with_mmio(
        DEFAULT_VIRTIO_BLK_MMIO_BASE,
        DEFAULT_VIRTIO_BLK_CAPACITY_BYTES,
    );
}

pub fn has_global_disk() -> bool {
    GLOBAL_DISK.lock().is_some()
}

pub fn with_global_disk<T, F>(f: F) -> ExofsResult<T>
where
    F: FnOnce(&dyn BlockDevice) -> ExofsResult<T>,
{
    let disk = GLOBAL_DISK.lock();
    let device = disk.as_ref().ok_or(ExofsError::Resource)?;
    f(device.as_ref())
}

pub fn flush_global_disk() -> ExofsResult<()> {
    with_global_disk(|device| device.flush())
}

pub fn default_global_disk_size_bytes() -> u64 {
    GLOBAL_DISK
        .lock()
        .as_ref()
        .map(|disk| disk.total_blocks().saturating_mul(disk.block_size() as u64))
        .unwrap_or(DEFAULT_VIRTIO_BLK_CAPACITY_BYTES as u64)
}

#[cfg(test)]
pub fn set_global_disk_for_test(device: Arc<dyn BlockDevice>) {
    let mut disk = GLOBAL_DISK.lock();
    *disk = Some(device);
}

#[cfg(test)]
pub fn clear_global_disk_for_test() {
    let mut disk = GLOBAL_DISK.lock();
    *disk = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    struct CountingBlockDevice {
        flushes: AtomicUsize,
    }

    impl CountingBlockDevice {
        fn new() -> Self {
            Self {
                flushes: AtomicUsize::new(0),
            }
        }
    }

    impl BlockDevice for CountingBlockDevice {
        fn read_block(&self, _lba: u64, buf: &mut [u8]) -> ExofsResult<()> {
            buf.fill(0);
            Ok(())
        }

        fn write_block(&self, _lba: u64, _buf: &[u8]) -> ExofsResult<()> {
            Ok(())
        }

        fn block_size(&self) -> u32 {
            4096
        }

        fn total_blocks(&self) -> u64 {
            1024
        }

        fn flush(&self) -> ExofsResult<()> {
            self.flushes.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    #[test]
    fn flush_global_disk_delegates_to_registered_device() {
        let device = Arc::new(CountingBlockDevice::new());
        set_global_disk_for_test(device.clone());
        assert!(flush_global_disk().is_ok());
        assert_eq!(device.flushes.load(Ordering::Relaxed), 1);
        clear_global_disk_for_test();
    }
}
