//! # drivers/iommu/mod.rs
//!
//! Driver Framework GI-03 - Isolation Mémoire (IOMMU)
//!
//! Responsabilités :
//! 1. `fault_queue.rs` : Remontée robuste (lock-free) des accès IOMMU illégaux.

pub mod fault_queue;
pub mod fault_handler;
pub mod domain_registry;

use crate::memory::dma::core::mapping::IOVA_ALLOCATOR;
use crate::memory::dma::core::types::IommuDomainId;
use crate::memory::dma::iommu::IOMMU_DOMAINS;

pub fn ensure_domain_for_pid(pid: u32) -> Result<IommuDomainId, ()> {
    domain_registry::IOMMU_DOMAIN_REGISTRY.ensure_domain(pid)
}

pub fn domain_of_pid(pid: u32) -> Result<IommuDomainId, ()> {
    domain_registry::IOMMU_DOMAIN_REGISTRY.domain_of_pid(pid)
}

pub fn pid_of_domain(domain: IommuDomainId) -> Option<u32> {
    domain_registry::IOMMU_DOMAIN_REGISTRY.pid_of_domain(domain)
}

pub fn release_domain_for_pid(pid: u32) {
    domain_registry::IOMMU_DOMAIN_REGISTRY.release_domain(pid);
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
