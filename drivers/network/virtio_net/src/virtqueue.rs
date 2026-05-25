use core::sync::atomic::{fence, Ordering};

use exo_syscall_abi as syscall;

use crate::config::PAGE_SIZE;

pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;
// VirtIO queue addresses stay physical until translated IOMMU contexts are
// attached and programmed for Ring1 drivers.
const DMA_MAP_FLAGS_BYPASS_IOMMU: u64 = 1 << 4;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
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
    pub ring: [u16; 256],
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
    pub ring: [VirtqUsedElem; 256],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Dma,
    Empty,
    Full,
    Invalid,
}

pub struct Virtqueue {
    pub phys_base: u64,
    pub virt_base: *mut u8,
    pub queue_size: u16,
    pub desc: *mut VirtqDesc,
    pub avail: *mut VirtqAvail,
    pub used: *mut VirtqUsed,
    pub free_head: u16,
    pub free_count: u16,
    pub last_used_idx: u16,
}

impl Virtqueue {
    pub const fn empty() -> Self {
        Self {
            phys_base: 0,
            virt_base: core::ptr::null_mut(),
            queue_size: 0,
            desc: core::ptr::null_mut(),
            avail: core::ptr::null_mut(),
            used: core::ptr::null_mut(),
            free_head: 0,
            free_count: 0,
            last_used_idx: 0,
        }
    }

    pub fn init(queue_size: u16) -> Result<Self, Error> {
        if queue_size == 0 || queue_size as usize > 256 || !queue_size.is_power_of_two() {
            return Err(Error::Invalid);
        }
        let avail_offset = queue_size as usize * core::mem::size_of::<VirtqDesc>();
        let used_offset = used_offset(queue_size);
        let bytes = align_up(used_offset + core::mem::size_of::<VirtqUsed>(), PAGE_SIZE);
        let mut virt = 0u64;
        let iova = unsafe {
            syscall::syscall5(
                syscall::SYS_DMA_ALLOC,
                bytes as u64,
                2,
                &mut virt as *mut u64 as u64,
                DMA_MAP_FLAGS_BYPASS_IOMMU,
                0,
            )
        };
        if iova < 0 || virt == 0 {
            return Err(Error::Dma);
        }
        let base = virt as *mut u8;
        unsafe {
            core::ptr::write_bytes(base, 0, bytes);
        }
        let desc = base as *mut VirtqDesc;
        let avail = unsafe { base.add(avail_offset) } as *mut VirtqAvail;
        let used = unsafe { base.add(used_offset) } as *mut VirtqUsed;
        let mut idx = 0u16;
        while idx < queue_size {
            unsafe {
                core::ptr::write_volatile(
                    desc.add(idx as usize),
                    VirtqDesc {
                        next: idx + 1,
                        ..VirtqDesc::default()
                    },
                );
            }
            idx += 1;
        }
        Ok(Self {
            phys_base: iova as u64,
            virt_base: base,
            queue_size,
            desc,
            avail,
            used,
            free_head: 0,
            free_count: queue_size,
            last_used_idx: 0,
        })
    }

    pub unsafe fn add_chain(&mut self, bufs: &[(u64, u32, u16)]) -> Result<u16, Error> {
        if bufs.is_empty() || bufs.len() > self.free_count as usize {
            return Err(Error::Full);
        }
        let head = self.free_head;
        let mut prev = head;
        let mut i = 0usize;
        while i < bufs.len() {
            let id = self.free_head;
            let desc = unsafe { self.desc.add(id as usize) };
            let next_free = unsafe { core::ptr::read_volatile(desc).next };
            self.free_head = next_free;
            self.free_count -= 1;
            let mut flags = bufs[i].2;
            if i + 1 < bufs.len() {
                flags |= VIRTQ_DESC_F_NEXT;
            }
            if i != 0 {
                unsafe {
                    core::ptr::write_volatile(&mut (*self.desc.add(prev as usize)).next, id);
                }
            }
            unsafe {
                core::ptr::write_volatile(
                    desc,
                    VirtqDesc {
                        addr: bufs[i].0,
                        len: bufs[i].1,
                        flags,
                        next: 0,
                    },
                );
            }
            prev = id;
            i += 1;
        }
        let avail_idx = unsafe { core::ptr::read_volatile(&(*self.avail).idx) };
        let slot = (avail_idx % self.queue_size) as usize;
        unsafe {
            core::ptr::write_volatile(&mut (*self.avail).ring[slot], head);
        }
        fence(Ordering::Release);
        unsafe {
            core::ptr::write_volatile(&mut (*self.avail).idx, avail_idx.wrapping_add(1));
        }
        Ok(head)
    }

    pub fn avail_phys(&self) -> u64 {
        self.phys_base + (self.queue_size as usize * core::mem::size_of::<VirtqDesc>()) as u64
    }

    pub fn used_phys(&self) -> u64 {
        self.phys_base + used_offset(self.queue_size) as u64
    }

    pub unsafe fn notify(notify: *mut u8, queue_idx: u16) {
        fence(Ordering::Release);
        unsafe {
            core::ptr::write_volatile(notify as *mut u16, queue_idx);
        }
    }

    pub unsafe fn poll_used(&mut self) -> Option<(u16, u32)> {
        let used_idx = unsafe { core::ptr::read_volatile(&(*self.used).idx) };
        fence(Ordering::Acquire);
        if self.last_used_idx == used_idx {
            return None;
        }
        let elem = unsafe {
            core::ptr::read_volatile(
                &(*self.used).ring[(self.last_used_idx % self.queue_size) as usize],
            )
        };
        self.last_used_idx = self.last_used_idx.wrapping_add(1);
        Some((elem.id as u16, elem.len))
    }

    pub unsafe fn recycle_desc(&mut self, head: u16) {
        if self.desc.is_null() || head >= self.queue_size {
            return;
        }
        let mut idx = head;
        loop {
            let desc = unsafe { self.desc.add(idx as usize) };
            let snapshot = unsafe { core::ptr::read_volatile(desc) };
            let flags = snapshot.flags;
            let next = snapshot.next;
            unsafe {
                core::ptr::write_volatile(
                    desc,
                    VirtqDesc {
                        addr: 0,
                        len: 0,
                        flags: 0,
                        next: self.free_head,
                    },
                );
            }
            self.free_head = idx;
            self.free_count = self.free_count.saturating_add(1).min(self.queue_size);
            if flags & VIRTQ_DESC_F_NEXT == 0 || next >= self.queue_size {
                break;
            }
            idx = next;
        }
    }
}

fn used_offset(queue_size: u16) -> usize {
    let desc_bytes = queue_size as usize * core::mem::size_of::<VirtqDesc>();
    align_up(desc_bytes + core::mem::size_of::<VirtqAvail>(), 4)
}

fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}
