//! TLB flush optimizations

use crate::arch::x86_64::registers;

/// Flush entire TLB
pub fn flush_all() {
    unsafe {
        let cr3 = registers::read_cr3();
        registers::write_cr3(cr3);
    }
}

/// Flush single page
pub fn flush_page(addr: usize) {
    unsafe {
        core::arch::asm!("invlpg [{}]", in(reg) addr, options(nostack, preserves_flags));
    }
}

/// PCID support (Process-Context Identifiers)
pub struct Pcid(u16);

impl Pcid {
    pub fn new(id: u16) -> Option<Self> {
        if id < 4096 {
            Some(Self(id))
        } else {
            None
        }
    }
}
