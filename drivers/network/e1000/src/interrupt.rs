use exo_syscall_abi as syscall;

use crate::regs::{ICR, IMC, IMS, IRQ_LSC, IRQ_RXDMT0, IRQ_RXO, IRQ_RXT0, IRQ_TXDW};

pub const IRQ_SOURCE_IOAPIC_LEVEL: u64 = 1;

#[inline]
unsafe fn read32(mmio: *mut u8, reg: usize) -> u32 {
    unsafe { core::ptr::read_volatile(mmio.add(reg) as *const u32) }
}

#[inline]
unsafe fn write32(mmio: *mut u8, reg: usize, value: u32) {
    unsafe { core::ptr::write_volatile(mmio.add(reg) as *mut u32, value) };
}

pub unsafe fn disable(mmio: *mut u8) {
    unsafe { write32(mmio, IMC, u32::MAX) };
}

pub unsafe fn enable_basic(mmio: *mut u8) {
    unsafe {
        write32(
            mmio,
            IMS,
            IRQ_TXDW | IRQ_LSC | IRQ_RXDMT0 | IRQ_RXO | IRQ_RXT0,
        )
    };
}

pub unsafe fn read_cause(mmio: *mut u8) -> u32 {
    unsafe { read32(mmio, ICR) }
}

pub fn register_irq(irq_line: u8, endpoint_id: u64, bdf_raw: u32) -> Result<u64, i64> {
    if irq_line == 0 || irq_line == u8::MAX {
        return Err(syscall::EINVAL);
    }
    let rc = unsafe {
        syscall::syscall6(
            syscall::SYS_IRQ_REGISTER,
            irq_line as u64 + 32,
            endpoint_id,
            0,
            IRQ_SOURCE_IOAPIC_LEVEL,
            bdf_raw as u64,
            1,
        )
    };
    if rc < 0 {
        Err(rc)
    } else {
        Ok(rc as u64)
    }
}
