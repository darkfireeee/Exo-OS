//! # drivers/iommu/mod.rs
//!
//! Driver Framework GI-03 - Isolation Mémoire (IOMMU)
//!
//! Responsabilités :
//! 1. `fault_queue.rs` : Remontée robuste (lock-free) des accès IOMMU illégaux.

pub mod fault_queue;
pub mod fault_handler;

use crate::memory::dma::core::types::IommuDomainId;

// Fonctions d`initialisation du fait du scope global du projet (à étendre)
pub fn ensure_domain_for_pid(_pid: u32) -> Result<IommuDomainId, ()> {
    Ok(IommuDomainId(1))
}

pub fn domain_of_pid(_pid: u32) -> Result<IommuDomainId, ()> {
    Ok(IommuDomainId(1))
}

pub fn iommu_init() {}
