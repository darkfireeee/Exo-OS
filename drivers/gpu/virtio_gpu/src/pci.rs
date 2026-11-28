//! VirtIO-GPU 1.2 Modern PCI capability discovery & BAR MMIO mapping (`0x1AF4:0x1050`).

use exo_syscall_abi as syscall;

pub const VIRTIO_PCI_VENDOR_ID: u16 = 0x1AF4;
pub const VIRTIO_GPU_PCI_DEVICE_ID: u16 = 0x1050;

pub const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
pub const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
pub const VIRTIO_PCI_CAP_ISR_CFG: u8 = 3;
pub const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

// VirtIO 1.2 Common Configuration register offsets
pub const COMMON_DEVICE_FEATURE_SELECT: usize = 0x00;
pub const COMMON_DEVICE_FEATURE: usize = 0x04;
pub const COMMON_DRIVER_FEATURE_SELECT: usize = 0x08;
pub const COMMON_DRIVER_FEATURE: usize = 0x0C;
pub const COMMON_DEVICE_STATUS: usize = 0x14;
pub const COMMON_QUEUE_SELECT: usize = 0x16;
pub const COMMON_QUEUE_SIZE: usize = 0x18;
pub const COMMON_QUEUE_ENABLE: usize = 0x1C;
pub const COMMON_QUEUE_NOTIFY_OFF: usize = 0x1E;
pub const COMMON_QUEUE_DESC: usize = 0x20;
pub const COMMON_QUEUE_DRIVER: usize = 0x28;
pub const COMMON_QUEUE_DEVICE: usize = 0x30;

pub const VIRTIO_STATUS_ACKNOWLEDGE: u8 = 1;
pub const VIRTIO_STATUS_DRIVER: u8 = 2;
pub const VIRTIO_STATUS_DRIVER_OK: u8 = 4;
pub const VIRTIO_STATUS_FEATURES_OK: u8 = 8;
pub const VIRTIO_STATUS_FAILED: u8 = 128;

pub const VIRTIO_F_VERSION_1: u64 = 1 << 32;

const PCI_STATUS_OFFSET: u16 = 0x06;
const PCI_CAPABILITY_LIST_OFFSET: u16 = 0x34;
const PCI_STATUS_CAP_LIST: u16 = 1 << 4;
const PCI_CAP_ID_VENDOR_SPECIFIC: u8 = 0x09;
const VIRTIO_PCI_CAP_MIN_LEN: u8 = 16;
const VIRTIO_PCI_NOTIFY_CAP_LEN: u8 = 20;
const CAPABILITY_SCAN_LIMIT: usize = 48;

#[derive(Clone, Copy)]
pub struct VirtioGpuPci {
    pub bdf_raw: u32,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub irq_line: u8,
    pub bar0_phys: u64,
    pub common_cfg: *mut u8,
    pub notify_cfg: *mut u8,
    pub notify_off_multiplier: u32,
    pub isr_cfg: *mut u8,
    pub device_cfg: *mut u8,
}

impl VirtioGpuPci {
    pub const fn empty() -> Self {
        Self {
            bdf_raw: 0,
            bus: 0,
            device: 0,
            function: 0,
            irq_line: 0,
            bar0_phys: 0,
            common_cfg: core::ptr::null_mut(),
            notify_cfg: core::ptr::null_mut(),
            notify_off_multiplier: 0,
            isr_cfg: core::ptr::null_mut(),
            device_cfg: core::ptr::null_mut(),
        }
    }
}

#[derive(Clone, Copy)]
struct BarMapping {
    size: u64,
    virt: *mut u8,
}

impl BarMapping {
    const fn empty() -> Self {
        Self {
            size: 0,
            virt: core::ptr::null_mut(),
        }
    }
}

pub fn discover_and_map() -> Result<VirtioGpuPci, i64> {
    let mut info = syscall::PciDeviceInfo::default();
    let rc = unsafe {
        syscall::syscall6(
            syscall::SYS_PCI_FIND_DEVICE,
            VIRTIO_PCI_VENDOR_ID as u64,
            VIRTIO_GPU_PCI_DEVICE_ID as u64,
            0xFFFF,
            0xFFFF,
            0,
            &mut info as *mut syscall::PciDeviceInfo as u64,
        )
    };
    if rc < 0 || info.vendor_id != VIRTIO_PCI_VENDOR_ID {
        return Err(if rc < 0 { rc } else { syscall::ENODEV });
    }
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid <= 0 {
        return Err(syscall::EACCES);
    }
    let bdf_raw = ((info.segment as u32) << 16)
        | ((info.bus as u32) << 8)
        | ((info.device as u32) << 3)
        | info.function as u32;

    let mut bars = [BarMapping::empty(); 6];
    // Claim the highest non-zero MMIO BAR first (for virtio-vga, BAR0 is the VGA framebuffer
    // already owned by fb_server, while BAR4 holds the VirtIO modern PCI capability registers!).
    let seed_bar = info
        .bars
        .iter()
        .enumerate()
        .rev()
        .find(|(_, b)| b.kind == 0 && b.phys != 0 && b.size != 0)
        .map(|(idx, _)| idx)
        .ok_or(syscall::ENODEV)?;
    map_bar(&info, seed_bar, pid as u64, bdf_raw, &mut bars)?;

    let bm = unsafe { syscall::syscall1(syscall::SYS_PCI_BUS_MASTER, 1) };
    if bm < 0 {
        return Err(bm);
    }
    let _ = unsafe { syscall::syscall3(syscall::SYS_PCI_SET_TOPOLOGY, bdf_raw as u64, 0, 1) };

    if cfg_read16(PCI_STATUS_OFFSET)? & PCI_STATUS_CAP_LIST == 0 {
        return Err(syscall::ENODEV);
    }

    let mut pci = VirtioGpuPci {
        bdf_raw,
        bus: info.bus,
        device: info.device,
        function: info.function,
        irq_line: info.irq_line,
        bar0_phys: info.bar0_phys,
        common_cfg: core::ptr::null_mut(),
        notify_cfg: core::ptr::null_mut(),
        notify_off_multiplier: 0,
        isr_cfg: core::ptr::null_mut(),
        device_cfg: core::ptr::null_mut(),
    };

    let mut ptr = cfg_read8(PCI_CAPABILITY_LIST_OFFSET)? as u16;
    let mut walked = 0usize;
    while ptr >= 0x40 && walked < CAPABILITY_SCAN_LIMIT {
        let next = cfg_read8(ptr + 1)? as u16;
        if cfg_read8(ptr)? == PCI_CAP_ID_VENDOR_SPECIFIC {
            let cap_len = cfg_read8(ptr + 2)?;
            if cap_len >= VIRTIO_PCI_CAP_MIN_LEN {
                let cfg_type = cfg_read8(ptr + 3)?;
                let bar = cfg_read8(ptr + 4)? as usize;
                let offset = cfg_read32(ptr + 8)? as u64;
                let length = cfg_read32(ptr + 12)? as u64;
                match cfg_type {
                    VIRTIO_PCI_CAP_COMMON_CFG if pci.common_cfg.is_null() => {
                        pci.common_cfg = cap_region(&info, bar, offset, length, pid as u64, bdf_raw, &mut bars)?;
                    }
                    VIRTIO_PCI_CAP_NOTIFY_CFG if pci.notify_cfg.is_null() && cap_len >= VIRTIO_PCI_NOTIFY_CAP_LEN => {
                        pci.notify_cfg = cap_region(&info, bar, offset, length, pid as u64, bdf_raw, &mut bars)?;
                        pci.notify_off_multiplier = cfg_read32(ptr + 16)?;
                    }
                    VIRTIO_PCI_CAP_ISR_CFG if pci.isr_cfg.is_null() => {
                        pci.isr_cfg = cap_region(&info, bar, offset, length, pid as u64, bdf_raw, &mut bars)?;
                    }
                    VIRTIO_PCI_CAP_DEVICE_CFG if pci.device_cfg.is_null() => {
                        pci.device_cfg = cap_region(&info, bar, offset, length, pid as u64, bdf_raw, &mut bars)?;
                    }
                    _ => {}
                }
            }
        }
        if next == 0 || next == ptr {
            break;
        }
        ptr = next;
        walked += 1;
    }

    if pci.common_cfg.is_null() || pci.notify_cfg.is_null() || pci.notify_off_multiplier == 0 {
        return Err(syscall::ENODEV);
    }
    Ok(pci)
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
        syscall::syscall5(syscall::SYS_PCI_CLAIM, bar.phys, bar.size, pid, bdf_raw as u64, 1)
    };
    if claim < 0 && claim != syscall::EBUSY {
        return Err(claim);
    }
    let virt = unsafe { syscall::syscall2(syscall::SYS_MMIO_MAP, bar.phys, bar.size) };
    if virt <= 0 {
        return Err(if virt < 0 { virt } else { syscall::ENOMEM });
    }
    bars[index] = BarMapping {
        size: bar.size,
        virt: virt as u64 as *mut u8,
    };
    Ok(bars[index])
}

#[inline]
pub unsafe fn read8(base: *mut u8, off: usize) -> u8 {
    unsafe { core::ptr::read_volatile(base.add(off)) }
}
#[inline]
pub unsafe fn write8(base: *mut u8, off: usize, v: u8) {
    unsafe { core::ptr::write_volatile(base.add(off), v) }
}
#[inline]
pub unsafe fn read16(base: *mut u8, off: usize) -> u16 {
    unsafe { core::ptr::read_volatile(base.add(off) as *const u16) }
}
#[inline]
pub unsafe fn write16(base: *mut u8, off: usize, v: u16) {
    unsafe { core::ptr::write_volatile(base.add(off) as *mut u16, v) }
}
#[inline]
pub unsafe fn read32(base: *mut u8, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile(base.add(off) as *const u32) }
}
#[inline]
pub unsafe fn write32(base: *mut u8, off: usize, v: u32) {
    unsafe { core::ptr::write_volatile(base.add(off) as *mut u32, v) }
}
#[inline]
pub unsafe fn write64(base: *mut u8, off: usize, v: u64) {
    unsafe {
        write32(base, off, v as u32);
        write32(base, off + 4, (v >> 32) as u32);
    }
}

fn cfg_read32(offset: u16) -> Result<u32, i64> {
    let rc = unsafe { syscall::syscall1(syscall::SYS_PCI_CFG_READ, offset as u64) };
    if rc < 0 { Err(rc) } else { Ok(rc as u32) }
}
fn cfg_read16(offset: u16) -> Result<u16, i64> {
    let v = cfg_read32(offset & !0x3)?;
    Ok((v >> (((offset & 0x2) * 8) as u32)) as u16)
}
fn cfg_read8(offset: u16) -> Result<u8, i64> {
    let v = cfg_read32(offset & !0x3)?;
    Ok((v >> (((offset & 0x3) * 8) as u32)) as u8)
}
