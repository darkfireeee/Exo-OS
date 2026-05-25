extern crate alloc;

use crate::fs::exofs::core::DiskOffset;
use crate::fs::exofs::core::ExofsError;
use crate::fs::exofs::core::ExofsResult;
use crate::fs::exofs::recovery::boot_recovery::BlockDevice;
use crate::memory::core::{Frame, PageFlags, PhysAddr, VirtAddr, PAGE_SIZE};
use crate::memory::physical::allocator::{alloc_pages, free_pages};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};
use exo_virtio_blk::hal::{install_hal_ops, ExoHalOps};
use exo_virtio_blk::ExoVirtioBlkDevice;
use spin::Mutex;

pub const DEFAULT_VIRTIO_BLK_CAPACITY_BYTES: usize = 512 * 1024 * 1024;

pub struct VirtioBlockAdapter {
    pub device: Mutex<ExoVirtioBlkDevice>,
}

impl VirtioBlockAdapter {
    pub fn new_legacy_pci(io_base: u16) -> ExofsResult<Self> {
        let device =
            ExoVirtioBlkDevice::new_legacy_pci(io_base).map_err(|_| ExofsError::IoError)?;
        Ok(Self {
            device: Mutex::new(device),
        })
    }
}

impl BlockDevice for VirtioBlockAdapter {
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> ExofsResult<()> {
        let mut dev = self.device.lock();
        dev.read_block(lba, buf)
            .map_err(|_| crate::fs::exofs::core::error::ExofsError::IoError)
    }

    fn write_block(&self, lba: u64, buf: &[u8]) -> ExofsResult<()> {
        let mut dev = self.device.lock();
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
    crate::fs::exofs::epoch::epoch_barriers::register_nvme_flush_fn(flush_global_disk);
    true
}

static VIRTIO_HAL_INSTALLED: AtomicBool = AtomicBool::new(false);

fn pages_to_order(pages: usize) -> Option<usize> {
    if pages == 0 {
        return None;
    }
    let mut order = 0usize;
    let mut covered = 1usize;
    while covered < pages {
        covered = covered.checked_shl(1)?;
        order = order.checked_add(1)?;
    }
    Some(order)
}

fn kernel_dma_alloc(pages: usize) -> Option<(usize, NonNull<u8>)> {
    let order = pages_to_order(pages)?;
    let frame = alloc_pages(
        order,
        crate::memory::core::AllocFlags::DMA32
            | crate::memory::core::AllocFlags::ZEROED
            | crate::memory::core::AllocFlags::PIN,
    )
    .ok()?;
    let phys = frame.start_address();
    let virt = crate::memory::core::phys_to_virt(phys);
    Some((
        phys.as_u64() as usize,
        NonNull::new(virt.as_u64() as *mut u8)?,
    ))
}

unsafe fn kernel_dma_dealloc(paddr: usize, _vaddr: NonNull<u8>, pages: usize) -> bool {
    let Some(order) = pages_to_order(pages) else {
        return false;
    };
    let frame = Frame::containing(PhysAddr::new(paddr as u64));
    free_pages(frame, order).is_ok()
}

unsafe fn kernel_mmio_phys_to_virt(paddr: usize, size: usize) -> Option<NonNull<u8>> {
    use crate::arch::x86_64::memory_iface::KERNEL_FAULT_ALLOC;
    use crate::memory::virt::address_space::kernel::KERNEL_AS;

    if size == 0 {
        return None;
    }
    let page_mask = PAGE_SIZE as u64 - 1;
    let phys = PhysAddr::new(paddr as u64);
    let page_phys = PhysAddr::new(phys.as_u64() & !page_mask);
    let page_off = (phys.as_u64() & page_mask) as usize;
    let map_size = page_off.checked_add(size)?;
    let pages = map_size.checked_add(PAGE_SIZE - 1)? / PAGE_SIZE;
    let virt_base = KERNEL_AS.reserve_vmalloc_pages(pages).ok()?;
    let flags = PageFlags::KERNEL_DMA | PageFlags::WRITE_THROUGH;

    for page_idx in 0..pages {
        let virt = VirtAddr::new(virt_base.as_u64() + (page_idx * PAGE_SIZE) as u64);
        let frame = Frame::containing(PhysAddr::new(
            page_phys.as_u64() + (page_idx * PAGE_SIZE) as u64,
        ));
        if unsafe { KERNEL_AS.map(virt, frame, flags, &KERNEL_FAULT_ALLOC) }.is_err() {
            return None;
        }
    }

    let virt = virt_base.as_u64().checked_add(page_off as u64)?;
    NonNull::new(virt as *mut u8)
}

fn install_kernel_hal_once() {
    if VIRTIO_HAL_INSTALLED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        install_hal_ops(ExoHalOps {
            dma_alloc: kernel_dma_alloc,
            dma_dealloc: kernel_dma_dealloc,
            mmio_phys_to_virt: kernel_mmio_phys_to_virt,
        });
    }
}

pub fn init_global_disk_with_legacy_pci(io_base: u16) -> ExofsResult<bool> {
    install_kernel_hal_once();
    let adapter = VirtioBlockAdapter::new_legacy_pci(io_base)?;
    Ok(register_global_disk(Arc::new(adapter)))
}

pub fn init_global_disk() {
    let Some(io_base) = crate::drivers::find_virtio_blk_legacy_io_port() else {
        return;
    };
    let _ = init_global_disk_with_legacy_pci(io_base);
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

pub fn write_at_offset(offset: DiskOffset, data: &[u8]) -> ExofsResult<usize> {
    if data.is_empty() {
        return Ok(0);
    }
    with_global_disk(|device| {
        let block_size = device.block_size() as usize;
        if block_size == 0 {
            return Err(ExofsError::InvalidSize);
        }
        let start_lba = offset.0 / block_size as u64;
        let block_off = (offset.0 % block_size as u64) as usize;
        let mut written = 0usize;
        let mut lba = start_lba;
        let mut block = Vec::new();
        block
            .try_reserve_exact(block_size)
            .map_err(|_| ExofsError::NoMemory)?;
        block.resize(block_size, 0);

        while written < data.len() {
            device.read_block(lba, &mut block)?;
            let off = if written == 0 { block_off } else { 0 };
            let chunk = core::cmp::min(block_size.saturating_sub(off), data.len() - written);
            block[off..off + chunk].copy_from_slice(&data[written..written + chunk]);
            device.write_block(lba, &block)?;
            written = written.saturating_add(chunk);
            lba = lba.saturating_add(1);
        }
        Ok(written)
    })
}

pub fn read_at_offset(offset: DiskOffset, len: usize) -> ExofsResult<Vec<u8>> {
    let mut out = Vec::new();
    if len == 0 {
        return Ok(out);
    }
    with_global_disk(|device| {
        let block_size = device.block_size() as usize;
        if block_size == 0 {
            return Err(ExofsError::InvalidSize);
        }
        out.try_reserve(len).map_err(|_| ExofsError::NoMemory)?;
        let start_lba = offset.0 / block_size as u64;
        let block_off = (offset.0 % block_size as u64) as usize;
        let mut read = 0usize;
        let mut lba = start_lba;
        let mut block = Vec::new();
        block
            .try_reserve_exact(block_size)
            .map_err(|_| ExofsError::NoMemory)?;
        block.resize(block_size, 0);

        while read < len {
            device.read_block(lba, &mut block)?;
            let off = if read == 0 { block_off } else { 0 };
            let chunk = core::cmp::min(block_size.saturating_sub(off), len - read);
            out.extend_from_slice(&block[off..off + chunk]);
            read = read.saturating_add(chunk);
            lba = lba.saturating_add(1);
        }
        Ok(out)
    })
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
