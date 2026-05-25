use exo_syscall_abi as syscall;

pub const IRQ_SOURCE_IOAPIC_LEVEL: u64 = 1;

pub unsafe fn ack_pending(isr_cfg: *mut u8) -> u8 {
    // PCI ISR status is acknowledged by reading it.
    unsafe { core::ptr::read_volatile(isr_cfg as *const u8) }
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
