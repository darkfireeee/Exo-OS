//! # memory/dma/iommu — Layer 0 Hardware IOMMU Engine
//!
//! Owns VT-d (`intel_vtd`), AMD-Vi (`amd_iommu`), I/O page tables (`page_table`),
//! and hardware domain descriptors (`domain`).
//! Per-PID ownership leases and fault queues live above in `crate::drivers::iommu`.

pub mod amd_iommu;
pub mod domain;
pub mod intel_vtd;
pub mod page_table;

pub use amd_iommu::{AmdDte, AmdIommuCmd, AmdIommuController, AMD_IOMMU};
pub use domain::{
    DomainType, IommuDomain, IommuDomainTable, PciBdf, IDENTITY_DOMAIN_ID, IOMMU_DOMAINS,
    MAX_DOMAINS,
};
pub use intel_vtd::{
    ContextEntry, ContextTable, DmarUnit, IntelVtd, RootEntry, RootTable, INTEL_VTD,
};
pub use page_table::{
    iommu_map, iommu_unmap, iommu_walk, IommuEntry, IommuFrameAlloc, IommuPageTable,
    IommuWalkResult,
};
