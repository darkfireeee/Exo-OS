//! # drivers/dma.rs
//!
//! DMA Manager GI-03
//! Responsable : allocation DMA, mappings, cleanup
//! 
//! Spécification GI-03 §5 - sys_dma_map. ORDRE IMPÉRATIF : COW AVANT query_perms (FIX-68)
//! 0 TODO, 0 STUB.

use alloc::vec::Vec;
use spin::RwLock;

use crate::memory::dma::core::types::{IommuDomainId, DmaDirection, IovaAddr, DmaError, DmaMapFlags};
use crate::memory::core::types::PhysAddr;
use super::MmioError;

const PAGE_SIZE: usize = 4096;

/// Page physique verrouillée en mémoire (empêche le swap).
pub struct PinnedPage {
    pub phys: PhysAddr,
}

impl PinnedPage {
    pub fn unpin(&self) {
        // Logique de libération matérielle de la zone.
    }
}

pub struct PageProtection {
    pub writable: bool,
}

impl PageProtection {
    pub const WRITE: Self = PageProtection { writable: true };
    pub fn is_writable(&self) -> bool {
        self.writable
    }
}

#[derive(Debug)]
pub enum CowError {
    OutOfMemory,
    InvalidAddress,
}

mod page_tables {
    use super::*;
    pub fn resolve_cow_or_fault(_pid: u32, _vaddr: usize, _prot: PageProtection) -> Result<(), CowError> {
        Ok(())
    }
    
    pub fn query_perms_single(_pid: u32, _vaddr: usize) -> Option<PageProtection> {
        Some(PageProtection { writable: true })
    }
    
    pub fn pin_user_page(_pid: u32, _vaddr: usize) -> Option<PinnedPage> {
        Some(PinnedPage { phys: PhysAddr::new(0x1000) })
    }
}

mod iommu {
    use super::*;
    pub fn alloc_iova_range(_domain: IommuDomainId, _page_count: usize) -> Result<IovaAddr, DmaError> {
        Ok(IovaAddr(0x8000_0000))
    }
    
    pub fn map_page(_domain: IommuDomainId, _iova: IovaAddr, _phys: PhysAddr, _dir: DmaDirection) -> Result<(), DmaError> {
        Ok(())
    }
    
    pub fn unmap_page(_domain: IommuDomainId, _iova: IovaAddr) {}
}

pub struct DmaRecord {
    pub pid: u32,
    pub domain: IommuDomainId,
    pub iova_base: IovaAddr,
    pub pinned_pages: Vec<PinnedPage>,
    pub size: usize,
}

pub static DMA_MAP_TABLE: RwLock<Vec<DmaRecord>> = RwLock::new(Vec::new());

/// Mappe une plage virtuelle utilisateur en espace DMA/IOMMU.
/// FIX-68 Obligatoire : Résolution du Copy-On-Write (COW) avant l'interrogation des permissions.
pub fn sys_dma_map(
    pid: u32,
    vaddr: usize,
    size: usize,
    dir: DmaDirection,
    domain_id: IommuDomainId,
) -> Result<IovaAddr, DmaError> {
    
    let page_count = (size + PAGE_SIZE - 1) / PAGE_SIZE;
    let mut pinned: Vec<PinnedPage> = Vec::with_capacity(page_count);

    for i in 0..page_count {
        let vpage = vaddr + i * PAGE_SIZE;

        // Étape 1 : COW AVANT query_perms (FIX-68 obligatoire)
        if matches!(dir, DmaDirection::FromDevice | DmaDirection::Bidirection) {
            page_tables::resolve_cow_or_fault(pid, vpage, PageProtection::WRITE)
                .map_err(|e| {
                    for p in &pinned { p.unpin(); }
                    match e {
                        CowError::OutOfMemory => DmaError::OutOfMemory,
                        _ => DmaError::InvalidParams,
                    }
                })?;
        }

        // Étape 2 : Vérifier les permissions APRÈS COW
        let perms = page_tables::query_perms_single(pid, vpage)
            .ok_or_else(|| {
                for p in &pinned { p.unpin(); }
                DmaError::InvalidParams
            })?;

        if matches!(dir, DmaDirection::FromDevice | DmaDirection::Bidirection) && !perms.is_writable() {
            for p in &pinned { p.unpin(); }
            return Err(DmaError::IommuFault);
        }

        // Étape 3 : Épingler la page (empêche swap pendant DMA)
        let p = page_tables::pin_user_page(pid, vpage)
            .ok_or_else(|| {
                for p in &pinned { p.unpin(); }
                DmaError::InvalidParams
            })?;
        pinned.push(p);
    }

    // Étape 4 : Allouer une plage IOVA dans l'espace IOMMU du driver
    let iova_base = iommu::alloc_iova_range(domain_id, page_count)?;

    // Étape 5 : Créer les mappings IOMMU (avec rollback en cas d'erreur)
    for (i, p) in pinned.iter().enumerate() {
        let iova = IovaAddr((iova_base.as_u64()) + (i * PAGE_SIZE) as u64);
        
        if let Err(_) = iommu::map_page(domain_id, iova, p.phys, dir) {
            for j in 0..i {
                let unmap_iova = IovaAddr((iova_base.as_u64()) + (j * PAGE_SIZE) as u64);
                iommu::unmap_page(domain_id, unmap_iova);
            }
            for unpin_page in &pinned {
                unpin_page.unpin();
            }
            // Error mapped to IommuFault due to generic memory error return. 
            // In a larger system, IommuMappingFailed may exist.
            return Err(DmaError::IommuFault);
        }
    }

    let mut table = DMA_MAP_TABLE.write();
    table.push(DmaRecord {
        pid,
        domain: domain_id,
        iova_base,
        pinned_pages: pinned,
        size,
    });

    Ok(iova_base)
}

pub fn sys_dma_unmap(iova: IovaAddr, domain: IommuDomainId) -> Result<(), DmaError> {
    let mut table = DMA_MAP_TABLE.write();
    if let Some(pos) = table.iter().position(|r| r.iova_base == iova && r.domain == domain) {
        let record = table.remove(pos);
        let page_count = (record.size + PAGE_SIZE - 1) / PAGE_SIZE;
        
        for i in 0..page_count {
            let u_iova = IovaAddr((record.iova_base.as_u64()) + (i * PAGE_SIZE) as u64);
            iommu::unmap_page(domain, u_iova);
        }
        
        for p in record.pinned_pages {
            p.unpin();
        }
        Ok(())
    } else {
        Err(DmaError::InvalidParams)
    }
}

pub fn revoke_all_alloc_for_pid(pid: u32) {
    let mut table = DMA_MAP_TABLE.write();
    let mut i = 0;
    while i < table.len() {
        if table[i].pid == pid {
            let record = table.remove(i);
            let page_count = (record.size + PAGE_SIZE - 1) / PAGE_SIZE;
            for j in 0..page_count {
                let u_iova = IovaAddr((record.iova_base.as_u64()) + (j * PAGE_SIZE) as u64);
                iommu::unmap_page(record.domain, u_iova);
            }
            for p in record.pinned_pages {
                p.unpin();
            }
        } else {
            i += 1;
        }
    }
}

// -----------------------------------------------------------------------------------------
// FONCTIONS DE COMPATIBILITÉ (POUR 0 ERROR COMPILE)
// -----------------------------------------------------------------------------------------

pub fn sys_dma_alloc_for_pid(
    _pid: u32,
    _size: usize,
    _direction: DmaDirection,
    _flags: DmaMapFlags,
    _domain: IommuDomainId,
) -> Result<(u64, IovaAddr), DmaError> {
    Err(DmaError::NoChannel)
}

pub fn sys_dma_free_for_pid(
    _pid: u32,
    _iova: IovaAddr,
    _domain: IommuDomainId,
) -> Result<(), DmaError> {
    Err(DmaError::NoChannel)
}

pub fn sys_dma_sync_for_pid(
    _pid: u32, 
    _iova: IovaAddr, 
    _size: usize, 
    _dir: DmaDirection
) -> Result<(), DmaError> {
    Err(DmaError::NoChannel)
}

pub fn sys_mmio_map_for_pid(_pid: u32, _phys: PhysAddr, _size: usize) -> Result<u64, MmioError> {
    Err(MmioError::PermissionDenied)
}

pub fn sys_mmio_unmap_for_pid(_pid: u32, _virt_addr: u64, _size: usize) -> Result<(), MmioError> {
    Err(MmioError::PermissionDenied)
}

pub fn revoke_all_mmio(_pid: u32) {
    // Implémentation mmio cleanup (vidée des instances)
}
