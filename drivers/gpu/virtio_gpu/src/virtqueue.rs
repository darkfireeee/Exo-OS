//! Split-ring Virtqueue DMA engine for VirtIO-GPU `controlq` (0) and `cursorq` (1).

use core::sync::atomic::{fence, Ordering};
use exo_syscall_abi as syscall;

use crate::pci::{
    read16, write16, write64, COMMON_QUEUE_DESC, COMMON_QUEUE_DEVICE, COMMON_QUEUE_DRIVER,
    COMMON_QUEUE_ENABLE, COMMON_QUEUE_NOTIFY_OFF, COMMON_QUEUE_SELECT, COMMON_QUEUE_SIZE,
};

pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;

const PAGE_SIZE: usize = 4096;
const MAX_Q_SIZE: usize = 64;

#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtqDesc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

#[repr(C)]
pub struct VirtqAvail {
    pub flags: u16,
    pub idx: u16,
    pub ring: [u16; MAX_Q_SIZE],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtqUsedElem {
    pub id: u32,
    pub len: u32,
}

#[repr(C)]
pub struct VirtqUsed {
    pub flags: u16,
    pub idx: u16,
    pub ring: [VirtqUsedElem; MAX_Q_SIZE],
}

#[derive(Clone, Copy)]
pub struct GpuVirtqueue {
    pub queue_idx: u16,
    pub queue_size: u16,
    pub phys_base: u64,
    pub desc: *mut VirtqDesc,
    pub avail: *mut VirtqAvail,
    pub used: *mut VirtqUsed,
    pub notify_addr: *mut u8,
    pub last_used_idx: u16,
}

impl GpuVirtqueue {
    pub const fn empty() -> Self {
        Self {
            queue_idx: 0,
            queue_size: 0,
            phys_base: 0,
            desc: core::ptr::null_mut(),
            avail: core::ptr::null_mut(),
            used: core::ptr::null_mut(),
            notify_addr: core::ptr::null_mut(),
            last_used_idx: 0,
        }
    }

    pub fn is_ready(&self) -> bool {
        !self.desc.is_null() && !self.notify_addr.is_null() && self.queue_size >= 4
    }

    pub unsafe fn init_and_bind(
        common_cfg: *mut u8,
        notify_cfg: *mut u8,
        notify_off_multiplier: u32,
        queue_idx: u16,
    ) -> Result<Self, i64> {
        unsafe { write16(common_cfg, COMMON_QUEUE_SELECT, queue_idx) };
        let max_size = unsafe { read16(common_cfg, COMMON_QUEUE_SIZE) };
        if max_size < 4 {
            return Err(syscall::ENODEV);
        }
        let qsz = max_size.min(MAX_Q_SIZE as u16);
        // Ensure power-of-two size
        let mut queue_size = 4u16;
        while queue_size * 2 <= qsz {
            queue_size *= 2;
        }

        let desc_bytes = (queue_size as usize) * core::mem::size_of::<VirtqDesc>();
        // VirtIO 1.2 §2.7: flags(2) + idx(2) + ring[queue_size](2*N) + used_event(2)
        let avail_bytes = 6 + (queue_size as usize) * 2;
        let used_off = align_up(desc_bytes + avail_bytes, 4);
        let used_bytes = 6 + (queue_size as usize) * core::mem::size_of::<VirtqUsedElem>();
        let total = align_up(used_off + used_bytes, PAGE_SIZE);

        let mut virt: u64 = 0;
        let iova = unsafe {
            syscall::syscall5(
                syscall::SYS_DMA_ALLOC,
                total as u64,
                2, // DMA_BIDIRECTIONAL
                &mut virt as *mut u64 as u64,
                0,
                0,
            )
        };
        if iova <= 0 || virt == 0 {
            return Err(syscall::ENOMEM);
        }
        let base = virt as *mut u8;
        unsafe { core::ptr::write_bytes(base, 0, total) };

        let phys_base = iova as u64;
        let desc = base as *mut VirtqDesc;
        let avail = unsafe { base.add(desc_bytes) } as *mut VirtqAvail;
        let used = unsafe { base.add(used_off) } as *mut VirtqUsed;
        let avail_phys = phys_base + desc_bytes as u64;
        let used_phys = phys_base + used_off as u64;

        unsafe {
            // Suppress PCI INTx assertions — virtio_gpu polls controlq/cursorq synchronously
            core::ptr::write_volatile(&mut (*avail).flags, 1);
            write16(common_cfg, COMMON_QUEUE_SIZE, queue_size);
            write64(common_cfg, COMMON_QUEUE_DESC, phys_base);
            write64(common_cfg, COMMON_QUEUE_DRIVER, avail_phys);
            write64(common_cfg, COMMON_QUEUE_DEVICE, used_phys);
            let notify_off = read16(common_cfg, COMMON_QUEUE_NOTIFY_OFF) as usize;
            write16(common_cfg, COMMON_QUEUE_ENABLE, 1);
            let notify_addr = notify_cfg.add(notify_off * (notify_off_multiplier as usize));
            Ok(Self {
                queue_idx,
                queue_size,
                phys_base,
                desc,
                avail,
                used,
                notify_addr,
                last_used_idx: 0,
            })
        }
    }

    /// Submit a descriptor chain synchronously (`bufs: &[(phys_addr, len, flags)]`) and wait for completion.
    pub unsafe fn submit_sync(&mut self, bufs: &[(u64, u32, u16)], spin_limit: usize) -> Result<u32, i64> {
        if !self.is_ready() || bufs.is_empty() || bufs.len() > self.queue_size as usize {
            return Err(syscall::EINVAL);
        }
        for (i, &(addr, len, extra_flags)) in bufs.iter().enumerate() {
            let mut flags = extra_flags;
            let next = if i + 1 < bufs.len() {
                flags |= VIRTQ_DESC_F_NEXT;
                (i + 1) as u16
            } else {
                0
            };
            unsafe {
                core::ptr::write_volatile(
                    self.desc.add(i),
                    VirtqDesc { addr, len, flags, next },
                );
            }
        }
        let avail_idx = unsafe { core::ptr::read_volatile(&(*self.avail).idx) };
        let slot = (avail_idx % self.queue_size) as usize;
        unsafe {
            core::ptr::write_volatile(&mut (*self.avail).ring[slot], 0);
            fence(Ordering::Release);
            core::ptr::write_volatile(&mut (*self.avail).idx, avail_idx.wrapping_add(1));
            fence(Ordering::Release);
            core::ptr::write_volatile(self.notify_addr as *mut u16, self.queue_idx);
        }

        for iter in 0..spin_limit {
            let used_idx = unsafe { core::ptr::read_volatile(&(*self.used).idx) };
            fence(Ordering::Acquire);
            if used_idx != self.last_used_idx {
                let elem = unsafe {
                    core::ptr::read_volatile(
                        &(*self.used).ring[(self.last_used_idx % self.queue_size) as usize],
                    )
                };
                self.last_used_idx = self.last_used_idx.wrapping_add(1);
                return Ok(elem.len);
            }
            if iter < 4_000 {
                core::hint::spin_loop();
            } else {
                // Yield CPU so QEMU's VirtIO-GPU BH / main-loop thread can complete controlq I/O
                #[repr(C)]
                struct Timespec {
                    tv_sec: i64,
                    tv_nsec: i64,
                }
                let ts = Timespec { tv_sec: 0, tv_nsec: 1_000_000 };
                unsafe {
                    let _ = syscall::syscall2(
                        syscall::SYS_NANOSLEEP,
                        &ts as *const Timespec as u64,
                        0,
                    );
                }
                if iter >= 4_250 {
                    break;
                }
            }
        }
        Err(syscall::ETIMEDOUT)
    }
}

#[inline]
const fn align_up(v: usize, a: usize) -> usize {
    (v + a - 1) & !(a - 1)
}
