//! # drivers/dma.rs
//!
//! DMA Manager GI-03
//! Responsable : allocation DMA, mappings, cleanup
//! 
//! Spécification GI-03 §5 - sys_dma_map. ORDRE IMPÉRATIF : COW AVANT query_perms (FIX-68)
//! 0 TODO, 0 STUB.

use alloc::boxed::Box;
use alloc::vec::Vec;
use spin::{Mutex, RwLock};

use crate::memory::{
    alloc_page, alloc_pages, free_page, free_pages, phys_to_virt, AllocFlags, Frame, PageFlags,
    PhysAddr, VirtAddr,
};
use crate::memory::dma::core::types::{IommuDomainId, DmaDirection, IovaAddr, DmaError, DmaMapFlags};
use crate::memory::dma::core::mapping::IOVA_ALLOCATOR;
use crate::memory::physical::frame::descriptor::{FrameFlags, FRAME_DESCRIPTORS};
use crate::memory::virt::{
    handle_page_fault, shootdown_sync, FaultAllocator, FaultCause, FaultContext, FaultResult,
    TlbFlushType, UserAddressSpace, VmaBacking, VmaDescriptor, VmaFlags,
};
use crate::memory::virt::page_table::{FrameAllocatorForWalk, PageTableWalker, WalkResult};
use crate::process::core::pid::Pid;
use crate::process::PROCESS_REGISTRY;

use super::{device_claims, MmioError};

const PAGE_SIZE: usize = 4096;

/// Page physique verrouillée en mémoire (empêche le swap).
pub struct PinnedPage {
    pub phys: PhysAddr,
    frame: Frame,
}

impl PinnedPage {
    pub fn unpin(&self) {
        unpin_frame(self.frame);
    }
}

pub struct PageProtection {
    pub writable: bool,
}

impl PageProtection {
    pub const WRITE: Self = PageProtection { writable: true };
    #[inline]
    pub const fn requires_write(&self) -> bool {
        self.writable
    }

    pub fn is_writable(&self) -> bool {
        self.writable
    }
}

#[derive(Debug)]
pub enum CowError {
    OutOfMemory,
    InvalidAddress,
}

struct PinnedFrameRef {
    frame: Frame,
    refs: usize,
}

struct MmioRecord {
    pid: u32,
    phys_base: PhysAddr,
    size: usize,
    map_base: u64,
    virt_base: u64,
    map_size: usize,
}

static PINNED_FRAMES: RwLock<Vec<PinnedFrameRef>> = RwLock::new(Vec::new());
static MMIO_MAP_TABLE: RwLock<Vec<MmioRecord>> = RwLock::new(Vec::new());
static DMA_IOVA_SERIALIZER: Mutex<()> = Mutex::new(());

#[inline]
fn align_down(value: usize) -> usize {
    value & !(PAGE_SIZE - 1)
}

#[inline]
fn align_up(value: usize) -> usize {
    (value + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

fn checked_range(base: usize, size: usize) -> Option<(usize, usize)> {
    if size == 0 {
        return None;
    }

    let end = base.checked_add(size)?;
    Some((base, end))
}

fn ranges_overlap(lhs_base: usize, lhs_size: usize, rhs_base: usize, rhs_size: usize) -> bool {
    let Some((lhs_start, lhs_end)) = checked_range(lhs_base, lhs_size) else {
        return false;
    };
    let Some((rhs_start, rhs_end)) = checked_range(rhs_base, rhs_size) else {
        return false;
    };

    lhs_start < rhs_end && rhs_start < lhs_end
}

fn user_as_for_pid(pid: u32) -> Option<&'static UserAddressSpace> {
    let pcb = PROCESS_REGISTRY.find_by_pid(Pid(pid))?;
    let ptr = pcb.address_space_ptr() as *const UserAddressSpace;
    if ptr.is_null() {
        return None;
    }

    Some(unsafe { &*ptr })
}

fn pin_frame(frame: Frame) {
    let mut pinned = PINNED_FRAMES.write();
    if let Some(entry) = pinned.iter_mut().find(|entry| entry.frame == frame) {
        entry.refs += 1;
        return;
    }

    FRAME_DESCRIPTORS.get(frame).set_flag(FrameFlags::PINNED);
    pinned.push(PinnedFrameRef { frame, refs: 1 });
}

fn unpin_frame(frame: Frame) {
    let mut pinned = PINNED_FRAMES.write();
    let Some(pos) = pinned.iter().position(|entry| entry.frame == frame) else {
        return;
    };

    if pinned[pos].refs > 1 {
        pinned[pos].refs -= 1;
        return;
    }

    pinned.remove(pos);
    FRAME_DESCRIPTORS.get(frame).clear_flag(FrameFlags::PINNED);
}

struct UserFaultAllocator<'a> {
    user_as: &'a UserAddressSpace,
}

impl FrameAllocatorForWalk for UserFaultAllocator<'_> {
    fn alloc_frame(&self, flags: AllocFlags) -> Result<Frame, crate::memory::AllocError> {
        alloc_page(flags)
    }

    fn free_frame(&self, frame: Frame) {
        let _ = free_page(frame);
    }
}

impl FaultAllocator for UserFaultAllocator<'_> {
    fn alloc_zeroed(&self) -> Result<Frame, crate::memory::AllocError> {
        alloc_page(AllocFlags::ZEROED)
    }

    fn alloc_nonzeroed(&self) -> Result<Frame, crate::memory::AllocError> {
        alloc_page(AllocFlags::NONE)
    }

    fn free_frame(&self, frame: Frame) {
        let _ = free_page(frame);
    }

    fn map_page(
        &self,
        virt: VirtAddr,
        frame: Frame,
        flags: PageFlags,
    ) -> Result<(), crate::memory::AllocError> {
        unsafe { self.user_as.map_page(virt, frame, flags, self) }
    }

    fn remap_flags(
        &self,
        virt: VirtAddr,
        flags: PageFlags,
    ) -> Result<(), crate::memory::AllocError> {
        let mut walker = PageTableWalker::new(self.user_as.pml4_phys());
        walker.remap_flags(virt, flags)
    }

    fn translate(&self, virt: VirtAddr) -> Option<PhysAddr> {
        self.user_as.translate(virt)
    }
}

mod page_tables {
    use super::*;

    pub fn resolve_cow_or_fault(pid: u32, vaddr: usize, prot: PageProtection) -> Result<(), CowError> {
        let user_as = user_as_for_pid(pid).ok_or(CowError::InvalidAddress)?;
        let page_addr = VirtAddr::new(align_down(vaddr) as u64);
        let vma_ptr = user_as.find_vma(page_addr).ok_or(CowError::InvalidAddress)?;
        let vma = unsafe { &*vma_ptr };

        let cause = if prot.requires_write() {
            FaultCause::Write
        } else {
            FaultCause::Read
        };

        let needs_fault = user_as.translate(page_addr).is_none()
            || (matches!(cause, FaultCause::Write) && vma.flags.contains(VmaFlags::COW));
        if !needs_fault {
            return Ok(());
        }

        let alloc = UserFaultAllocator { user_as };
        let ctx = FaultContext::new(page_addr, cause, false).with_vma(vma_ptr);
        match handle_page_fault(&ctx, &alloc) {
            FaultResult::Handled => Ok(()),
            FaultResult::Oom { .. } => Err(CowError::OutOfMemory),
            FaultResult::Segfault { .. } | FaultResult::KernelFault { .. } => {
                Err(CowError::InvalidAddress)
            }
        }
    }
    
    pub fn query_perms_single(pid: u32, vaddr: usize) -> Option<PageProtection> {
        let user_as = user_as_for_pid(pid)?;
        let page_addr = VirtAddr::new(align_down(vaddr) as u64);
        let walker = PageTableWalker::new(user_as.pml4_phys());

        match walker.walk_read(page_addr) {
            WalkResult::Leaf { entry, .. } | WalkResult::HugePage { entry, .. } => {
                let flags = entry.to_page_flags();
                Some(PageProtection {
                    writable: flags.contains(PageFlags::WRITABLE),
                })
            }
            WalkResult::NotMapped => {
                let vma_ptr = user_as.find_vma(page_addr)?;
                let vma = unsafe { &*vma_ptr };
                Some(PageProtection {
                    writable: vma.flags.contains(VmaFlags::WRITE) || vma.flags.contains(VmaFlags::COW),
                })
            }
            WalkResult::AllocError(_) => None,
        }
    }
    
    pub fn pin_user_page(pid: u32, vaddr: usize) -> Option<PinnedPage> {
        let user_as = user_as_for_pid(pid)?;
        let page_addr = VirtAddr::new(align_down(vaddr) as u64);
        let phys = user_as.translate(page_addr)?;
        let frame = Frame::containing(phys);
        pin_frame(frame);

        Some(PinnedPage {
            phys: frame.start_address(),
            frame,
        })
    }
}

pub struct DmaRecord {
    pub pid: u32,
    pub domain: IommuDomainId,
    pub iova_base: IovaAddr,
    pub pinned_pages: Vec<PinnedPage>,
    pub size: usize,
}

pub static DMA_MAP_TABLE: RwLock<Vec<DmaRecord>> = RwLock::new(Vec::new());

pub struct DmaAllocRecord {
    pub pid: u32,
    pub domain: IommuDomainId,
    pub iova: IovaAddr,
    pub virt: u64,
    pub frame: Frame,
    pub order: usize,
}

pub static DMA_ALLOC_TABLE: RwLock<Vec<DmaAllocRecord>> = RwLock::new(Vec::new());

fn rollback_pinned_pages(pinned: &[PinnedPage]) {
    for page in pinned {
        page.unpin();
    }
}

fn mmio_page_flags() -> PageFlags {
    PageFlags::PRESENT
        | PageFlags::WRITABLE
        | PageFlags::USER
        | PageFlags::NO_CACHE
        | PageFlags::NO_EXECUTE
}

fn mmio_vma_flags() -> VmaFlags {
    VmaFlags::READ | VmaFlags::WRITE | VmaFlags::IO | VmaFlags::DONTCOPY | VmaFlags::DONTEXPAND
}

fn rollback_mmio_pages(user_as: &UserAddressSpace, map_base: VirtAddr, mapped_pages: usize) {
    for page_idx in 0..mapped_pages {
        let virt = VirtAddr::new(map_base.as_u64() + (page_idx * PAGE_SIZE) as u64);
        let _ = unsafe { user_as.unmap_page(virt) };
    }
}

fn unmap_mmio_record(record: MmioRecord) {
    let Some(user_as) = user_as_for_pid(record.pid) else {
        return;
    };

    if let Some(vma_ptr) = user_as.remove_vma(VirtAddr::new(record.map_base)) {
        let _ = unsafe { Box::from_raw(vma_ptr) };
    }

    for page_idx in 0..(record.map_size / PAGE_SIZE) {
        let virt = VirtAddr::new(record.map_base + (page_idx * PAGE_SIZE) as u64);
        let _ = unsafe { user_as.unmap_page(virt) };
    }

    let cpu_count = crate::arch::x86_64::acpi::madt::madt_cpu_count();
    unsafe {
        shootdown_sync(
            TlbFlushType::Range {
                start: VirtAddr::new(record.map_base),
                end: VirtAddr::new(record.map_base + record.map_size as u64),
            },
            cpu_count,
        );
    }
}

/// Mappe une plage virtuelle utilisateur en espace DMA/IOMMU.
/// FIX-68 Obligatoire : Résolution du Copy-On-Write (COW) avant l'interrogation des permissions.
pub fn sys_dma_map(
    pid: u32,
    vaddr: usize,
    size: usize,
    dir: DmaDirection,
    domain_id: IommuDomainId,
) -> Result<IovaAddr, DmaError> {
    if size == 0 {
        return Err(DmaError::InvalidParams);
    }

    let page_count = (size + PAGE_SIZE - 1) / PAGE_SIZE;
    let mut pinned: Vec<PinnedPage> = Vec::with_capacity(page_count);

    for i in 0..page_count {
        let vpage = vaddr + i * PAGE_SIZE;

        // Étape 1 : COW AVANT query_perms (FIX-68 obligatoire)
        if matches!(dir, DmaDirection::FromDevice | DmaDirection::Bidirection) {
            page_tables::resolve_cow_or_fault(pid, vpage, PageProtection::WRITE)
                .map_err(|e| {
                    rollback_pinned_pages(&pinned);
                    match e {
                        CowError::OutOfMemory => DmaError::OutOfMemory,
                        _ => DmaError::InvalidParams,
                    }
                })?;
        }

        // Étape 2 : Vérifier les permissions APRÈS COW
        let perms = page_tables::query_perms_single(pid, vpage)
            .ok_or_else(|| {
                rollback_pinned_pages(&pinned);
                DmaError::InvalidParams
            })?;

        if matches!(dir, DmaDirection::FromDevice | DmaDirection::Bidirection) && !perms.is_writable() {
            rollback_pinned_pages(&pinned);
            return Err(DmaError::IommuFault);
        }

        // Étape 3 : Épingler la page (empêche swap pendant DMA)
        let p = page_tables::pin_user_page(pid, vpage)
            .ok_or_else(|| {
                rollback_pinned_pages(&pinned);
                DmaError::InvalidParams
            })?;
        pinned.push(p);
    }

    // Étape 4-5 : Allouer des IOVAs contiguës et enregistrer chaque page épinglée.
    let _iova_guard = DMA_IOVA_SERIALIZER.lock();
    let mut mapped_count = 0usize;
    let mut iova_base = IovaAddr::zero();

    for (idx, pinned_page) in pinned.iter().enumerate() {
        let mapped_iova = match IOVA_ALLOCATOR.map(
            pinned_page.phys,
            PAGE_SIZE,
            dir,
            DmaMapFlags::NONE,
            domain_id,
        ) {
            Ok(iova) => iova,
            Err(err) => {
                for rollback_idx in 0..mapped_count {
                    let rollback_iova = IovaAddr(iova_base.as_u64() + (rollback_idx * PAGE_SIZE) as u64);
                    let _ = IOVA_ALLOCATOR.unmap(rollback_iova, domain_id);
                }
                rollback_pinned_pages(&pinned);
                return Err(err);
            }
        };

        if idx == 0 {
            iova_base = mapped_iova;
        } else if mapped_iova.as_u64() != iova_base.as_u64() + (idx * PAGE_SIZE) as u64 {
            let _ = IOVA_ALLOCATOR.unmap(mapped_iova, domain_id);
            for rollback_idx in 0..mapped_count {
                let rollback_iova = IovaAddr(iova_base.as_u64() + (rollback_idx * PAGE_SIZE) as u64);
                let _ = IOVA_ALLOCATOR.unmap(rollback_iova, domain_id);
            }
            rollback_pinned_pages(&pinned);
            return Err(DmaError::OutOfMemory);
        }

        mapped_count += 1;
    }

    DMA_MAP_TABLE.write().push(DmaRecord {
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
            let _ = IOVA_ALLOCATOR.unmap(u_iova, domain);
        }
        
        for p in record.pinned_pages {
            p.unpin();
        }
        Ok(())
    } else {
        Err(DmaError::InvalidParams)
    }
}

pub fn revoke_all_map_for_pid(pid: u32) -> usize {
    let mut table = DMA_MAP_TABLE.write();
    let mut i = 0;
    let mut released = 0usize;
    while i < table.len() {
        if table[i].pid == pid {
            let record = table.remove(i);
            let page_count = (record.size + PAGE_SIZE - 1) / PAGE_SIZE;
            for j in 0..page_count {
                let u_iova = IovaAddr((record.iova_base.as_u64()) + (j * PAGE_SIZE) as u64);
                let _ = IOVA_ALLOCATOR.unmap(u_iova, record.domain);
            }
            for p in record.pinned_pages {
                p.unpin();
            }
            released += 1;
        } else {
            i += 1;
        }
    }
    released
}

fn alloc_flags_from_dma_flags(flags: DmaMapFlags) -> AllocFlags {
    let mut alloc_flags = AllocFlags::ZEROED;

    if flags.contains(DmaMapFlags::DMA16) {
        alloc_flags = alloc_flags | AllocFlags::DMA;
    } else if flags.contains(DmaMapFlags::DMA32) {
        alloc_flags = alloc_flags | AllocFlags::DMA32;
    }

    alloc_flags
}

fn order_for_size(size: usize) -> usize {
    let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
    let mut order = 0usize;
    let mut covered_pages = 1usize;

    while covered_pages < pages {
        covered_pages <<= 1;
        order += 1;
    }

    order
}

pub fn revoke_all_alloc_for_pid(pid: u32) -> usize {
    let mut table = DMA_ALLOC_TABLE.write();
    let mut i = 0;
    let mut released = 0usize;

    while i < table.len() {
        if table[i].pid == pid {
            let record = table.remove(i);
            let _ = IOVA_ALLOCATOR.unmap(record.iova, record.domain);
            let _ = free_pages(record.frame, record.order);
            released += 1;
        } else {
            i += 1;
        }
    }

    released
}

pub fn revoke_all_for_pid(pid: u32) -> usize {
    revoke_all_map_for_pid(pid) + revoke_all_alloc_for_pid(pid)
}

// -----------------------------------------------------------------------------------------
// FONCTIONS DE COMPATIBILITÉ (POUR 0 ERROR COMPILE)
// -----------------------------------------------------------------------------------------

pub fn sys_dma_alloc_for_pid(
    pid: u32,
    size: usize,
    direction: DmaDirection,
    flags: DmaMapFlags,
    domain: IommuDomainId,
) -> Result<(u64, IovaAddr), DmaError> {
    if size == 0 {
        return Err(DmaError::InvalidParams);
    }

    let order = order_for_size(size);
    let frame = alloc_pages(order, alloc_flags_from_dma_flags(flags))
        .map_err(|_| DmaError::OutOfMemory)?;
    let phys = frame.start_address();
    let virt = phys_to_virt(phys).as_u64();

    match IOVA_ALLOCATOR.map(phys, size, direction, flags, domain) {
        Ok(iova) => {
            DMA_ALLOC_TABLE.write().push(DmaAllocRecord {
                pid,
                domain,
                iova,
                virt,
                frame,
                order,
            });
            Ok((virt, iova))
        }
        Err(err) => {
            let _ = free_pages(frame, order);
            Err(err)
        }
    }
}

pub fn sys_dma_free_for_pid(
    pid: u32,
    iova: IovaAddr,
    domain: IommuDomainId,
) -> Result<(), DmaError> {
    let mut table = DMA_ALLOC_TABLE.write();
    let Some(pos) = table
        .iter()
        .position(|record| record.pid == pid && record.iova == iova && record.domain == domain) else {
        return Err(DmaError::InvalidParams);
    };

    let record = table.remove(pos);
    IOVA_ALLOCATOR.unmap(record.iova, record.domain)?;
    let _ = free_pages(record.frame, record.order);
    Ok(())
}

pub fn sys_dma_sync_for_pid(
    pid: u32, 
    iova: IovaAddr, 
    size: usize, 
    dir: DmaDirection
) -> Result<(), DmaError> {
    let owned_alloc = DMA_ALLOC_TABLE.read().iter().any(|record| record.pid == pid && record.iova == iova);
    let owned_map = DMA_MAP_TABLE.read().iter().any(|record| record.pid == pid && record.iova_base == iova);

    if !owned_alloc && !owned_map {
        return Err(DmaError::InvalidParams);
    }

    match dir {
        DmaDirection::ToDevice => IOVA_ALLOCATOR.sync_for_device(iova, size),
        DmaDirection::FromDevice => IOVA_ALLOCATOR.sync_for_cpu(iova, size),
        DmaDirection::Bidirection => {
            IOVA_ALLOCATOR.sync_for_device(iova, size);
            IOVA_ALLOCATOR.sync_for_cpu(iova, size);
        }
        DmaDirection::None => {}
    }

    Ok(())
}

pub fn sys_mmio_map_for_pid(_pid: u32, _phys: PhysAddr, _size: usize) -> Result<u64, MmioError> {
    let pid = _pid;
    let phys = _phys;
    let size = _size;

    if size == 0 {
        return Err(MmioError::InvalidParams);
    }

    if !device_claims::claim_contains(pid, phys, size) {
        return Err(MmioError::PermissionDenied);
    }

    let user_as = user_as_for_pid(pid).ok_or(MmioError::PermissionDenied)?;
    let offset = (phys.as_u64() as usize) & (PAGE_SIZE - 1);
    let map_phys_base = PhysAddr::new(align_down(phys.as_u64() as usize) as u64);
    let map_size = align_up(size + offset);

    {
        let mmio = MMIO_MAP_TABLE.read();
        if mmio.iter().any(|record| {
            record.pid == pid
                && ranges_overlap(
                    record.phys_base.as_u64() as usize,
                    record.map_size,
                    map_phys_base.as_u64() as usize,
                    map_size,
                )
        }) {
            return Err(MmioError::AlreadyMapped);
        }
    }

    let Some(map_base) = user_as.find_free_gap(map_size, None) else {
        return Err(MmioError::OutOfMemory);
    };

    let page_flags = mmio_page_flags();
    let mapped_pages = map_size / PAGE_SIZE;
    let alloc = UserFaultAllocator { user_as };

    for page_idx in 0..mapped_pages {
        let virt = VirtAddr::new(map_base.as_u64() + (page_idx * PAGE_SIZE) as u64);
        let phys_page = Frame::containing(PhysAddr::new(
            map_phys_base.as_u64() + (page_idx * PAGE_SIZE) as u64,
        ));

        if unsafe { user_as.map_page(virt, phys_page, page_flags, &alloc) }.is_err() {
            rollback_mmio_pages(user_as, map_base, page_idx);
            return Err(MmioError::OutOfMemory);
        }
    }

    let vma = Box::new(VmaDescriptor::new(
        map_base,
        VirtAddr::new(map_base.as_u64() + map_size as u64),
        mmio_vma_flags(),
        page_flags,
        VmaBacking::Device,
    ));
    let vma_ptr = Box::into_raw(vma);
    if !unsafe { user_as.insert_vma(vma_ptr) } {
        rollback_mmio_pages(user_as, map_base, mapped_pages);
        let _ = unsafe { Box::from_raw(vma_ptr) };
        return Err(MmioError::AlreadyMapped);
    }

    let virt_base = map_base.as_u64() + offset as u64;
    MMIO_MAP_TABLE.write().push(MmioRecord {
        pid,
        phys_base: map_phys_base,
        size,
        map_base: map_base.as_u64(),
        virt_base,
        map_size,
    });

    Ok(virt_base)
}

pub fn sys_mmio_unmap_for_pid(_pid: u32, _virt_addr: u64, _size: usize) -> Result<(), MmioError> {
    let mut table = MMIO_MAP_TABLE.write();
    let Some(pos) = table
        .iter()
        .position(|record| record.pid == _pid && record.virt_base == _virt_addr && record.size == _size) else {
        return Err(MmioError::NotMapped);
    };

    let record = table.remove(pos);
    drop(table);
    unmap_mmio_record(record);
    Ok(())
}

pub fn revoke_all_mmio(_pid: u32) {
    let mut table = MMIO_MAP_TABLE.write();
    let mut drained = Vec::new();
    let mut idx = 0usize;

    while idx < table.len() {
        if table[idx].pid == _pid {
            drained.push(table.remove(idx));
        } else {
            idx += 1;
        }
    }
    drop(table);

    for record in drained {
        unmap_mmio_record(record);
    }
}
