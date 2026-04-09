//! # drivers/pci_cfg.rs
//!
//! Accès PCI configuration space + helpers de cleanup GI-03.

use core::hint::spin_loop;

use spin::Mutex;

use crate::arch::x86_64::{inl, irq_save, outl};
use crate::scheduler::timer::clock::monotonic_ns;

use super::device_claims::{self, PciBdf};
use super::pci_topology;
use super::PciCfgError;

const PCI_CFG_ADDR: u16 = 0xCF8;
const PCI_CFG_DATA: u16 = 0xCFC;

const PCI_COMMAND_OFFSET: u16 = 0x04;
const PCI_STATUS_OFFSET: u16 = 0x06;
const PCI_CAPABILITY_LIST_OFFSET: u16 = 0x34;
const PCI_BRIDGE_CONTROL_OFFSET: u16 = 0x3E;

const PCI_COMMAND_BUS_MASTER: u16 = 1 << 2;
const PCI_STATUS_CAP_LIST: u16 = 1 << 4;
const PCI_BRIDGE_CTL_BUS_RESET: u16 = 1 << 6;

const PCI_CAP_ID_EXP: u8 = 0x10;
const PCI_EXP_DEVSTA: u16 = 0x0A;
const PCI_EXP_DEVSTA_TRPND: u16 = 1 << 5;
const PCI_EXP_LNKSTA: u16 = 0x12;
const PCI_EXP_LNKSTA_DLLLA: u16 = 1 << 13;

static PCI_CFG_LOCK: Mutex<()> = Mutex::new(());

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
    pci_cfg_write16(bridge_bdf, PCI_BRIDGE_CONTROL_OFFSET, bridge_control | PCI_BRIDGE_CTL_BUS_RESET);
    busy_wait_ms(2);
    pci_cfg_write16(bridge_bdf, PCI_BRIDGE_CONTROL_OFFSET, bridge_control & !PCI_BRIDGE_CTL_BUS_RESET);
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
