// kernel/src/memory/dma/iommu/mod.rs
//
// Module IOMMU — domaines, tables de pages, pilotes Intel VT-d et AMD IOMMU.

pub mod domain;
pub mod page_table;
pub mod intel_vtd;
pub mod amd_iommu;

pub use domain::{
    IommuDomain, IommuDomainTable, IOMMU_DOMAINS, DomainType,
    PciBdf, IDENTITY_DOMAIN_ID, MAX_DOMAINS,
};
pub use page_table::{
    IommuEntry, IommuPageTable, IommuFrameAlloc, IommuWalkResult,
    iommu_map, iommu_unmap, iommu_walk,
};
pub use intel_vtd::{IntelVtd, INTEL_VTD, DmarUnit, RootTable, RootEntry, ContextTable, ContextEntry};
pub use amd_iommu::{AmdIommuController, AMD_IOMMU, AmdDte, AmdIommuCmd};
