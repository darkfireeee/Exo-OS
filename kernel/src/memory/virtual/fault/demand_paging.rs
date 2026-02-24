// kernel/src/memory/virtual/fault/demand_paging.rs
//
// Demand paging — alloue et mappe une page au premier accès.
//   - Anonymous  : zero-fill garanti (frame neuf du buddy + ZEROED flag)
//   - File       : délègue au FileFaultProvider enregistré (trait Couche 0)
//   - Device     : mappe la page physique fournie par DeviceFaultProvider
//   - Shared     : zero-fill avec marqueur COW si partageable
//   - Direct     : déjà mappé, ne devrait jamais arriver ici
//
// Couche 0 — aucune dépendance scheduler/process/ipc/fs.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use core::ptr::NonNull;

use crate::memory::core::{VirtAddr, PhysAddr, PageFlags, Frame, AllocFlags, PAGE_SIZE};
use crate::memory::virt::vma::{VmaDescriptor, VmaFlags, VmaBacking};
use super::{FaultContext, FaultResult};
use super::handler::FaultAllocator;

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES DE DEMAND PAGING
// ─────────────────────────────────────────────────────────────────────────────

pub struct DemandPagingStats {
    pub anon_faults:    AtomicU64,
    pub file_faults:    AtomicU64,
    pub device_faults:  AtomicU64,
    pub shared_faults:  AtomicU64,
    pub oom_count:      AtomicU64,
    pub map_errors:     AtomicU64,
    /// Total de pages allouées en zero-fill.
    pub zero_fill_pages: AtomicU64,
    /// Pages fournies par le FileFaultProvider.
    pub file_filled_pages: AtomicU64,
    /// Pages fournies par le DeviceFaultProvider.
    pub device_pages:   AtomicU64,
}

impl DemandPagingStats {
    pub const fn new() -> Self {
        DemandPagingStats {
            anon_faults:      AtomicU64::new(0),
            file_faults:      AtomicU64::new(0),
            device_faults:    AtomicU64::new(0),
            shared_faults:    AtomicU64::new(0),
            oom_count:        AtomicU64::new(0),
            map_errors:       AtomicU64::new(0),
            zero_fill_pages:  AtomicU64::new(0),
            file_filled_pages: AtomicU64::new(0),
            device_pages:     AtomicU64::new(0),
        }
    }
}

pub static DEMAND_PAGING_STATS: DemandPagingStats = DemandPagingStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// TRAIT FileFaultProvider (Couche 0 — injection de dépendance)
// ─────────────────────────────────────────────────────────────────────────────

/// Fournisseur de contenu pour les pages file-backed.
///
/// Implémenté par la couche VFS/page-cache hors Couche 0.
/// Enregistré via `register_file_fault_provider()`.
///
/// Contrat :
///   - `file_id`   : identifiant opaque du fichier (fourni par `VmaDescriptor.file_id`)
///   - `file_offset` : offset dans le fichier (en octets, aligné PAGE_SIZE)
///   - `dest_frame`  : frame physique destination, déjà alloué
///
/// L'implémentation doit remplir exactement PAGE_SIZE octets dans `dest_frame`.
/// Retourne `Ok(())` si le contenu a été chargé, `Err(_)` sinon.
pub trait FileFaultProvider: Sync {
    fn load_file_page(
        &self,
        file_id:     u64,
        file_offset: u64,
        dest_frame:  Frame,
    ) -> Result<(), crate::memory::core::AllocError>;
}

/// Fournisseur de frames pour les pages device-backed.
///
/// Enregistré via `register_device_fault_provider()`.
///
/// L'implémentation retourne le frame physique correspondant à
/// (device_id, offset) sans allouer de nouvelle mémoire physique.
pub trait DeviceFaultProvider: Sync {
    fn get_device_page(
        &self,
        device_id: u64,
        offset:    u64,
    ) -> Option<Frame>;
}

// ─────────────────────────────────────────────────────────────────────────────
// REGISTRES GLOBAUX (atomic pointer, Couche 0 safe)
// ─────────────────────────────────────────────────────────────────────────────

use core::sync::atomic::AtomicPtr;

/// Fournisseur de pages file-backed globalement enregistré.
/// Initialement nul (Couche 0 fonctionne sans lui — zero-fill fallback).
static FILE_FAULT_PROVIDER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
/// Vtable stockée séparément (fat pointer).
static FILE_FAULT_VTABLE:   AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Fournisseur de pages device-backed.
static DEVICE_FAULT_PROVIDER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static DEVICE_FAULT_VTABLE:   AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Enregistre un fournisseur de pages file-backed.
///
/// Doit être appelé une seule fois depuis la couche VFS (après init mémoire).
/// # Safety : appel single-threaded pendant l'init VFS.
pub unsafe fn register_file_fault_provider(
    data_ptr: *const (),
    vtable:   *const (),
) {
    FILE_FAULT_PROVIDER.store(data_ptr as *mut (), Ordering::Release);
    FILE_FAULT_VTABLE  .store(vtable   as *mut (), Ordering::Release);
}

/// Enregistre un fournisseur de frames device-backed.
/// # Safety : appel single-threaded pendant l'init drivers.
pub unsafe fn register_device_fault_provider(
    data_ptr: *const (),
    vtable:   *const (),
) {
    DEVICE_FAULT_PROVIDER.store(data_ptr as *mut (), Ordering::Release);
    DEVICE_FAULT_VTABLE  .store(vtable   as *mut (), Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// HELPER INTERNE : allouer + mapper une page zéro
// ─────────────────────────────────────────────────────────────────────────────

#[inline(always)]
fn alloc_zero_map<A: FaultAllocator>(
    page_addr: VirtAddr,
    flags:     PageFlags,
    alloc:     &A,
) -> FaultResult {
    let frame = match alloc.alloc_zeroed() {
        Ok(f)  => f,
        Err(_) => {
            DEMAND_PAGING_STATS.oom_count.fetch_add(1, Ordering::Relaxed);
            return FaultResult::Oom { addr: page_addr };
        }
    };
    DEMAND_PAGING_STATS.zero_fill_pages.fetch_add(1, Ordering::Relaxed);
    match alloc.map_page(page_addr, frame, flags) {
        Ok(_)  => FaultResult::Handled,
        Err(_) => {
            alloc.free_frame(frame);
            DEMAND_PAGING_STATS.map_errors.fetch_add(1, Ordering::Relaxed);
            FaultResult::Oom { addr: page_addr }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HANDLER PRINCIPAL
// ─────────────────────────────────────────────────────────────────────────────

/// Traite un fault de demand paging (page non présente, accès légitime sur VMA valide).
///
/// Dispatche selon `vma.backing` :
/// - `Anonymous` : zero-fill garanti
/// - `File`      : lecture via FileFaultProvider (fallback zero-fill si non enregistré)
/// - `Device`    : map frame physique fourni par DeviceFaultProvider
/// - `Shared`    : zero-fill + flags COW pour partage futur
/// - `Direct`    : erreur (ne doit pas arriver ici)
pub fn handle_demand_paging<A: FaultAllocator>(
    ctx:   &FaultContext,
    vma:   &VmaDescriptor,
    alloc: &A,
) -> FaultResult {
    let page_addr = VirtAddr::new(ctx.fault_addr.as_u64() & !(PAGE_SIZE as u64 - 1));

    match vma.backing {
        // ── ANONYMOUS : zero-fill ────────────────────────────────────────────
        VmaBacking::Anonymous => {
            DEMAND_PAGING_STATS.anon_faults.fetch_add(1, Ordering::Relaxed);
            let result = alloc_zero_map(page_addr, vma.page_flags, alloc);
            if result.is_handled() { vma.record_fault(); }
            result
        }

        // ── FILE-BACKED : FileFaultProvider ou zero-fill ─────────────────────
        VmaBacking::File => {
            DEMAND_PAGING_STATS.file_faults.fetch_add(1, Ordering::Relaxed);

            let data_ptr = FILE_FAULT_PROVIDER.load(Ordering::Acquire);
            let vtable   = FILE_FAULT_VTABLE  .load(Ordering::Acquire);

            if !data_ptr.is_null() && !vtable.is_null() {
                // Fournisseur enregistré — allouer un frame vide et le remplir.
                let frame = match alloc.alloc_nonzeroed() {
                    Ok(f)  => f,
                    Err(_) => {
                        DEMAND_PAGING_STATS.oom_count.fetch_add(1, Ordering::Relaxed);
                        return FaultResult::Oom { addr: ctx.fault_addr };
                    }
                };

                // Calcul de l'offset fichier depuis le debut de la VMA.
                let vma_offset = page_addr.as_u64()
                    .saturating_sub(vma.start.as_u64());
                let file_offset = vma.file_offset.saturating_add(vma_offset);

                // Reconstruction du fat pointer et appel au provider.
                // SAFETY: ptrs valides, enregistrés par register_file_fault_provider.
                let fat: (*const (), *const ()) = (data_ptr as *const (), vtable as *const ());
                let provider: &dyn FileFaultProvider = unsafe {
                    core::mem::transmute(fat)
                };
                match provider.load_file_page(vma.inode_id, file_offset, frame) {
                    Ok(()) => {
                        DEMAND_PAGING_STATS.file_filled_pages.fetch_add(1, Ordering::Relaxed);
                        match alloc.map_page(page_addr, frame, vma.page_flags) {
                            Ok(_) => { vma.record_fault(); FaultResult::Handled }
                            Err(_) => {
                                alloc.free_frame(frame);
                                DEMAND_PAGING_STATS.map_errors.fetch_add(1, Ordering::Relaxed);
                                FaultResult::Oom { addr: ctx.fault_addr }
                            }
                        }
                    }
                    Err(_) => {
                        // Erreur I/O : libérer le frame → zero-fill fallback.
                        alloc.free_frame(frame);
                        let result = alloc_zero_map(page_addr, vma.page_flags, alloc);
                        if result.is_handled() { vma.record_fault(); }
                        result
                    }
                }
            } else {
                // Pas de fournisseur — zero-fill (acceptable pendant boot).
                let result = alloc_zero_map(page_addr, vma.page_flags, alloc);
                if result.is_handled() { vma.record_fault(); }
                result
            }
        }

        // ── DEVICE : frame fourni par DeviceFaultProvider ───────────────────
        VmaBacking::Device => {
            DEMAND_PAGING_STATS.device_faults.fetch_add(1, Ordering::Relaxed);

            let data_ptr = DEVICE_FAULT_PROVIDER.load(Ordering::Acquire);
            let vtable   = DEVICE_FAULT_VTABLE  .load(Ordering::Acquire);

            if !data_ptr.is_null() && !vtable.is_null() {
                let vma_offset = page_addr.as_u64()
                    .saturating_sub(vma.start.as_u64());
                let device_offset = vma.file_offset.saturating_add(vma_offset);

                let fat: (*const (), *const ()) = (data_ptr as *const (), vtable as *const ());
                let provider: &dyn DeviceFaultProvider = unsafe {
                    core::mem::transmute(fat)
                };
                if let Some(dev_frame) = provider.get_device_page(vma.inode_id, device_offset) {
                    // Mapper avec flags device (no-cache, no-exec, présent).
                    let dev_flags = vma.page_flags | PageFlags::PRESENT | PageFlags::NO_CACHE;
                    DEMAND_PAGING_STATS.device_pages.fetch_add(1, Ordering::Relaxed);
                    match alloc.map_page(page_addr, dev_frame, dev_flags) {
                        Ok(_) => { vma.record_fault(); FaultResult::Handled }
                        Err(_) => {
                            DEMAND_PAGING_STATS.map_errors.fetch_add(1, Ordering::Relaxed);
                            FaultResult::Oom { addr: ctx.fault_addr }
                        }
                    }
                } else {
                    // Device ne fournit pas ce frame → SEGFAULT.
                    FaultResult::Segfault { addr: ctx.fault_addr }
                }
            } else {
                // Pas de provider device → zero-fill (config incomplète).
                let result = alloc_zero_map(page_addr, vma.page_flags, alloc);
                if result.is_handled() { vma.record_fault(); }
                result
            }
        }

        // ── SHARED : zero-fill + COW pour partage ────────────────────────────
        VmaBacking::Shared => {
            DEMAND_PAGING_STATS.shared_faults.fetch_add(1, Ordering::Relaxed);
            // Shared anonymous : zero-fill, mais les flags doivent rester COW
            // pour que la copie se déclenche si l'un des partageants écrit.
            let shared_flags = if vma.flags.contains(VmaFlags::WRITE) {
                vma.page_flags | PageFlags::COW
            } else {
                vma.page_flags
            };
            let result = alloc_zero_map(page_addr, shared_flags, alloc);
            if result.is_handled() { vma.record_fault(); }
            result
        }

        // ── DIRECT : déjà mappé, ne doit pas arriver ici ────────────────────
        VmaBacking::Direct => {
            // Le mapping physique direct aurait dû être établi à la création
            // de la VMA (mmap avec MAP_FIXED + physaddr). Un fault ici est
            // une incohérence : renvoyer SEGFAULT.
            FaultResult::Segfault { addr: ctx.fault_addr }
        }
    }
}
