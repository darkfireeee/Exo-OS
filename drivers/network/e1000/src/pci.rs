use exo_syscall_abi as syscall;

pub const E1000_VENDOR_ID: u16 = 0x8086;
pub const E1000_DEVICE_82540EM: u16 = 0x100E;
pub const E1000_MMIO_SIZE_FALLBACK: u64 = 128 * 1024;

#[derive(Clone, Copy)]
pub struct PciDevice {
    pub bdf_raw: u32,
    pub bar0_phys: u64,
    pub bar0_size: u64,
    pub bar0_virt: *mut u8,
    pub irq_line: u8,
}

pub fn discover_and_map() -> Result<PciDevice, i64> {
    let mut info = syscall::PciDeviceInfo::default();
    let rc = unsafe {
        syscall::syscall6(
            syscall::SYS_PCI_FIND_DEVICE,
            E1000_VENDOR_ID as u64,
            E1000_DEVICE_82540EM as u64,
            u16::MAX as u64,
            u16::MAX as u64,
            0,
            &mut info as *mut syscall::PciDeviceInfo as u64,
        )
    };
    if rc < 0 {
        return Err(rc);
    }
    if info.bar0_phys == 0 || info.bar0_kind != 0 {
        return Err(syscall::ENODEV);
    }

    let bdf_raw = ((info.segment as u32) << 16)
        | ((info.bus as u32) << 8)
        | ((info.device as u32) << 3)
        | info.function as u32;
    let size = if info.bar0_size == 0 {
        E1000_MMIO_SIZE_FALLBACK
    } else {
        info.bar0_size
    };
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid <= 0 {
        return Err(syscall::EACCES);
    }

    let claim = unsafe {
        syscall::syscall5(
            syscall::SYS_PCI_CLAIM,
            info.bar0_phys,
            size,
            pid as u64,
            bdf_raw as u64,
            1,
        )
    };
    if claim < 0 {
        return Err(claim);
    }

    let mapped = unsafe { syscall::syscall2(syscall::SYS_MMIO_MAP, info.bar0_phys, size) };
    if mapped < 0 {
        return Err(mapped);
    }

    let bus_master = unsafe { syscall::syscall1(syscall::SYS_PCI_BUS_MASTER, 1) };
    if bus_master < 0 {
        return Err(bus_master);
    }

    Ok(PciDevice {
        bdf_raw,
        bar0_phys: info.bar0_phys,
        bar0_size: size,
        bar0_virt: mapped as u64 as *mut u8,
        irq_line: info.irq_line,
    })
}
