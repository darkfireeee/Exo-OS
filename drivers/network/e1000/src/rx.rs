use crate::regs::DESC_STATUS_DD;

pub const RX_RING_SIZE: usize = 256;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct RxDesc {
    pub addr: u64,
    pub length: u16,
    pub checksum: u16,
    pub status: u8,
    pub errors: u8,
    pub special: u16,
}

impl RxDesc {
    pub const fn empty() -> Self {
        Self {
            addr: 0,
            length: 0,
            checksum: 0,
            status: 0,
            errors: 0,
            special: 0,
        }
    }

    pub fn done(&self) -> bool {
        self.status & DESC_STATUS_DD != 0
    }
}

pub struct RxRing {
    pub desc: *mut RxDesc,
    pub iova: u64,
    pub virt: u64,
    pub head: usize,
}

impl RxRing {
    pub const fn empty() -> Self {
        Self {
            desc: core::ptr::null_mut(),
            iova: 0,
            virt: 0,
            head: 0,
        }
    }

    pub fn is_ready(&self) -> bool {
        !self.desc.is_null()
    }

    pub unsafe fn init(&mut self, virt: u64, iova: u64) {
        self.virt = virt;
        self.iova = iova;
        self.desc = virt as *mut RxDesc;
        self.head = 0;
        let mut idx = 0usize;
        while idx < RX_RING_SIZE {
            unsafe {
                core::ptr::write_volatile(self.desc.add(idx), RxDesc::empty());
            }
            idx += 1;
        }
    }

    pub unsafe fn set_buffer(&mut self, idx: usize, iova: u64) {
        if idx >= RX_RING_SIZE || self.desc.is_null() {
            return;
        }
        let desc = unsafe { &mut *self.desc.add(idx) };
        desc.addr = iova;
        desc.length = 0;
        desc.checksum = 0;
        desc.status = 0;
        desc.errors = 0;
        desc.special = 0;
    }

    pub unsafe fn poll_one(&mut self) -> Option<(u16, u16)> {
        if self.desc.is_null() {
            return None;
        }
        let idx = self.head & (RX_RING_SIZE - 1);
        let desc = unsafe { &mut *self.desc.add(idx) };
        if !desc.done() {
            return None;
        }
        let len = desc.length;
        desc.status = 0;
        self.head = (self.head + 1) & (RX_RING_SIZE - 1);
        Some((idx as u16, len))
    }
}
