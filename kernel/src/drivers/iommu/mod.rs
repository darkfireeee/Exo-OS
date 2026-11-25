//! # drivers/iommu/mod.rs — GI-03 PID-to-IOMMU Domain Ownership & Fault Queue
//!
//! Architectural boundary:
//! - `memory/dma/iommu/` (Layer 0): VT-d / AMD-Vi hardware registers, root/context tables,
//!   I/O page tables, and domain table storage.
//! - `drivers/iommu/` (GI-03 driver framework): PID ↔ `IommuDomainId` ownership leases
//!   (`pid_domain_registry.rs`), lock-free fault reporting (`fault_queue.rs`), and fault handler.

pub mod fault_handler;
pub mod fault_queue;
pub mod pid_domain_registry;
pub use pid_domain_registry as domain_registry;

use crate::memory::dma::core::mapping::IOVA_ALLOCATOR;
use crate::memory::dma::core::types::IommuDomainId;
use crate::memory::dma::iommu::IOMMU_DOMAINS;

pub fn ensure_domain_for_pid(pid: u32) -> Result<IommuDomainId, ()> {
    pid_domain_registry::IOMMU_DOMAIN_REGISTRY.ensure_domain(pid)
}

pub fn domain_of_pid(pid: u32) -> Result<IommuDomainId, ()> {
    pid_domain_registry::IOMMU_DOMAIN_REGISTRY.domain_of_pid(pid)
}

pub fn pid_of_domain(domain: IommuDomainId) -> Option<u32> {
    pid_domain_registry::IOMMU_DOMAIN_REGISTRY.pid_of_domain(domain)
}

pub fn release_domain_for_pid(pid: u32) {
    pid_domain_registry::IOMMU_DOMAIN_REGISTRY.release_domain(pid);
}

pub fn force_disable_domain(domain: IommuDomainId) {
    let _ = IOMMU_DOMAINS.with_domain_mut(domain, |dom| dom.deactivate());
}

pub fn disable_domain_atomic(domain: IommuDomainId) {
    let _ = IOMMU_DOMAINS.with_domain_mut(domain, |dom| dom.deactivate());
}

pub fn iommu_init() {
    fault_queue::IOMMU_FAULT_QUEUE.init();
    IOVA_ALLOCATOR.enable_iommu();
}
