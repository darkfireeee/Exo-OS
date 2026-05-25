use core::ptr::NonNull;

use spin::Mutex;
use virtio_drivers::{BufferDirection, Hal, PhysAddr, PAGE_SIZE};

/// Kernel-provided primitives used by the VirtIO HAL.
///
/// The storage driver crate stays independent from the kernel crate, so the
/// kernel installs these callbacks during ExoFS storage initialization. Without
/// installed callbacks the HAL fails closed instead of pretending that heap
/// virtual addresses are device-visible physical addresses.
#[derive(Clone, Copy)]
pub struct ExoHalOps {
    pub dma_alloc: fn(pages: usize) -> Option<(PhysAddr, NonNull<u8>)>,
    pub dma_dealloc: unsafe fn(paddr: PhysAddr, vaddr: NonNull<u8>, pages: usize) -> bool,
    pub mmio_phys_to_virt: unsafe fn(paddr: PhysAddr, size: usize) -> Option<NonNull<u8>>,
}

#[derive(Clone, Copy)]
struct BounceRecord {
    paddr: PhysAddr,
    vaddr: NonNull<u8>,
    pages: usize,
    len: usize,
    original: *mut u8,
    direction: BufferDirection,
}

// SAFETY: records are only accessed while held inside BOUNCE_TABLE's spinlock.
unsafe impl Send for BounceRecord {}

const MAX_BOUNCE_RECORDS: usize = 128;

static HAL_OPS: Mutex<Option<ExoHalOps>> = Mutex::new(None);
static BOUNCE_TABLE: Mutex<[Option<BounceRecord>; MAX_BOUNCE_RECORDS]> =
    Mutex::new([None; MAX_BOUNCE_RECORDS]);

pub struct ExoHal;

pub fn install_hal_ops(ops: ExoHalOps) {
    *HAL_OPS.lock() = Some(ops);
}

fn installed_ops() -> Option<ExoHalOps> {
    *HAL_OPS.lock()
}

fn pages_for_len(len: usize) -> Option<usize> {
    len.checked_add(PAGE_SIZE - 1).map(|n| n / PAGE_SIZE)
}

fn insert_bounce(record: BounceRecord) -> bool {
    let mut table = BOUNCE_TABLE.lock();
    if table
        .iter()
        .flatten()
        .any(|existing| existing.paddr == record.paddr)
    {
        return false;
    }
    let Some(slot) = table.iter_mut().find(|slot| slot.is_none()) else {
        return false;
    };
    *slot = Some(record);
    true
}

fn take_bounce(paddr: PhysAddr) -> Option<BounceRecord> {
    let mut table = BOUNCE_TABLE.lock();
    for slot in table.iter_mut() {
        if matches!(slot, Some(record) if record.paddr == paddr) {
            return slot.take();
        }
    }
    None
}

fn should_copy_to_device(direction: BufferDirection) -> bool {
    matches!(
        direction,
        BufferDirection::DriverToDevice | BufferDirection::Both
    )
}

fn should_copy_from_device(direction: BufferDirection) -> bool {
    matches!(
        direction,
        BufferDirection::DeviceToDriver | BufferDirection::Both
    )
}

unsafe impl Hal for ExoHal {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        if pages == 0 {
            return (0, NonNull::dangling());
        }
        let Some(ops) = installed_ops() else {
            return (0, NonNull::dangling());
        };
        (ops.dma_alloc)(pages).unwrap_or_else(|| (0, NonNull::dangling()))
    }

    unsafe fn dma_dealloc(paddr: PhysAddr, vaddr: NonNull<u8>, pages: usize) -> i32 {
        let Some(ops) = installed_ops() else {
            return -1;
        };
        if unsafe { (ops.dma_dealloc)(paddr, vaddr, pages) } {
            0
        } else {
            -1
        }
    }

    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, size: usize) -> NonNull<u8> {
        let Some(ops) = installed_ops() else {
            panic!("exo-virtio-blk HAL MMIO mapping requested before HAL install");
        };
        unsafe { (ops.mmio_phys_to_virt)(paddr, size) }
            .expect("exo-virtio-blk HAL rejected MMIO mapping")
    }

    unsafe fn share(buffer: NonNull<[u8]>, direction: BufferDirection) -> PhysAddr {
        let len = unsafe { buffer.as_ref().len() };
        if len == 0 {
            return 0;
        }
        let Some(pages) = pages_for_len(len) else {
            return 0;
        };
        let Some(ops) = installed_ops() else {
            return 0;
        };
        let Some((paddr, vaddr)) = (ops.dma_alloc)(pages) else {
            return 0;
        };
        if paddr == 0 {
            return 0;
        }

        let original = buffer.as_ptr() as *mut u8;
        if should_copy_to_device(direction) {
            unsafe {
                core::ptr::copy_nonoverlapping(original as *const u8, vaddr.as_ptr(), len);
            }
        }

        let record = BounceRecord {
            paddr,
            vaddr,
            pages,
            len,
            original,
            direction,
        };
        if !insert_bounce(record) {
            let _ = unsafe { (ops.dma_dealloc)(paddr, vaddr, pages) };
            return 0;
        }
        paddr
    }

    unsafe fn unshare(paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {
        let Some(record) = take_bounce(paddr) else {
            return;
        };
        if should_copy_from_device(record.direction) {
            unsafe {
                core::ptr::copy_nonoverlapping(record.vaddr.as_ptr(), record.original, record.len);
            }
        }
        if let Some(ops) = installed_ops() {
            let _ = unsafe { (ops.dma_dealloc)(record.paddr, record.vaddr, record.pages) };
        }
    }
}

#[cfg(test)]
pub fn clear_hal_state_for_test() {
    *HAL_OPS.lock() = None;
    *BOUNCE_TABLE.lock() = [None; MAX_BOUNCE_RECORDS];
}
