// kernel/src/memory/dma/iommu/mod.rs
//
// IOMMU module — domains, page tables, Intel VT-d and AMD IOMMU drivers.

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
