use crate::regs::{RAH, RAL};

pub const DEFAULT_MAC: [u8; 6] = [0x02, 0x45, 0x58, 0x4f, 0x00, 0x01];

#[inline]
unsafe fn read32(mmio: *mut u8, reg: usize) -> u32 {
    unsafe { core::ptr::read_volatile(mmio.add(reg) as *const u32) }
}

pub unsafe fn read_mac(mmio: *mut u8) -> [u8; 6] {
    let ral = unsafe { read32(mmio, RAL) };
    let rah = unsafe { read32(mmio, RAH) };
    let mac = [
        (ral & 0xff) as u8,
        ((ral >> 8) & 0xff) as u8,
        ((ral >> 16) & 0xff) as u8,
        ((ral >> 24) & 0xff) as u8,
        (rah & 0xff) as u8,
        ((rah >> 8) & 0xff) as u8,
    ];
    if mac == [0; 6] {
        DEFAULT_MAC
    } else {
        mac
    }
}
