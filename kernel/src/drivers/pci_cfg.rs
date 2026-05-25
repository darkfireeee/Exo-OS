//! # drivers/pci_cfg.rs
//!
//! Accès PCI configuration space + helpers de cleanup GI-03.

use core::hint::spin_loop;

use spin::Mutex;

use crate::arch::x86_64::{inl, irq_save, outl};
use crate::memory::core::PhysAddr;
use crate::scheduler::timer::clock::monotonic_ns;

use super::device_claims::{self, PciBdf};
use super::pci_topology;
use super::PciCfgError;

const PCI_CFG_ADDR: u16 = 0xCF8;
const PCI_CFG_DATA: u16 = 0xCFC;

const PCI_COMMAND_OFFSET: u16 = 0x04;
const PCI_STATUS_OFFSET: u16 = 0x06;
const PCI_CLASS_REV_OFFSET: u16 = 0x08;
const PCI_HEADER_TYPE_OFFSET: u16 = 0x0E;
const PCI_CAPABILITY_LIST_OFFSET: u16 = 0x34;
const PCI_BRIDGE_CONTROL_OFFSET: u16 = 0x3E;
const PCI_BAR0_OFFSET: u16 = 0x10;
const PCI_INTERRUPT_LINE_OFFSET: u16 = 0x3C;
const PCI_INTERRUPT_PIN_OFFSET: u16 = 0x3D;

const PCI_COMMAND_IO_SPACE: u16 = 1 << 0;
const PCI_COMMAND_MEMORY_SPACE: u16 = 1 << 1;
const PCI_COMMAND_BUS_MASTER: u16 = 1 << 2;
const PCI_STATUS_CAP_LIST: u16 = 1 << 4;
const PCI_BRIDGE_CTL_BUS_RESET: u16 = 1 << 6;

const PCI_CAP_ID_EXP: u8 = 0x10;
const PCI_EXP_DEVSTA: u16 = 0x0A;
const PCI_EXP_DEVSTA_TRPND: u16 = 1 << 5;
const PCI_EXP_LNKSTA: u16 = 0x12;
const PCI_EXP_LNKSTA_DLLLA: u16 = 1 << 13;
const PCI_VENDOR_VIRTIO: u16 = 0x1AF4;
const PCI_DEVICE_VIRTIO_BLK_LEGACY: u16 = 0x1001;
const PCI_DEVICE_VIRTIO_BLK_MODERN: u16 = 0x1042;

static PCI_CFG_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct PciBarInfo {
    pub kind: u8,
    pub _pad: [u8; 7],
    pub phys: u64,
    pub size: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct PciDeviceInfo {
    pub vendor_id: u16,
    pub device_id: u16,
    pub segment: u16,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision: u8,
    pub irq_line: u8,
    pub irq_pin: u8,
    pub bar0_kind: u8,
    pub _pad0: u8,
    pub bar0_phys: u64,
    pub bar0_size: u64,
    pub bars: [PciBarInfo; 6],
}

#[inline]
fn now_ms() -> u64 {
    monotonic_ns() / 1_000_000
}

fn busy_wait_ms(delay_ms: u64) {
    let deadline = now_ms().saturating_add(delay_ms);
    while now_ms() < deadline {
        spin_loop();
    }
}

fn claimed_bdf(pid: u32) -> Result<PciBdf, PciCfgError> {
    device_claims::bdf_of_pid(pid).ok_or(PciCfgError::NotClaimed)
}

#[inline]
fn topology_bdf(bdf: PciBdf) -> pci_topology::PciBdf {
    pci_topology::PciBdf {
        bus: bdf.bus,
        dev: bdf.dev,
        func: bdf.func,
    }
}

#[inline]
fn claim_bdf(bdf: pci_topology::PciBdf) -> PciBdf {
    PciBdf {
        bus: bdf.bus,
        dev: bdf.dev,
        func: bdf.func,
    }
}

fn cfg_address(bdf: PciBdf, offset: u16) -> u32 {
    0x8000_0000u32
        | ((bdf.bus as u32) << 16)
        | ((bdf.dev as u32) << 11)
        | ((bdf.func as u32) << 8)
        | ((offset as u32) & !0x3)
}

fn pci_cfg_read32(bdf: PciBdf, offset: u16) -> u32 {
    let _irq = irq_save();
    let _lock = PCI_CFG_LOCK.lock();

    unsafe {
        outl(PCI_CFG_ADDR, cfg_address(bdf, offset));
        inl(PCI_CFG_DATA)
    }
}

fn pci_cfg_write32(bdf: PciBdf, offset: u16, value: u32) {
    let _irq = irq_save();
    let _lock = PCI_CFG_LOCK.lock();

    unsafe {
        outl(PCI_CFG_ADDR, cfg_address(bdf, offset));
        outl(PCI_CFG_DATA, value);
    }
}

fn pci_cfg_read16(bdf: PciBdf, offset: u16) -> u16 {
    let shift = ((offset & 0x2) * 8) as u32;
    (pci_cfg_read32(bdf, offset) >> shift) as u16
}

fn pci_cfg_write16(bdf: PciBdf, offset: u16, value: u16) {
    let aligned = offset & !0x3;
    let shift = ((offset & 0x2) * 8) as u32;
    let mask = !(0xFFFFu32 << shift);
    let current = pci_cfg_read32(bdf, aligned);
    let updated = (current & mask) | ((value as u32) << shift);
    pci_cfg_write32(bdf, aligned, updated);
}

fn pci_cfg_read8(bdf: PciBdf, offset: u16) -> u8 {
    let shift = ((offset & 0x3) * 8) as u32;
    (pci_cfg_read32(bdf, offset) >> shift) as u8
}

fn is_virtio_block_device(vendor_id: u16, device_id: u16, class_code: u8, subclass: u8) -> bool {
    vendor_id == PCI_VENDOR_VIRTIO
        && (device_id == PCI_DEVICE_VIRTIO_BLK_LEGACY
            || device_id == PCI_DEVICE_VIRTIO_BLK_MODERN
            || ((0x1040..=0x107F).contains(&device_id) && class_code == 0x01 && subclass == 0x00))
}

fn pci_mmio_bar_base(bdf: PciBdf, bar_offset: u16) -> Option<u64> {
    let raw = pci_cfg_read32(bdf, bar_offset);
    if raw == 0 || raw == u32::MAX || raw & 1 != 0 {
        return None;
    }

    let mem_type = (raw >> 1) & 0x3;
    if mem_type == 0x1 {
        return None;
    }

    let low = (raw & 0xFFFF_FFF0) as u64;
    let base = if mem_type == 0x2 {
        let high = pci_cfg_read32(bdf, bar_offset + 4) as u64;
        (high << 32) | low
    } else {
        low
    };

    if base == 0 {
        None
    } else {
        Some(base)
    }
}

fn pci_bar_base_and_kind(bdf: PciBdf, bar_offset: u16) -> Option<(u64, u8)> {
    let raw = pci_cfg_read32(bdf, bar_offset);
    if raw == 0 || raw == u32::MAX {
        return None;
    }

    if raw & 1 != 0 {
        let base = (raw & 0xFFFF_FFFC) as u64;
        return (base != 0).then_some((base, 1));
    }

    let mem_type = (raw >> 1) & 0x3;
    if mem_type == 0x1 {
        return None;
    }

    let low = (raw & 0xFFFF_FFF0) as u64;
    let base = if mem_type == 0x2 {
        let high = pci_cfg_read32(bdf, bar_offset + 4) as u64;
        (high << 32) | low
    } else {
        low
    };

    (base != 0).then_some((base, 0))
}

fn pci_bar_size(bdf: PciBdf, bar_offset: u16) -> u64 {
    let raw = pci_cfg_read32(bdf, bar_offset);
    if raw == 0 || raw == u32::MAX {
        return 0;
    }

    let command = pci_cfg_read16(bdf, PCI_COMMAND_OFFSET);
    pci_cfg_write16(
        bdf,
        PCI_COMMAND_OFFSET,
        command & !(PCI_COMMAND_IO_SPACE | PCI_COMMAND_MEMORY_SPACE),
    );

    pci_cfg_write32(bdf, bar_offset, u32::MAX);
    let size_raw = pci_cfg_read32(bdf, bar_offset);
    pci_cfg_write32(bdf, bar_offset, raw);
    pci_cfg_write16(bdf, PCI_COMMAND_OFFSET, command);

    if raw & 1 != 0 {
        let mask = size_raw & 0xFFFF_FFFC;
        if mask == 0 {
            return 0;
        }
        (!(mask as u64)).wrapping_add(1) & 0xFFFF_FFFF
    } else {
        let mask = size_raw & 0xFFFF_FFF0;
        if mask == 0 {
            return 0;
        }
        (!(mask as u64)).wrapping_add(1) & 0xFFFF_FFFF
    }
}

fn pci_bars(bdf: PciBdf, header_type: u8) -> [PciBarInfo; 6] {
    let bar_slots = if header_type & 0x7F == 0x01 { 2 } else { 6 };
    let mut bars = [PciBarInfo::default(); 6];
    let mut bar_idx = 0u16;
    while bar_idx < bar_slots {
        let offset = PCI_BAR0_OFFSET + bar_idx * 4;
        let raw = pci_cfg_read32(bdf, offset);
        if let Some((phys, kind)) = pci_bar_base_and_kind(bdf, offset) {
            bars[bar_idx as usize] = PciBarInfo {
                kind,
                _pad: [0; 7],
                phys,
                size: pci_bar_size(bdf, offset),
            };
        }

        let is_64_bit_mem_bar = raw & 1 == 0 && ((raw >> 1) & 0x3) == 0x2;
        bar_idx += if is_64_bit_mem_bar { 2 } else { 1 };
    }
    bars
}

/// Returns true when the requested range stays inside a PCI MMIO BAR for `bdf`.
///
/// PCI MMIO apertures are not guaranteed to appear as `Reserved` in the boot
/// memory map. A claim carrying a BDF can use this hardware-authored range as
/// the allow-list instead of trusting an arbitrary userspace physical range.
pub(super) fn pci_mmio_bar_contains(bdf: PciBdf, phys_base: PhysAddr, size: usize) -> bool {
    let Some(request_end) = phys_base.as_u64().checked_add(size as u64) else {
        return false;
    };
    let header_type = pci_cfg_read8(bdf, PCI_HEADER_TYPE_OFFSET);
    pci_bars(bdf, header_type).iter().any(|bar| {
        if bar.kind != 0 || bar.phys == 0 || bar.size == 0 {
            return false;
        }
        let Some(bar_end) = bar.phys.checked_add(bar.size) else {
            return false;
        };
        phys_base.as_u64() >= bar.phys && request_end <= bar_end
    })
}

#[allow(clippy::too_many_arguments)]
pub fn find_pci_device(
    vendor_filter: u16,
    device_filter: u16,
    class_filter: u16,
    subclass_filter: u16,
    index: u32,
) -> Option<PciDeviceInfo> {
    let mut seen = 0u32;
    for bus in 0u16..=255 {
        let bus = bus as u8;
        for dev in 0u8..32 {
            let bdf0 = PciBdf { bus, dev, func: 0 };
            if pci_cfg_read16(bdf0, 0x00) == u16::MAX {
                continue;
            }

            let header_type = pci_cfg_read8(bdf0, PCI_HEADER_TYPE_OFFSET);
            let function_count = if header_type & 0x80 != 0 { 8 } else { 1 };

            for func in 0u8..function_count {
                let bdf = PciBdf { bus, dev, func };
                let id = pci_cfg_read32(bdf, 0x00);
                let vendor_id = (id & 0xFFFF) as u16;
                if vendor_id == u16::MAX {
                    continue;
                }
                let device_id = (id >> 16) as u16;
                let class_reg = pci_cfg_read32(bdf, PCI_CLASS_REV_OFFSET);
                let revision = (class_reg & 0xFF) as u8;
                let prog_if = ((class_reg >> 8) & 0xFF) as u8;
                let subclass = ((class_reg >> 16) & 0xFF) as u8;
                let class_code = ((class_reg >> 24) & 0xFF) as u8;

                if vendor_filter != 0 && vendor_id != vendor_filter {
                    continue;
                }
                if device_filter != 0 && device_id != device_filter {
                    continue;
                }
                if class_filter <= u8::MAX as u16 && class_code != class_filter as u8 {
                    continue;
                }
                if subclass_filter <= u8::MAX as u16 && subclass != subclass_filter as u8 {
                    continue;
                }

                if seen != index {
                    seen = seen.saturating_add(1);
                    continue;
                }

                let bars = pci_bars(bdf, header_type);
                let bar0 = bars[0];
                return Some(PciDeviceInfo {
                    vendor_id,
                    device_id,
                    segment: 0,
                    bus,
                    device: dev,
                    function: func,
                    class_code,
                    subclass,
                    prog_if,
                    revision,
                    irq_line: pci_cfg_read8(bdf, PCI_INTERRUPT_LINE_OFFSET),
                    irq_pin: pci_cfg_read8(bdf, PCI_INTERRUPT_PIN_OFFSET),
                    bar0_kind: bar0.kind,
                    _pad0: 0,
                    bar0_phys: bar0.phys,
                    bar0_size: bar0.size,
                    bars,
                });
            }
        }
    }

    None
}

fn find_capability(bdf: PciBdf, cap_id: u8) -> Option<u16> {
    if pci_cfg_read16(bdf, PCI_STATUS_OFFSET) & PCI_STATUS_CAP_LIST == 0 {
        return None;
    }

    let mut ptr = pci_cfg_read8(bdf, PCI_CAPABILITY_LIST_OFFSET) as u16;
    let mut walked = 0usize;

    while ptr >= 0x40 && walked < 48 {
        let id = pci_cfg_read8(bdf, ptr);
        if id == cap_id {
            return Some(ptr);
        }

        let next = pci_cfg_read8(bdf, ptr + 1) as u16;
        if next == 0 || next == ptr {
            break;
        }

        ptr = next;
        walked += 1;
    }

    None
}

pub fn sys_pci_cfg_read_for_pid(pid: u32, offset: u16) -> Result<u32, PciCfgError> {
    Ok(pci_cfg_read32(claimed_bdf(pid)?, offset))
}

pub fn find_virtio_blk_mmio_bar() -> Option<usize> {
    for bus in 0u16..=255 {
        let bus = bus as u8;
        for dev in 0u8..32 {
            let bdf0 = PciBdf { bus, dev, func: 0 };
            if pci_cfg_read16(bdf0, 0x00) == u16::MAX {
                continue;
            }

            let header_type = pci_cfg_read8(bdf0, PCI_HEADER_TYPE_OFFSET);
            let function_count = if header_type & 0x80 != 0 { 8 } else { 1 };

            for func in 0u8..function_count {
                let bdf = PciBdf { bus, dev, func };
                let id = pci_cfg_read32(bdf, 0x00);
                let vendor_id = (id & 0xFFFF) as u16;
                if vendor_id == u16::MAX {
                    continue;
                }
                let device_id = (id >> 16) as u16;
                let class_reg = pci_cfg_read32(bdf, PCI_CLASS_REV_OFFSET);
                let class_code = (class_reg >> 24) as u8;
                let subclass = (class_reg >> 16) as u8;
                if !is_virtio_block_device(vendor_id, device_id, class_code, subclass) {
                    continue;
                }

                let header_type = pci_cfg_read8(bdf, PCI_HEADER_TYPE_OFFSET) & 0x7F;
                let bar_slots = if header_type == 0x01 { 2 } else { 6 };
                let mut bar_idx = 0u16;
                while bar_idx < bar_slots {
                    let offset = PCI_BAR0_OFFSET + bar_idx * 4;
                    let raw = pci_cfg_read32(bdf, offset);
                    if let Some(base) = pci_mmio_bar_base(bdf, offset) {
                        if base <= usize::MAX as u64 {
                            return Some(base as usize);
                        }
                    }

                    let is_64_bit_mem_bar = raw & 1 == 0 && ((raw >> 1) & 0x3) == 0x2;
                    bar_idx += if is_64_bit_mem_bar { 2 } else { 1 };
                }
            }
        }
    }

    None
}

pub fn sys_pci_cfg_write_for_pid(pid: u32, offset: u16, value: u32) -> Result<(), PciCfgError> {
    pci_cfg_write32(claimed_bdf(pid)?, offset, value);
    Ok(())
}

pub fn sys_pci_bus_master_for_pid(pid: u32, enable: bool) -> Result<(), PciCfgError> {
    let bdf = claimed_bdf(pid)?;
    let mut command = pci_cfg_read16(bdf, PCI_COMMAND_OFFSET);

    if enable {
        command |= PCI_COMMAND_BUS_MASTER;
    } else {
        command &= !PCI_COMMAND_BUS_MASTER;
    }

    pci_cfg_write16(bdf, PCI_COMMAND_OFFSET, command);
    Ok(())
}

pub fn wait_bus_master_quiesced_for_pid(pid: u32, timeout_ms: u64) -> Result<bool, PciCfgError> {
    let bdf = claimed_bdf(pid)?;
    let pcie_cap = find_capability(bdf, PCI_CAP_ID_EXP);
    let deadline = now_ms().saturating_add(timeout_ms);

    loop {
        let command = pci_cfg_read16(bdf, PCI_COMMAND_OFFSET);
        let master_enabled = (command & PCI_COMMAND_BUS_MASTER) != 0;
        let tx_pending = pcie_cap
            .map(|cap| (pci_cfg_read16(bdf, cap + PCI_EXP_DEVSTA) & PCI_EXP_DEVSTA_TRPND) != 0)
            .unwrap_or(false);

        if !master_enabled && !tx_pending {
            return Ok(true);
        }

        if now_ms() >= deadline {
            return Ok(false);
        }

        spin_loop();
    }
}

pub fn sys_secondary_bus_reset_for_pid(pid: u32) -> Result<bool, PciCfgError> {
    let device_bdf = claimed_bdf(pid)?;
    let Some(bridge_bdf) = pci_topology::get_parent_bridge(topology_bdf(device_bdf)) else {
        return Ok(false);
    };
    let bridge_bdf = claim_bdf(bridge_bdf);

    let bridge_control = pci_cfg_read16(bridge_bdf, PCI_BRIDGE_CONTROL_OFFSET);
    pci_cfg_write16(
        bridge_bdf,
        PCI_BRIDGE_CONTROL_OFFSET,
        bridge_control | PCI_BRIDGE_CTL_BUS_RESET,
    );
    busy_wait_ms(2);
    pci_cfg_write16(
        bridge_bdf,
        PCI_BRIDGE_CONTROL_OFFSET,
        bridge_control & !PCI_BRIDGE_CTL_BUS_RESET,
    );
    Ok(true)
}

pub fn sys_wait_link_retraining_for_pid(pid: u32, timeout_ms: u64) -> Result<bool, PciCfgError> {
    let device_bdf = claimed_bdf(pid)?;
    let Some(bridge_bdf) = pci_topology::get_parent_bridge(topology_bdf(device_bdf)) else {
        busy_wait_ms(250);
        return Ok(true);
    };
    let bridge_bdf = claim_bdf(bridge_bdf);

    let Some(pcie_cap) = find_capability(bridge_bdf, PCI_CAP_ID_EXP) else {
        return Ok(false);
    };

    busy_wait_ms(100);
    let deadline = now_ms().saturating_add(timeout_ms);

    loop {
        let link_status = pci_cfg_read16(bridge_bdf, pcie_cap + PCI_EXP_LNKSTA);
        if link_status & PCI_EXP_LNKSTA_DLLLA != 0 {
            return Ok(true);
        }

        if now_ms() >= deadline {
            return Ok(false);
        }

        spin_loop();
    }
}
