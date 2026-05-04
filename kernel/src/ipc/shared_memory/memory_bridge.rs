// ipc/shared_memory/memory_bridge.rs — Pont IPC -> memory pour map_shm_into_process.
//
// La couche memory ne doit pas importer les tables IPC. Ce module expose les
// callbacks inverses: IPC garde ses descripteurs et memory reçoit seulement les
// informations nécessaires au mappage virtuel.

use core::sync::atomic::Ordering;

use crate::ipc::shared_memory::descriptor::SHM_DESC_DIR;
use crate::ipc::shared_memory::mapping::SHM_MAPPING_TABLE;
use crate::memory::virt::mmap::{
    register_shm_provider, ShmMapError, ShmProviderFns, ShmRegionInfo,
};

pub fn register_with_memory() {
    register_shm_provider(ShmProviderFns {
        region_info,
        page_phys,
        release_region,
        register_mapping,
    });
}

fn region_info(desc_idx: usize, writable: bool) -> Result<ShmRegionInfo, ShmMapError> {
    let dir = SHM_DESC_DIR.lock();
    let desc = unsafe { dir.get(desc_idx) }.ok_or(ShmMapError::InvalidRegion)?;
    if !desc.is_active() {
        return Err(ShmMapError::InvalidRegion);
    }
    if writable && (desc.permissions & 0x2 == 0) {
        return Err(ShmMapError::PermissionDenied);
    }

    let n_pages = desc.page_count();
    if n_pages == 0 {
        return Err(ShmMapError::InvalidRegion);
    }

    desc.add_mapping();
    Ok(ShmRegionInfo {
        n_pages,
        size_bytes: desc.size_bytes as usize,
    })
}

fn page_phys(desc_idx: usize, page_idx: usize) -> Option<u64> {
    let dir = SHM_DESC_DIR.lock();
    let desc = unsafe { dir.get(desc_idx) }?;
    if !desc.is_active() {
        return None;
    }
    desc.page_phys(page_idx).map(|phys| phys.0)
}

fn release_region(desc_idx: usize) {
    let dir = SHM_DESC_DIR.lock();
    if let Some(desc) = unsafe { dir.get(desc_idx) } {
        desc.remove_mapping();
    }
}

fn register_mapping(
    desc_idx: usize,
    pid: u32,
    virt_base: u64,
    writable: bool,
    n_pages: usize,
) -> Result<usize, ShmMapError> {
    {
        let dir = SHM_DESC_DIR.lock();
        let desc = unsafe { dir.get(desc_idx) }.ok_or(ShmMapError::InvalidRegion)?;
        if !desc.is_active() || desc.page_count() != n_pages {
            return Err(ShmMapError::InvalidRegion);
        }
    }

    let mapping_idx = {
        let mut tbl = SHM_MAPPING_TABLE.lock();
        tbl.alloc().ok_or(ShmMapError::AllocFailed)?
    };

    let mut tbl = SHM_MAPPING_TABLE.lock();
    if let Some(mapping) = tbl.entry(mapping_idx) {
        mapping.desc_idx.store(desc_idx as u32, Ordering::Relaxed);
        mapping.process_id.store(pid, Ordering::Relaxed);
        mapping.virt_base.store(virt_base, Ordering::Relaxed);
        mapping
            .permissions
            .store(if writable { 0x3 } else { 0x1 }, Ordering::Relaxed);
        mapping
            .mapped_pages
            .store(n_pages as u32, Ordering::Relaxed);
        mapping.mark_active();
        Ok(mapping_idx)
    } else {
        tbl.free(mapping_idx);
        Err(ShmMapError::AllocFailed)
    }
}
