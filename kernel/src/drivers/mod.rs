//! # drivers/mod.rs
//!
//! Driver Framework GI-03 - Point d'entrée principal
//! Agrège: IOMMU, DMA, PCI, MSI, MMIO, device claims

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use pci_types;
use spin::RwLock;

pub mod iommu;
pub mod dma;
pub mod device_claims;
pub mod pci_topology;
mod pci_cfg;

#[cfg(test)]
pub mod tests;

// Re-export key types and functions
pub use dma::{sys_dma_alloc_for_pid, sys_dma_free_for_pid, sys_mmio_map_for_pid, sys_mmio_unmap_for_pid};
pub use iommu::{ensure_domain_for_pid, domain_of_pid, iommu_init, pid_of_domain, release_domain_for_pid};
pub use crate::memory::dma::core::types::IommuDomainId;
pub use device_claims::ClaimError;
pub use pci_topology::PciError as TopoError;

// Error type s (to be unified with real driver errors eventually)
#[derive(Clone, Copy, Debug)]
pub enum MmioError {
    PermissionDenied,
    AlreadyMapped,
    OutOfMemory,
    NotMapped,
    InvalidParams,
}

#[derive(Clone, Copy, Debug)]
pub enum MsiError {
    NotFound,
    TableFull,
    AmbiguousClaim,
    NoSpace,
    InvalidParams,
}

#[derive(Clone, Copy, Debug)]
pub enum PciCfgError {
    NotClaimed,
    PermissionDenied,
}

const MAX_MSI_HANDLES: usize = 256;

struct MsiLease {
    handle: u64,
    pid: u32,
    count: u16,
    configured_mask: u64,
}

static NEXT_MSI_HANDLE: AtomicU64 = AtomicU64::new(1);
static MSI_LEASES: RwLock<Vec<MsiLease>> = RwLock::new(Vec::new());

pub fn release_all_mmio_for_pid(pid: u32) -> usize {
    dma::revoke_all_mmio(pid);
    0
}

pub fn release_all_dma_for_pid(pid: u32) -> usize {
    dma::revoke_all_for_pid(pid)
}

pub fn release_all_msi_for_pid(pid: u32) -> usize {
    let mut leases = MSI_LEASES.write();
    let before = leases.len();
    leases.retain(|lease| lease.pid != pid);
    before - leases.len()
}

pub fn sys_dma_map(
    pid: u32,
    vaddr: usize,
    size: usize,
    dir: crate::memory::dma::core::types::DmaDirection,
) -> Result<crate::memory::dma::core::types::IovaAddr, crate::memory::dma::core::types::DmaError> {
    let domain = iommu::domain_of_pid(pid)
        .map_err(|_| crate::memory::dma::core::types::DmaError::NotInitialized)?;
    dma::sys_dma_map(pid, vaddr, size, dir, domain)
}

pub fn sys_dma_unmap(
    pid: u32,
    iova: crate::memory::dma::core::types::IovaAddr,
    domain: IommuDomainId,
) -> Result<(), crate::memory::dma::core::types::DmaError> {
    if pid != 0 {
        let expected = iommu::domain_of_pid(pid)
            .map_err(|_| crate::memory::dma::core::types::DmaError::NotInitialized)?;
        if expected != domain {
            return Err(crate::memory::dma::core::types::DmaError::InvalidParams);
        }
    }

    dma::sys_dma_unmap(iova, domain)
}

pub fn sys_dma_sync_for_pid(
    pid: u32,
    iova: crate::memory::dma::core::types::IovaAddr,
    size: usize,
    dir: crate::memory::dma::core::types::DmaDirection,
) -> Result<(), crate::memory::dma::core::types::DmaError> {
    dma::sys_dma_sync_for_pid(pid, iova, size, dir)
}

pub fn sys_pci_cfg_read_for_pid(pid: u32, offset: u16) -> Result<u32, PciCfgError> {
    pci_cfg::sys_pci_cfg_read_for_pid(pid, offset)
}

pub fn sys_pci_cfg_write_for_pid(pid: u32, offset: u16, value: u32) -> Result<(), PciCfgError> {
    pci_cfg::sys_pci_cfg_write_for_pid(pid, offset, value)
}

pub fn sys_pci_bus_master_for_pid(pid: u32, enable: bool) -> Result<(), PciCfgError> {
    pci_cfg::sys_pci_bus_master_for_pid(pid, enable)
}

pub fn wait_bus_master_quiesced_for_pid(pid: u32, timeout: u64) -> Result<bool, PciCfgError> {
    pci_cfg::wait_bus_master_quiesced_for_pid(pid, timeout)
}

pub fn sys_secondary_bus_reset_for_pid(pid: u32) -> Result<bool, PciCfgError> {
    pci_cfg::sys_secondary_bus_reset_for_pid(pid)
}

pub fn sys_wait_link_retraining_for_pid(pid: u32, timeout: u64) -> Result<bool, PciCfgError> {
    pci_cfg::sys_wait_link_retraining_for_pid(pid, timeout)
}

pub fn sys_pci_claim(
    phys_base: crate::memory::core::types::PhysAddr,
    size: usize,
    pid: u32,
    bdf: Option<pci_types::PciAddress>,
    calling_pid: u32,
) -> Result<(), ClaimError> {
    let custom_bdf = bdf.map(|bdf| device_claims::PciBdf {
        bus: bdf.bus(),
        dev: bdf.device(),
        func: bdf.function(),
    });

    device_claims::sys_pci_claim(phys_base, size, pid, custom_bdf, calling_pid)
}

pub fn release_claims_for_pid(pid: u32) -> usize { 
    device_claims::revoke_claims_for_pid(pid);
    0
}

pub fn release_claim_for_owner(pid: u32) -> usize {
    device_claims::revoke_claims_for_pid(pid);
    iommu::release_domain_for_pid(pid);
    0
}

pub fn sys_msi_alloc_for_pid(pid: u32, count: u16) -> Result<u64, MsiError> {
    if count == 0 || count as usize > u64::BITS as usize {
        return Err(MsiError::InvalidParams);
    }

    if device_claims::bdf_of_pid(pid).is_none() {
        return Err(MsiError::AmbiguousClaim);
    }

    let mut leases = MSI_LEASES.write();
    if leases.len() >= MAX_MSI_HANDLES {
        return Err(MsiError::TableFull);
    }

    let handle = NEXT_MSI_HANDLE.fetch_add(1, Ordering::Relaxed);
    leases.push(MsiLease {
        handle,
        pid,
        count,
        configured_mask: 0,
    });
    Ok(handle)
}

pub fn sys_msi_config_for_pid(pid: u32, handle: u64, vector_idx: u16) -> Result<(), MsiError> {
    let mut leases = MSI_LEASES.write();
    let Some(lease) = leases.iter_mut().find(|lease| lease.handle == handle) else {
        return Err(MsiError::NotFound);
    };

    if lease.pid != pid {
        return Err(MsiError::NotFound);
    }
    if vector_idx >= lease.count {
        return Err(MsiError::InvalidParams);
    }

    lease.configured_mask |= 1u64 << vector_idx;
    Ok(())
}

pub fn sys_msi_free_for_pid(pid: u32, handle: u64) -> Result<(), MsiError> {
    let mut leases = MSI_LEASES.write();
    let Some(pos) = leases.iter().position(|lease| lease.handle == handle && lease.pid == pid) else {
        return Err(MsiError::NotFound);
    };

    leases.remove(pos);
    Ok(())
}

pub fn sys_pci_set_topology(
    bdf: pci_types::PciAddress,
    parent: Option<pci_types::PciAddress>,
) -> Result<(), TopoError> {
    let custom_child = pci_topology::PciBdf {
        bus: bdf.bus(),
        dev: bdf.device(),
        func: bdf.function(),
    };

    let Some(custom_parent) = parent.map(|p| pci_topology::PciBdf {
        bus: p.bus(),
        dev: p.device(),
        func: p.function(),
    }) else {
        return Ok(());
    };

    pci_topology::register_bridge_link(custom_child, custom_parent)
}
pub fn sys_pci_get_topology(pid: u32, _dev: u32) -> Result<(u32, u16), TopoError> {
    let Some(child) = device_claims::bdf_of_pid(pid) else {
        return Err(TopoError::TopologyTableFull);
    };
    let Some(parent) = pci_topology::get_parent_bridge(pci_topology::PciBdf {
        bus: child.bus,
        dev: child.dev,
        func: child.func,
    }) else {
        return Err(TopoError::TopologyTableFull);
    };

    let encoded = ((parent.bus as u32) << 8) | ((parent.dev as u32) << 3) | parent.func as u32;
    Ok((encoded, 0))
}
