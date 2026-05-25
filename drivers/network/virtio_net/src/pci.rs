use exo_syscall_abi as syscall;

use crate::config::{
    VIRTIO_NET_PCI_DEVICE_LEGACY, VIRTIO_NET_PCI_DEVICE_MODERN, VIRTIO_PCI_CAP_COMMON_CFG,
    VIRTIO_PCI_CAP_DEVICE_CFG, VIRTIO_PCI_CAP_ISR_CFG, VIRTIO_PCI_CAP_NOTIFY_CFG,
    VIRTIO_PCI_VENDOR,
};

const PCI_STATUS_OFFSET: u16 = 0x06;
const PCI_CAPABILITY_LIST_OFFSET: u16 = 0x34;
const PCI_STATUS_CAP_LIST: u16 = 1 << 4;
const PCI_CAP_ID_VENDOR_SPECIFIC: u8 = 0x09;
const VIRTIO_PCI_CAP_MIN_LEN: u8 = 16;
const VIRTIO_PCI_NOTIFY_CAP_LEN: u8 = 20;
const CAPABILITY_SCAN_LIMIT: usize = 48;

pub struct PciDevice {
    pub bdf_raw: u32,
    pub irq_line: u8,
    pub common_cfg: *mut u8,
    pub notify_cfg: *mut u8,
    pub notify_off_multiplier: u32,
    pub isr_cfg: *mut u8,
    pub device_cfg: *mut u8,
}

#[derive(Clone, Copy)]
struct BarMapping {
    phys: u64,
    size: u64,
    virt: *mut u8,
}

impl BarMapping {
    const fn empty() -> Self {
        Self {
            phys: 0,
            size: 0,
            virt: core::ptr::null_mut(),
        }
    }
}

struct CapabilityRegions {
    common_cfg: *mut u8,
    notify_cfg: *mut u8,
    notify_off_multiplier: u32,
    isr_cfg: *mut u8,
    device_cfg: *mut u8,
}

impl CapabilityRegions {
    const fn empty() -> Self {
        Self {
            common_cfg: core::ptr::null_mut(),
            notify_cfg: core::ptr::null_mut(),
            notify_off_multiplier: 0,
            isr_cfg: core::ptr::null_mut(),
            device_cfg: core::ptr::null_mut(),
        }
    }
}

pub fn discover_and_map() -> Result<PciDevice, i64> {
    match find_one(VIRTIO_NET_PCI_DEVICE_MODERN) {
        Ok(device) => Ok(device),
        Err(modern_err) if modern_err == syscall::ENOENT => find_one(VIRTIO_NET_PCI_DEVICE_LEGACY),
        Err(modern_err) => Err(modern_err),
    }
}

fn find_one(device_id: u16) -> Result<PciDevice, i64> {
    let mut info = syscall::PciDeviceInfo::default();
    let rc = unsafe {
        syscall::syscall6(
            syscall::SYS_PCI_FIND_DEVICE,
            VIRTIO_PCI_VENDOR as u64,
            device_id as u64,
            u16::MAX as u64,
            u16::MAX as u64,
            0,
            &mut info as *mut syscall::PciDeviceInfo as u64,
        )
    };
    if rc < 0 {
        return Err(rc);
    }
    let bdf_raw = ((info.segment as u32) << 16)
        | ((info.bus as u32) << 8)
        | ((info.device as u32) << 3)
        | info.function as u32;
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid <= 0 {
        return Err(syscall::EACCES);
    }

    let mut bars = [BarMapping::empty(); 6];
    let seed_bar = info
        .bars
        .iter()
        .position(|bar| bar.kind == 0 && bar.phys != 0 && bar.size != 0)
        .ok_or_else(|| {
            debug_errno(b"virtio_net_driver: pci mmio bar errno ", syscall::ENODEV);
            syscall::ENODEV
        })?;
    map_bar(&info, seed_bar, pid as u64, bdf_raw, &mut bars).map_err(|err| {
        debug_errno(b"virtio_net_driver: pci seed map errno ", err);
        err
    })?;
    let bus_master = unsafe { syscall::syscall1(syscall::SYS_PCI_BUS_MASTER, 1) };
    if bus_master < 0 {
        debug_errno(b"virtio_net_driver: pci bus master errno ", bus_master);
        return Err(bus_master);
    }

    let regions = parse_capabilities(&info, pid as u64, bdf_raw, &mut bars).map_err(|err| {
        debug_errno(b"virtio_net_driver: pci caps errno ", err);
        err
    })?;
    if regions.common_cfg.is_null()
        || regions.notify_cfg.is_null()
        || regions.notify_off_multiplier == 0
        || regions.isr_cfg.is_null()
    {
        debug_errno(
            b"virtio_net_driver: pci caps missing errno ",
            syscall::ENODEV,
        );
        return Err(syscall::ENODEV);
    }

    Ok(PciDevice {
        bdf_raw,
        irq_line: info.irq_line,
        common_cfg: regions.common_cfg,
        notify_cfg: regions.notify_cfg,
        notify_off_multiplier: regions.notify_off_multiplier,
        isr_cfg: regions.isr_cfg,
        device_cfg: regions.device_cfg,
    })
}

fn parse_capabilities(
    info: &syscall::PciDeviceInfo,
    pid: u64,
    bdf_raw: u32,
    bars: &mut [BarMapping; 6],
) -> Result<CapabilityRegions, i64> {
    if cfg_read16(PCI_STATUS_OFFSET)? & PCI_STATUS_CAP_LIST == 0 {
        return Err(syscall::ENODEV);
    }

    let mut regions = CapabilityRegions::empty();
    let mut ptr = cfg_read8(PCI_CAPABILITY_LIST_OFFSET)? as u16;
    let mut walked = 0usize;
    while ptr >= 0x40 && walked < CAPABILITY_SCAN_LIMIT {
        let next = cfg_read8(ptr + 1)? as u16;
        if cfg_read8(ptr)? == PCI_CAP_ID_VENDOR_SPECIFIC {
            parse_virtio_capability(ptr, info, pid, bdf_raw, bars, &mut regions)?;
        }
        if next == 0 || next == ptr {
            break;
        }
        ptr = next;
        walked += 1;
    }
    Ok(regions)
}

fn parse_virtio_capability(
    ptr: u16,
    info: &syscall::PciDeviceInfo,
    pid: u64,
    bdf_raw: u32,
    bars: &mut [BarMapping; 6],
    regions: &mut CapabilityRegions,
) -> Result<(), i64> {
    let cap_len = cfg_read8(ptr + 2)?;
    if cap_len < VIRTIO_PCI_CAP_MIN_LEN {
        return Ok(());
    }
    let cfg_type = cfg_read8(ptr + 3)?;
    let bar = cfg_read8(ptr + 4)? as usize;
    let offset = cfg_read32(ptr + 8)? as u64;
    let length = cfg_read32(ptr + 12)? as u64;

    match cfg_type {
        VIRTIO_PCI_CAP_COMMON_CFG if regions.common_cfg.is_null() => {
            regions.common_cfg = cap_region(info, bar, offset, length, pid, bdf_raw, bars)?;
        }
        VIRTIO_PCI_CAP_NOTIFY_CFG if regions.notify_cfg.is_null() => {
            if cap_len < VIRTIO_PCI_NOTIFY_CAP_LEN {
                return Err(syscall::ENODEV);
            }
            regions.notify_cfg = cap_region(info, bar, offset, length, pid, bdf_raw, bars)?;
            regions.notify_off_multiplier = cfg_read32(ptr + 16)?;
        }
        VIRTIO_PCI_CAP_ISR_CFG if regions.isr_cfg.is_null() => {
            regions.isr_cfg = cap_region(info, bar, offset, length, pid, bdf_raw, bars)?;
        }
        VIRTIO_PCI_CAP_DEVICE_CFG if regions.device_cfg.is_null() => {
            regions.device_cfg = cap_region(info, bar, offset, length, pid, bdf_raw, bars)?;
        }
        _ => {}
    }
    Ok(())
}

fn cap_region(
    info: &syscall::PciDeviceInfo,
    bar: usize,
    offset: u64,
    length: u64,
    pid: u64,
    bdf_raw: u32,
    bars: &mut [BarMapping; 6],
) -> Result<*mut u8, i64> {
    if bar >= bars.len() || length == 0 {
        return Err(syscall::ENODEV);
    }
    let mapped = map_bar(info, bar, pid, bdf_raw, bars)?;
    let end = offset.checked_add(length).ok_or(syscall::ENODEV)?;
    if end > mapped.size {
        return Err(syscall::ENODEV);
    }
    Ok(unsafe { mapped.virt.add(offset as usize) })
}

fn map_bar(
    info: &syscall::PciDeviceInfo,
    index: usize,
    pid: u64,
    bdf_raw: u32,
    bars: &mut [BarMapping; 6],
) -> Result<BarMapping, i64> {
    if index >= bars.len() {
        return Err(syscall::ENODEV);
    }
    if !bars[index].virt.is_null() {
        return Ok(bars[index]);
    }
    let bar = info.bars[index];
    if bar.kind != 0 || bar.phys == 0 || bar.size == 0 {
        return Err(syscall::ENODEV);
    }
    let claim = unsafe {
        syscall::syscall5(
            syscall::SYS_PCI_CLAIM,
            bar.phys,
            bar.size,
            pid,
            bdf_raw as u64,
            1,
        )
    };
    if claim < 0 {
        return Err(claim);
    }
    let virt = unsafe { syscall::syscall2(syscall::SYS_MMIO_MAP, bar.phys, bar.size) };
    if virt < 0 {
        return Err(virt);
    }
    bars[index] = BarMapping {
        phys: bar.phys,
        size: bar.size,
        virt: virt as u64 as *mut u8,
    };
    Ok(bars[index])
}

fn cfg_read32(offset: u16) -> Result<u32, i64> {
    let rc = unsafe { syscall::syscall1(syscall::SYS_PCI_CFG_READ, offset as u64) };
    if rc < 0 {
        Err(rc)
    } else {
        Ok(rc as u32)
    }
}

fn cfg_read16(offset: u16) -> Result<u16, i64> {
    let value = cfg_read32(offset & !0x3)?;
    let shift = ((offset & 0x2) * 8) as u32;
    Ok((value >> shift) as u16)
}

fn cfg_read8(offset: u16) -> Result<u8, i64> {
    let value = cfg_read32(offset & !0x3)?;
    let shift = ((offset & 0x3) * 8) as u32;
    Ok((value >> shift) as u8)
}

fn debug_errno(prefix: &[u8], err: i64) {
    debug_write(prefix);
    let negative = err < 0;
    let mut value = if negative {
        err.wrapping_neg() as u64
    } else {
        err as u64
    };
    if negative {
        debug_write(b"-");
    }
    let mut digits = [0u8; 20];
    let mut pos = digits.len();
    if value == 0 {
        pos -= 1;
        digits[pos] = b'0';
    } else {
        while value != 0 {
            pos -= 1;
            digits[pos] = b'0' + (value % 10) as u8;
            value /= 10;
        }
    }
    debug_write(&digits[pos..]);
    debug_write(b"\n");
}

fn debug_write(bytes: &[u8]) {
    for &byte in bytes {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::asm!("out 0xE9, al", in("al") byte, options(nomem, nostack));
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = byte;
    }
}
