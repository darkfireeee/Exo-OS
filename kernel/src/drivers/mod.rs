//! # drivers/mod.rs
//!
//! Driver Framework GI-03 - Point d'entrée principal
//! Agrège: IOMMU, DMA, PCI, MSI, MMIO, device claims

use pci_types;

pub mod iommu;
pub mod dma;
pub mod device_claims;
pub mod pci_topology;

#[cfg(test)]
pub mod tests;

// Re-export key types and functions
pub use dma::{sys_dma_alloc_for_pid, sys_dma_free_for_pid, sys_mmio_map_for_pid, sys_mmio_unmap_for_pid};
pub use iommu::{ensure_domain_for_pid, domain_of_pid, iommu_init};
pub use crate::memory::dma::core::types::IommuDomainId;
pub use device_claims::ClaimError;
pub use pci_topology::PciError as TopoError;

// Error type s (to be unified with real driver errors eventually)
#[derive(Clone, Copy, Debug)]
pub enum MmioError {
    PermissionDenied,
    AlreadyMapped,
    OutOfMemory,
}

#[derive(Clone, Copy, Debug)]
pub enum MsiError {
    NotFound,
    TableFull,
    AmbiguousClaim,
    NoSpace,
}

#[derive(Clone, Copy, Debug)]
pub enum PciCfgError {
    NotClaimed,
    PermissionDenied,
}

//  functions (to be properly implemented in GI-03 phases)

pub fn release_all_mmio_for_pid(_pid: u32) -> usize { 0 }
pub fn release_all_dma_for_pid(_pid: u32) -> usize { 0 }
pub fn release_all_msi_for_pid(_pid: u32) -> usize { 0 }

pub fn sys_dma_map(_phys: crate::memory::core::types::PhysAddr, _size: usize, _dir: crate::memory::dma::core::types::DmaDirection, _flags: crate::memory::dma::core::types::DmaMapFlags, _domain: IommuDomainId) -> Result<crate::memory::dma::core::types::IovaAddr, crate::memory::dma::core::types::DmaError> { Err(crate::memory::dma::core::types::DmaError::NotInitialized) }

pub fn sys_dma_unmap(_iova: crate::memory::dma::core::types::IovaAddr, _domain: IommuDomainId) -> Result<(), crate::memory::dma::core::types::DmaError> { Err(crate::memory::dma::core::types::DmaError::NotInitialized) }

pub fn sys_dma_sync_for_pid(_pid: u32, _iova: crate::memory::dma::core::types::IovaAddr, _size: usize, _dir: crate::memory::dma::core::types::DmaDirection) -> Result<(), crate::memory::dma::core::types::DmaError> { Err(crate::memory::dma::core::types::DmaError::NotInitialized) }

pub fn sys_pci_cfg_read_for_pid(_pid: u32, _offset: u16) -> Result<u32, PciCfgError> { Err(PciCfgError::NotClaimed) }
pub fn sys_pci_cfg_write_for_pid(_pid: u32, _offset: u16, _value: u32) -> Result<(), PciCfgError> { Err(PciCfgError::NotClaimed) }
// PCI bus master and link retraining (called by do_exit cleanup chain)
pub fn sys_pci_bus_master_for_pid(_pid: u32, _enable: bool) -> Result<(), PciCfgError> { Ok(()) }
pub fn wait_bus_master_quiesced_for_pid(_pid: u32, _timeout: u64) -> Result<bool, PciCfgError> { Ok(true) }
pub fn sys_secondary_bus_reset_for_pid(_pid: u32) -> Result<bool, PciCfgError> { Ok(false) }    
pub fn sys_wait_link_retraining_for_pid(_pid: u32, _timeout: u64) -> Result<bool, PciCfgError> { Ok(true) }

pub fn sys_pci_claim(bdf: pci_types::PciAddress, pid: u32) -> Result<(), ClaimError> {
    let custom_bdf = device_claims::PciBdf {
        bus: bdf.bus(),
        dev: bdf.device(),
        func: bdf.function(),
    };
    device_claims::sys_pci_claim(
        crate::memory::core::types::PhysAddr::new(0),
        0,
        pid,
        Some(custom_bdf),
        pid
    )
}

pub fn release_claims_for_pid(pid: u32) -> usize { 
    device_claims::revoke_claims_for_pid(pid);
    0
}

pub fn release_claim_for_owner(pid: u32) -> usize {
    device_claims::revoke_claims_for_pid(pid);
    0
}

pub fn sys_msi_alloc_for_pid(_pid: u32, _count: u16) -> Result<u64, MsiError> { Err(MsiError::TableFull) }
pub fn sys_msi_config_for_pid(_pid: u32, _handle: u64, _vector_idx: u16) -> Result<(), MsiError> { Err(MsiError::NotFound) }
pub fn sys_msi_free_for_pid(_pid: u32, _handle: u64) -> Result<(), MsiError> { Err(MsiError::NotFound) }

pub fn sys_pci_set_topology(bdf: pci_types::PciAddress, _pid: u32, parent: Option<pci_types::PciAddress>) -> Result<(), TopoError> { 
    let custom_child = pci_topology::PciBdf {
        bus: bdf.bus(),
        dev: bdf.device(),
        func: bdf.function(),
    };
    let custom_parent = parent.map(|p| pci_topology::PciBdf {
        bus: p.bus(),
        dev: p.device(),
        func: p.function(),
    }).unwrap_or(pci_topology::PciBdf { bus: 0, dev: 0, func: 0 });

    pci_topology::register_bridge_link(custom_child, custom_parent)
}
pub fn sys_pci_get_topology(_pid: u32, _dev: u32) -> Result<(u32, u16), TopoError> { Err(TopoError::TopologyTableFull) }
