use crate::regs::{DESC_STATUS_DD, TX_CMD_EOP, TX_CMD_IFCS, TX_CMD_RS};

pub const TX_RING_SIZE: usize = 256;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct TxDesc {
    pub addr: u64,
    pub length: u16,
    pub csum_offset: u8,
    pub cmd: u8,
    pub status: u8,
    pub csum_start: u8,
    pub special: u16,
}

impl TxDesc {
    pub const fn empty() -> Self {
        Self {
            addr: 0,
            length: 0,
            csum_offset: 0,
            cmd: 0,
            status: DESC_STATUS_DD,
            csum_start: 0,
            special: 0,
        }
    }
}

pub struct TxRing {
    pub desc: *mut TxDesc,
    pub iova: u64,
    pub virt: u64,
    pub tail: usize,
}

impl TxRing {
    pub const fn empty() -> Self {
        Self {
            desc: core::ptr::null_mut(),
            iova: 0,
            virt: 0,
            tail: 0,
        }
    }

    pub fn is_ready(&self) -> bool {
        !self.desc.is_null()
    }

    pub unsafe fn init(&mut self, virt: u64, iova: u64) {
        self.virt = virt;
        self.iova = iova;
        self.desc = virt as *mut TxDesc;
        self.tail = 0;
        let mut idx = 0usize;
        while idx < TX_RING_SIZE {
            unsafe {
                core::ptr::write_volatile(self.desc.add(idx), TxDesc::empty());
            }
            idx += 1;
        }
    }

    pub unsafe fn prepare(&mut self, addr: u64, len: u16) -> Option<usize> {
        if self.desc.is_null() {
            return None;
        }
        let idx = self.tail & (TX_RING_SIZE - 1);
        let desc = unsafe { &mut *self.desc.add(idx) };
        if desc.status & DESC_STATUS_DD == 0 {
            return None;
        }
        desc.addr = addr;
        desc.length = len;
        desc.csum_offset = 0;
        desc.cmd = TX_CMD_EOP | TX_CMD_IFCS | TX_CMD_RS;
        desc.status = 0;
        desc.csum_start = 0;
        desc.special = 0;
        self.tail = (self.tail + 1) & (TX_RING_SIZE - 1);
        Some(idx)
    }

    pub unsafe fn completed(&self, idx: usize) -> bool {
        if self.desc.is_null() || idx >= TX_RING_SIZE {
            return false;
        }
        let desc = unsafe { &*self.desc.add(idx) };
        desc.status & DESC_STATUS_DD != 0
    }
}
