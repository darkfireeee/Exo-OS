// kernel/src/memory/virtual/mmap.rs
//
// ──────────────────────────────────────────────────────────────────────────────
// IMPLÉMENTATION DE MMAP / MUNMAP / MPROTECT / BRK
// ──────────────────────────────────────────────────────────────────────────────
//
// Implémente les appels système de gestion de mémoire virtuelle utilisateur.
// La couche syscall injecte un pointeur de fonction vers l'espace d'adressage
// courant via `register_current_as_getter()`.
//
// COUCHE 0 — aucune dépendance scheduler/process/ipc/fs.

use alloc::boxed::Box;
use core::sync::atomic::{AtomicPtr, Ordering};

use crate::memory::core::{PageFlags, VirtAddr, PAGE_SIZE};
use crate::memory::virt::address_space::{UserAddressSpace, USER_MMAP_BASE};
use crate::memory::virt::vma::{VmaBacking, VmaDescriptor, VmaFlags};

// ─────────────────────────────────────────────────────────────────────────────
// Erreurs
// ─────────────────────────────────────────────────────────────────────────────

/// Erreurs possibles des opérations mmap/munmap/mprotect/brk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmapError {
    /// Longueur invalide (0 ou dépassement).
    InvalidLength,
    /// Adresse invalide ou non alignée.
    InvalidAddress,
    /// Plus d'espace virtuel disponible.
    OutOfVirtualMemory,
    /// Aucun espace d'adressage courant (getter non enregistré ou null).
    NoAddressSpace,
    /// Échec d'allocation mémoire.
    AllocFailed,
    /// Région non mappée.
    NotMapped,
    /// Permission refusée.
    PermissionDenied,
}

impl MmapError {
    /// Traduit l'erreur en errno POSIX négatif.
    pub fn to_kernel_errno(self) -> i64 {
        match self {
            MmapError::InvalidLength => -22,      // EINVAL
            MmapError::InvalidAddress => -22,     // EINVAL
            MmapError::OutOfVirtualMemory => -12, // ENOMEM
            MmapError::NoAddressSpace => -12,     // ENOMEM
            MmapError::AllocFailed => -12,        // ENOMEM
            MmapError::NotMapped => -14,          // EFAULT
            MmapError::PermissionDenied => -13,   // EACCES
        }
    }
}

/// Résultat interne du chemin mremap sans copie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MremapZeroCopy {
    /// La VMA a été déplacée par déplacement des PTEs.
    Moved(usize),
    /// Le cas demandé exige le fallback compatibilité par copie.
    Unsupported,
}

// ─────────────────────────────────────────────────────────────────────────────
// Injection du getter de l'espace d'adressage courant (COUCHE 0 pattern)
// ─────────────────────────────────────────────────────────────────────────────

/// Signature de la fonction retournant le UserAddressSpace du thread courant.
pub type CurrentAsGetterFn = fn() -> *mut UserAddressSpace;

/// Pointeur de fonction enregistré par la couche supérieure (scheduler/process).
static CURRENT_AS_FN: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Enregistre la fonction qui retourne l'espace d'adressage du thread courant.
/// Doit être appelée par le scheduler lors de son initialisation.
pub fn register_current_as_getter(f: CurrentAsGetterFn) {
    CURRENT_AS_FN.store(f as *mut (), Ordering::Release);
}

/// Obtient un pointeur vers l'espace d'adressage utilisateur courant.
fn get_current_user_as() -> Option<*mut UserAddressSpace> {
    let ptr = CURRENT_AS_FN.load(Ordering::Acquire);
    if ptr.is_null() {
        return None;
    }
    // SAFETY: Enregistré via register_current_as_getter avec la bonne signature.
    let f: CurrentAsGetterFn = unsafe { core::mem::transmute(ptr) };
    let as_ptr = f();
    if as_ptr.is_null() {
        None
    } else {
        Some(as_ptr)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Constantes POSIX (prot / map flags)
// ─────────────────────────────────────────────────────────────────────────────

/// Protection en lecture (PROT_READ).
const PROT_READ: u32 = 1;
/// Protection en écriture (PROT_WRITE).
const PROT_WRITE: u32 = 2;
/// Protection en exécution (PROT_EXEC).
const PROT_EXEC: u32 = 4;

/// Mapping partagé (MAP_SHARED).
const MAP_SHARED: u32 = 0x01;
/// Mapping privé (MAP_PRIVATE).
#[allow(dead_code)]
const MAP_PRIVATE: u32 = 0x02;
/// Adresse fixée (MAP_FIXED).
const MAP_FIXED: u32 = 0x10;
/// Mapping anonyme (MAP_ANONYMOUS / MAP_ANON).
const MAP_ANON: u32 = 0x20;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers de conversion de flags
// ─────────────────────────────────────────────────────────────────────────────

/// Convertit les protections POSIX en flags de page x86.
fn prot_to_page_flags(prot: u32) -> PageFlags {
    let mut f = PageFlags::PRESENT | PageFlags::USER;
    if prot & PROT_WRITE != 0 {
        f = f | PageFlags::WRITABLE;
    }
    // NX par défaut sauf si PROT_EXEC explicite
    if prot & PROT_EXEC == 0 {
        f = f | PageFlags::NO_EXECUTE;
    }
    f
}

/// Convertit les flags POSIX (prot + map_flags) en VmaFlags.
fn prot_to_vma_flags(prot: u32, map_flags: u32) -> VmaFlags {
    let mut f = VmaFlags::NONE;
    if prot & PROT_READ != 0 {
        f = f | VmaFlags::READ;
    }
    if prot & PROT_WRITE != 0 {
        f = f | VmaFlags::WRITE;
    }
    if prot & PROT_EXEC != 0 {
        f = f | VmaFlags::EXEC;
    }
    if map_flags & MAP_SHARED != 0 {
        f = f | VmaFlags::SHARED;
    }
    if map_flags & MAP_FIXED != 0 {
        f = f | VmaFlags::FIXED;
    }
    if map_flags & MAP_ANON != 0 {
        f = f | VmaFlags::ANONYMOUS;
    }
    f
}

// ─────────────────────────────────────────────────────────────────────────────
// do_mmap
// ─────────────────────────────────────────────────────────────────────────────

/// Mappe une région mémoire dans l'espace utilisateur courant.
///
/// - `addr`  : adresse souhaitée (0 = noyau choisit).
/// - `len`   : taille en octets (arrondie au multiple de PAGE_SIZE).
/// - `prot`  : PROT_READ | PROT_WRITE | PROT_EXEC.
/// - `flags` : MAP_ANON | MAP_PRIVATE | MAP_SHARED | MAP_FIXED.
/// - `_fd`   : descripteur de fichier (ignoré — seuls les mappings anonymes sont supportés).
/// - `_off`  : offset dans le fichier (ignoré).
///
/// Retourne l'adresse virtuelle de début de la région (comme usize).
pub fn do_mmap(
    addr: u64,
    len: usize,
    prot: u32,
    flags: u32,
    _fd: i32,
    _off: u64,
) -> Result<usize, MmapError> {
    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    // SAFETY: le pointeur est valide tant qu'on traite le syscall de ce thread.
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };
    do_mmap_in_as(user_as, addr, len, prot, flags)
}

/// Variante explicite pour les serveurs Ring1 autorisés à gérer un AS cible.
pub fn do_mmap_in_as(
    user_as: &UserAddressSpace,
    addr: u64,
    len: usize,
    prot: u32,
    flags: u32,
) -> Result<usize, MmapError> {
    if len == 0 {
        return Err(MmapError::InvalidLength);
    }

    // Alignement sur PAGE_SIZE
    let len_aligned = (len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    user_as.stats.mmap_calls.fetch_add(1, Ordering::Relaxed);

    // Recherche d'un gap virtuel libre
    let fixed = flags & MAP_FIXED != 0;
    let base = if fixed {
        if addr == 0 || addr % PAGE_SIZE as u64 != 0 {
            return Err(MmapError::InvalidAddress);
        }
        VirtAddr::new(addr)
    } else {
        let hint = if addr != 0 {
            Some(VirtAddr::new(addr))
        } else {
            None
        };
        user_as
            .find_free_gap(len_aligned, hint)
            .ok_or(MmapError::OutOfVirtualMemory)?
    };

    let start = base;
    let end_raw = base
        .as_u64()
        .checked_add(len_aligned as u64)
        .ok_or(MmapError::OutOfVirtualMemory)?;
    let end = VirtAddr::new(end_raw);

    let page_flags = prot_to_page_flags(prot);
    let vma_flags = prot_to_vma_flags(prot, flags) | VmaFlags::ANONYMOUS;

    // Allouer et insérer le descripteur VMA
    let vma = Box::new(VmaDescriptor::new(
        start,
        end,
        vma_flags,
        page_flags,
        VmaBacking::Anonymous,
    ));
    let vma_ptr = Box::into_raw(vma);

    // SAFETY: vma_ptr est valide, non-null, exclusif.
    let inserted = unsafe { user_as.insert_vma(vma_ptr) };
    if !inserted {
        // Libérer si l'insertion échoue (ex: chevauchement avec MAP_FIXED)
        // SAFETY: nous sommes les seuls propriétaires.
        let _ = unsafe { Box::from_raw(vma_ptr) };
        return Err(MmapError::InvalidAddress);
    }

    Ok(start.as_u64() as usize)
}

// ─────────────────────────────────────────────────────────────────────────────
// do_munmap
// ─────────────────────────────────────────────────────────────────────────────

/// Dé-mappe la région virtuelle débutant à `addr`.
///
/// `addr` doit être aligné sur PAGE_SIZE.
/// Démappe toutes les PTEs présentes dans la VMA et libère leurs frames physiques.
///
/// V-04 : TLB shootdown synchrone (tous CPUs) AVANT free_page.
pub fn do_munmap(addr: u64, _len: usize) -> Result<(), MmapError> {
    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    // SAFETY: user_as_ptr est non-null (garanti par get_current_user_as) et vit le temps du syscall.
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };
    do_munmap_in_as(user_as, addr)
}

/// Démappe une VMA dans un espace d'adressage explicite.
pub fn do_munmap_in_as(user_as: &UserAddressSpace, addr: u64) -> Result<(), MmapError> {
    if addr == 0 || addr % PAGE_SIZE as u64 != 0 {
        return Err(MmapError::InvalidAddress);
    }

    user_as.stats.munmap_calls.fetch_add(1, Ordering::Relaxed);

    match user_as.remove_vma(VirtAddr::new(addr)) {
        None => Err(MmapError::NotMapped),
        Some(vma_ptr) => {
            // Récupérer les bornes de la VMA avant de libérer le descripteur.
            let (vma_start, vma_end) = unsafe {
                let vma = &*vma_ptr;
                (vma.start, vma.end)
            };

            // Libérer le VmaDescriptor alloué par do_mmap / do_brk.
            // SAFETY: le pointeur est valide et nous sommes les propriétaires.
            let _ = unsafe { Box::from_raw(vma_ptr) };

            // V-04 : traiter les pages par lots pour limiter la taille du buffer.
            //   1. Démappe les PTEs (flush TLB local intégré via unmap_page).
            //   2. TLB shootdown synchrone vers tous les CPUs actifs.
            //   3. Libère les frames physiques accumulés.
            // Cette séparation garantit qu'aucun CPUs ne peut accéder à un frame
            // après sa libération via un entrée TLB périmée (V-04).
            const BATCH: usize = 64; // 64 × 4 KiB = 256 KiB par lot
            let cpu_count = crate::arch::x86_64::acpi::madt::madt_cpu_count();

            let mut cursor = vma_start.as_u64();
            while cursor < vma_end.as_u64() {
                let mut frames = [crate::memory::core::types::Frame::containing(
                    crate::memory::core::types::PhysAddr::new(0),
                ); BATCH];
                let mut count = 0usize;

                // Phase 1 : démappe jusqu'à BATCH pages (flush local intégré).
                while cursor < vma_end.as_u64() && count < BATCH {
                    // SAFETY: adresses user canoniques.
                    if let Some(f) = unsafe { user_as.unmap_page(VirtAddr::new(cursor)) } {
                        frames[count] = f;
                        count += 1;
                    }
                    cursor += PAGE_SIZE as u64;
                }

                if count == 0 {
                    continue;
                }

                // Phase 2 : TLB shootdown synchrone (tous CPUs) — V-04.
                let batch_start = VirtAddr::new(
                    vma_start
                        .as_u64()
                        .max(cursor - (count as u64 * PAGE_SIZE as u64)),
                );
                let batch_end = VirtAddr::new(cursor);
                // SAFETY: plage canonique user, appelé hors IRQ.
                unsafe {
                    crate::memory::virt::shootdown_sync(
                        crate::memory::virt::TlbFlushType::Range {
                            start: batch_start,
                            end: batch_end,
                        },
                        cpu_count,
                    );
                }

                // Phase 3 : libérer les frames (TLBs déjà invalidés partout).
                for i in 0..count {
                    let _ = crate::memory::physical::allocator::buddy::free_page(frames[i]);
                }
            }
            Ok(())
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// do_mprotect
// ─────────────────────────────────────────────────────────────────────────────

/// Modifie les permissions d'une région mémoire existante.
///
/// `addr` doit être aligné sur PAGE_SIZE et correspondre au début d'une VMA.
/// Met à jour les PTEs déjà présentes en table de pages ET invalide le TLB.
pub fn do_mprotect(addr: u64, len: usize, prot: u32) -> Result<(), MmapError> {
    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    // SAFETY: user_as_ptr est non-null (garanti par get_current_user_as) et vit le temps du syscall.
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };
    do_mprotect_in_as(user_as, addr, len, prot)
}

/// Modifie les permissions d'une région dans un espace d'adressage explicite.
pub fn do_mprotect_in_as(
    user_as: &UserAddressSpace,
    addr: u64,
    len: usize,
    prot: u32,
) -> Result<(), MmapError> {
    if addr == 0 || addr % PAGE_SIZE as u64 != 0 {
        return Err(MmapError::InvalidAddress);
    }
    if len == 0 {
        return Err(MmapError::InvalidLength);
    }

    let vma_const_ptr = user_as
        .find_vma(VirtAddr::new(addr))
        .ok_or(MmapError::NotMapped)?;

    // SAFETY: La VMA est valide et protégée par le verrou de l'address space.
    let vma: &mut VmaDescriptor = unsafe { &mut *(vma_const_ptr as *mut VmaDescriptor) };

    let len_aligned = (len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let range_start = VirtAddr::new(addr);
    let range_end_raw = addr
        .checked_add(len_aligned as u64)
        .ok_or(MmapError::InvalidLength)?;
    let range_end = VirtAddr::new(range_end_raw);
    if range_end.as_u64() > vma.end.as_u64() {
        return Err(MmapError::NotMapped);
    }

    let new_page_flags = prot_to_page_flags(prot);

    // Mettre à jour les flags de page dans la VMA (pour les futurs demand-faults).
    vma.page_flags = new_page_flags;

    // Mettre à jour les flags VMA (READ/WRITE/EXEC uniquement).
    let perm_mask = VmaFlags::READ | VmaFlags::WRITE | VmaFlags::EXEC;
    let keep = VmaFlags::from_bits(vma.flags.bits() & !perm_mask.bits());
    let mut new_perm = VmaFlags::NONE;
    if prot & PROT_READ != 0 {
        new_perm = new_perm | VmaFlags::READ;
    }
    if prot & PROT_WRITE != 0 {
        new_perm = new_perm | VmaFlags::WRITE;
    }
    if prot & PROT_EXEC != 0 {
        new_perm = new_perm | VmaFlags::EXEC;
    }
    vma.flags = keep | new_perm;

    // Appliquer les nouveaux flags sur les PTEs déjà présentes dans la table.
    // Les pages non encore faultées (demand paging) reçoivent une Err ignorée :
    // elles hériteront des nouveaux flags lors de leur prochain fault via vma.page_flags.
    let mut walker = crate::memory::virt::page_table::PageTableWalker::new(user_as.pml4_phys());
    let mut cursor = range_start.as_u64();
    while cursor < range_end.as_u64() {
        // remap_flags → Err si page absente (demand paging) → ignoré intentionnellement.
        let _ = walker.remap_flags(VirtAddr::new(cursor), new_page_flags);
        cursor += PAGE_SIZE as u64;
    }

    // Invalider le TLB pour toute la plage modifiée, y compris si l'AS cible
    // tourne sur un autre CPU.
    let cpu_count = crate::arch::x86_64::acpi::madt::madt_cpu_count();
    // SAFETY: adresses user canoniques dans [range_start, range_end), hors IRQ.
    unsafe {
        crate::memory::virt::shootdown_sync(
            crate::memory::virt::TlbFlushType::Range {
                start: range_start,
                end: range_end,
            },
            cpu_count,
        );
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// do_mremap_zero_copy
// ─────────────────────────────────────────────────────────────────────────────

/// Déplace une VMA complète par déplacement de PTEs, sans recopier les données.
///
/// Ce chemin couvre le cas critique des allocateurs : une région anonyme privée
/// entière est agrandie/déplacée avec `MREMAP_MAYMOVE`. Les cas partiels ou
/// `MREMAP_DONTUNMAP` restent traités par le fallback de compatibilité côté
/// syscall, car ils nécessitent une sémantique de splitting/aliasing séparée.
pub fn do_mremap_zero_copy(
    old_addr: u64,
    old_len: usize,
    new_len: usize,
    flags: u64,
    new_addr: u64,
) -> Result<MremapZeroCopy, MmapError> {
    const MREMAP_MAYMOVE: u64 = 0x1;
    const MREMAP_FIXED: u64 = 0x2;
    const MREMAP_DONTUNMAP: u64 = 0x4;

    if old_addr == 0 || old_addr % PAGE_SIZE as u64 != 0 {
        return Err(MmapError::InvalidAddress);
    }
    if old_len == 0 || new_len == 0 {
        return Err(MmapError::InvalidLength);
    }
    if flags & MREMAP_DONTUNMAP != 0 {
        return Ok(MremapZeroCopy::Unsupported);
    }
    if flags & MREMAP_MAYMOVE == 0 {
        return Ok(MremapZeroCopy::Unsupported);
    }
    if flags & MREMAP_FIXED != 0 && (new_addr == 0 || new_addr % PAGE_SIZE as u64 != 0) {
        return Err(MmapError::InvalidAddress);
    }

    let old_len_aligned = (old_len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let new_len_aligned = (new_len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    if new_len_aligned < old_len_aligned {
        return Ok(MremapZeroCopy::Unsupported);
    }
    let old_start = VirtAddr::new(old_addr);

    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    // SAFETY: le pointeur est valide tant qu'on traite le syscall de ce thread.
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };

    let old_vma_ptr = user_as.find_vma(old_start).ok_or(MmapError::NotMapped)?;
    // SAFETY: pointeur de VMA issu de l'AS courant; lecture instantanée des métadonnées.
    let old_vma = unsafe { &*old_vma_ptr };
    if old_vma.start.as_u64() != old_addr || old_vma.size() != old_len_aligned {
        return Ok(MremapZeroCopy::Unsupported);
    }
    if new_len_aligned > old_len_aligned && old_vma.flags.contains(VmaFlags::DONTEXPAND) {
        return Err(MmapError::PermissionDenied);
    }

    let target_start = if flags & MREMAP_FIXED != 0 {
        VirtAddr::new(new_addr)
    } else {
        user_as
            .find_free_gap(new_len_aligned, None)
            .ok_or(MmapError::OutOfVirtualMemory)?
    };
    let target_end_raw = target_start
        .as_u64()
        .checked_add(new_len_aligned as u64)
        .ok_or(MmapError::OutOfVirtualMemory)?;
    let target_end = VirtAddr::new(target_end_raw);

    let mut moved_vma = old_vma.clone_metadata();
    moved_vma.start = target_start;
    moved_vma.end = target_end;

    let new_vma = Box::new(moved_vma);
    let new_vma_ptr = Box::into_raw(new_vma);
    // SAFETY: new_vma_ptr est une allocation fraîche non encore attachée.
    if !unsafe { user_as.insert_vma(new_vma_ptr) } {
        // SAFETY: insertion échouée, donc l'arbre n'a pas pris la propriété.
        let _ = unsafe { Box::from_raw(new_vma_ptr) };
        return Err(MmapError::InvalidAddress);
    }

    struct PtAllocOnly;
    impl crate::memory::virt::page_table::FrameAllocatorForWalk for PtAllocOnly {
        fn alloc_frame(
            &self,
            flags: crate::memory::AllocFlags,
        ) -> Result<crate::memory::Frame, crate::memory::AllocError> {
            crate::memory::physical::allocator::buddy::alloc_pages(0, flags)
        }

        fn free_frame(&self, frame: crate::memory::Frame) {
            let _ = crate::memory::physical::allocator::buddy::free_pages(frame, 0);
        }
    }

    let pt_alloc = PtAllocOnly;
    let mut walker = crate::memory::virt::page_table::PageTableWalker::new(user_as.pml4_phys());
    let page_count = old_len_aligned / PAGE_SIZE;
    let mut moved_pages = 0usize;

    for page_idx in 0..page_count {
        let src = VirtAddr::new(old_start.as_u64() + (page_idx * PAGE_SIZE) as u64);
        let dst = VirtAddr::new(target_start.as_u64() + (page_idx * PAGE_SIZE) as u64);
        match walker.move_leaf(src, dst, &pt_alloc) {
            Ok(_) => moved_pages += 1,
            Err(_) => {
                while moved_pages > 0 {
                    moved_pages -= 1;
                    let rollback_src =
                        VirtAddr::new(target_start.as_u64() + (moved_pages * PAGE_SIZE) as u64);
                    let rollback_dst =
                        VirtAddr::new(old_start.as_u64() + (moved_pages * PAGE_SIZE) as u64);
                    let _ = walker.move_leaf(rollback_src, rollback_dst, &pt_alloc);
                }
                free_removed_vma(user_as, target_start);
                return Err(MmapError::AllocFailed);
            }
        }
    }

    free_removed_vma(user_as, old_start);

    let cpu_count = crate::arch::x86_64::acpi::madt::madt_cpu_count();
    let old_end_raw = old_start
        .as_u64()
        .checked_add(old_len_aligned as u64)
        .ok_or(MmapError::OutOfVirtualMemory)?;
    let old_end = VirtAddr::new(old_end_raw);
    // SAFETY: plages user canoniques, hors IRQ.
    unsafe {
        crate::memory::virt::shootdown_sync(
            crate::memory::virt::TlbFlushType::Range {
                start: old_start,
                end: old_end,
            },
            cpu_count,
        );
        crate::memory::virt::shootdown_sync(
            crate::memory::virt::TlbFlushType::Range {
                start: target_start,
                end: target_end,
            },
            cpu_count,
        );
    }

    Ok(MremapZeroCopy::Moved(target_start.as_u64() as usize))
}

// ─────────────────────────────────────────────────────────────────────────────
// do_brk
// ─────────────────────────────────────────────────────────────────────────────

/// Fallback historique pour les espaces sans image ELF publiée. Les processus
/// normaux utilisent `UserAddressSpace::heap_start`, initialisé par le loader.
const BRK_BASE: u64 = 0x0000_0001_0000_0000; // 4 GiB

fn align_up_page(addr: u64) -> Option<u64> {
    let mask = PAGE_SIZE as u64 - 1;
    addr.checked_add(mask).map(|value| value & !mask)
}

/// Étend ou réduit le heap utilisateur (per-process via `UserAddressSpace`).
///
/// - `addr == 0` : retourne le break courant.
/// - `addr != 0` : déplace le break vers `addr` (arrondi au prochain multiple
///   de PAGE_SIZE) et crée uniquement la portion de VMA réellement nouvelle.
///
/// Retourne la nouvelle valeur du break.
pub fn do_brk(addr: u64) -> Result<u64, MmapError> {
    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    // SAFETY: user_as_ptr est non-null (garanti par get_current_user_as) et vit le temps du syscall.
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };

    let heap_base = match user_as.heap_start.load(Ordering::Acquire) {
        0 => BRK_BASE,
        base => base,
    };
    let cur = match user_as.heap_end.load(Ordering::Acquire) {
        0 => heap_base,
        current => current,
    };

    if addr == 0 {
        return Ok(cur);
    }

    if addr < heap_base || addr > USER_MMAP_BASE {
        return Err(MmapError::InvalidAddress);
    }

    let new_brk = align_up_page(addr).ok_or(MmapError::InvalidAddress)?;
    if new_brk < heap_base || new_brk > USER_MMAP_BASE {
        return Err(MmapError::InvalidAddress);
    }

    if user_as.heap_start.load(Ordering::Acquire) == 0 {
        user_as.init_heap_bounds(heap_base);
    }

    if new_brk > cur {
        let start = user_as
            .heap_covered_end_from(VirtAddr::new(cur), VirtAddr::new(new_brk))
            .ok_or(MmapError::PermissionDenied)?;
        if start.as_u64() >= new_brk {
            user_as.heap_end.store(new_brk, Ordering::Release);
            return Ok(new_brk);
        }

        let end = VirtAddr::new(new_brk);
        let page_flags =
            PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::USER | PageFlags::NO_EXECUTE;
        let vma_flags = VmaFlags::READ | VmaFlags::WRITE | VmaFlags::ANONYMOUS | VmaFlags::HEAP;

        let vma = Box::new(VmaDescriptor::new(
            start,
            end,
            vma_flags,
            page_flags,
            VmaBacking::Anonymous,
        ));
        let vma_ptr = Box::into_raw(vma);

        // SAFETY: vma_ptr valide, non-null, exclusif — issu de Box::into_raw.
        let inserted = unsafe { user_as.insert_vma(vma_ptr) };
        if !inserted {
            // SAFETY: vma_ptr est encore exclusif — insert_vma a échoué, on reprend la propriété.
            let _ = unsafe { Box::from_raw(vma_ptr) };
            return Err(MmapError::OutOfVirtualMemory);
        }
    }
    // Réduction : on ne retire pas les VMAs (comportement Linux sur shrink).

    // Mettre à jour le break per-process (pas de state global).
    user_as.heap_end.store(new_brk, Ordering::Release);
    Ok(new_brk)
}

// ─────────────────────────────────────────────────────────────────────────────
// map_shm_into_process  —  P1-02
// ─────────────────────────────────────────────────────────────────────────────

/// Erreurs spécifiques au mappage SHM dans un espace d'adressage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShmMapError {
    /// Aucun espace d'adressage cible disponible.
    NoAddressSpace,
    /// Plus d'espace virtuel disponible dans l'espace cible.
    OutOfVirtualMemory,
    /// Échec de l'allocation ou mappage d'une frame.
    AllocFailed,
    /// Région SHM invalide ou inactive.
    InvalidRegion,
    /// Permission refusée (intersection vide).
    PermissionDenied,
}

/// Métadonnées minimales d'une région SHM fournies par la couche IPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShmRegionInfo {
    pub n_pages: usize,
    pub size_bytes: usize,
}

pub type ShmRegionInfoFn =
    fn(desc_idx: usize, writable: bool) -> Result<ShmRegionInfo, ShmMapError>;
pub type ShmPagePhysFn = fn(desc_idx: usize, page_idx: usize) -> Option<u64>;
pub type ShmReleaseRegionFn = fn(desc_idx: usize);
pub type ShmRegisterMappingFn = fn(
    desc_idx: usize,
    pid: u32,
    virt_base: u64,
    writable: bool,
    n_pages: usize,
) -> Result<usize, ShmMapError>;

/// Callbacks IPC nécessaires pour mapper une région SHM sans importer IPC dans memory/.
#[derive(Clone, Copy)]
pub struct ShmProviderFns {
    pub region_info: ShmRegionInfoFn,
    pub page_phys: ShmPagePhysFn,
    pub release_region: ShmReleaseRegionFn,
    pub register_mapping: ShmRegisterMappingFn,
}

static SHM_REGION_INFO_FN: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static SHM_PAGE_PHYS_FN: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static SHM_RELEASE_REGION_FN: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static SHM_REGISTER_MAPPING_FN: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

pub fn register_shm_provider(provider: ShmProviderFns) {
    SHM_REGION_INFO_FN.store(provider.region_info as *mut (), Ordering::Release);
    SHM_PAGE_PHYS_FN.store(provider.page_phys as *mut (), Ordering::Release);
    SHM_RELEASE_REGION_FN.store(provider.release_region as *mut (), Ordering::Release);
    SHM_REGISTER_MAPPING_FN.store(provider.register_mapping as *mut (), Ordering::Release);
}

fn shm_provider() -> Option<ShmProviderFns> {
    let region_info = SHM_REGION_INFO_FN.load(Ordering::Acquire);
    let page_phys = SHM_PAGE_PHYS_FN.load(Ordering::Acquire);
    let release_region = SHM_RELEASE_REGION_FN.load(Ordering::Acquire);
    let register_mapping = SHM_REGISTER_MAPPING_FN.load(Ordering::Acquire);
    if region_info.is_null()
        || page_phys.is_null()
        || release_region.is_null()
        || register_mapping.is_null()
    {
        return None;
    }
    Some(ShmProviderFns {
        // SAFETY: enregistré exclusivement via register_shm_provider avec les signatures attendues.
        region_info: unsafe { core::mem::transmute(region_info) },
        page_phys: unsafe { core::mem::transmute(page_phys) },
        release_region: unsafe { core::mem::transmute(release_region) },
        register_mapping: unsafe { core::mem::transmute(register_mapping) },
    })
}

/// Résultat d'un mappage SHM dans un espace d'adressage.
pub struct ShmMapIntoResult {
    /// Adresse virtuelle de base du mapping dans l'espace cible.
    pub virt_base: u64,
    /// Nombre de pages effectivement mappées.
    pub n_pages: usize,
    /// Index du mapping dans SHM_MAPPING_TABLE (pour shm_unmap).
    pub mapping_idx: usize,
}

fn free_removed_vma(user_as: &UserAddressSpace, start: VirtAddr) {
    if let Some(vma_ptr) = user_as.remove_vma(start) {
        // SAFETY: remove_vma() transfère la propriété du descripteur à l'appelant.
        let _ = unsafe { Box::from_raw(vma_ptr) };
    }
}

fn rollback_shm_into_process(user_as: &UserAddressSpace, virt_base: VirtAddr, n_pages: usize) {
    for i in 0..n_pages {
        let v = VirtAddr::new(virt_base.as_u64().saturating_add((i * PAGE_SIZE) as u64));
        unsafe {
            user_as.unmap_page(v);
        }
    }
    free_removed_vma(user_as, virt_base);
}

/// Mappe une région SHM existante dans l'espace d'adressage d'un processus.
///
/// ## Opérations
/// 1. Trouve un gap virtuel libre dans `user_as` (ou utilise `hint_virt`)
/// 2. Enregistre une VMA de type `VmaBacking::Shared`
/// 3. Mappe chaque frame physique SHM via `user_as.map_page()` (NO_COW)
/// 4. Enregistre le mapping via le provider SHM injecté par la couche IPC
///
/// ## Safety
/// `user_as` doit être un pointeur valide vers un `UserAddressSpace` actif.
/// Les frames SHM sont fournies par le provider enregistré et ne sont jamais
/// libérées tant qu'au moins un mapping reste actif.
///
/// ## Arguments
/// - `user_as`   : espace d'adressage du processus cible
/// - `desc_idx`  : index opaque de la région SHM côté provider
/// - `pid`       : PID du processus (pour les hooks VMM)
/// - `hint_virt` : adresse virtuelle souhaitée (0 = auto)
/// - `writable`  : autoriser l'écriture dans le mapping
pub fn map_shm_into_process(
    user_as: &UserAddressSpace,
    desc_idx: usize,
    pid: u32,
    hint_virt: u64,
    writable: bool,
) -> Result<ShmMapIntoResult, ShmMapError> {
    use crate::memory::core::{Frame, PageFlags};
    use crate::memory::physical::allocator::buddy;

    let provider = shm_provider().ok_or(ShmMapError::InvalidRegion)?;

    // ── 1. Lire les métadonnées de la région SHM ──────────────────────────
    let info = (provider.region_info)(desc_idx, writable)?;
    let n_pages = info.n_pages;
    let size_bytes = info.size_bytes;

    if n_pages == 0 {
        (provider.release_region)(desc_idx);
        return Err(ShmMapError::InvalidRegion);
    }

    let size_aligned = (size_bytes + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // ── 2. Trouver un gap virtuel libre ───────────────────────────────────
    let hint = if hint_virt != 0 {
        Some(VirtAddr::new(hint_virt))
    } else {
        None
    };
    let virt_base = user_as.find_free_gap(size_aligned, hint).ok_or_else(|| {
        (provider.release_region)(desc_idx);
        ShmMapError::OutOfVirtualMemory
    })?;

    let virt_end = VirtAddr::new(virt_base.as_u64() + size_aligned as u64);

    // ── 3. Construire les flags de la VMA ────────────────────────────────
    let vma_flags = if writable {
        VmaFlags::READ | VmaFlags::WRITE | VmaFlags::SHARED
    } else {
        VmaFlags::READ | VmaFlags::SHARED
    };
    // NO_COW sur les pages SHM (partagées, pas de copy-on-write)
    let page_flags = if writable {
        PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::USER | PageFlags::NO_EXECUTE
    } else {
        PageFlags::PRESENT | PageFlags::USER | PageFlags::NO_EXECUTE
    };

    // ── 4. Insérer la VMA ─────────────────────────────────────────────────
    let vma = Box::new(VmaDescriptor::new(
        virt_base,
        virt_end,
        vma_flags,
        page_flags,
        VmaBacking::Shared,
    ));
    let vma_ptr = Box::into_raw(vma);
    // SAFETY: vma_ptr est valide, non-null, exclusif.
    let inserted = unsafe { user_as.insert_vma(vma_ptr) };
    if !inserted {
        // SAFETY: on récupère la propriété que l'on avait.
        let _ = unsafe { Box::from_raw(vma_ptr) };
        (provider.release_region)(desc_idx);
        return Err(ShmMapError::OutOfVirtualMemory);
    }

    // ── 5. Mapper les frames physiques SHM ──────────────────────────────
    // Allocateur de tables intermédiaires (PT/PD/PDPT) uniquement —
    // les frames de données viennent du pool SHM.
    struct PtAllocOnly;
    impl crate::memory::virt::page_table::FrameAllocatorForWalk for PtAllocOnly {
        fn alloc_frame(
            &self,
            flags: crate::memory::AllocFlags,
        ) -> Result<Frame, crate::memory::AllocError> {
            buddy::alloc_pages(0, flags)
        }
        fn free_frame(&self, f: Frame) {
            let _ = buddy::free_pages(f, 0);
        }
    }
    let pt_alloc = PtAllocOnly;

    {
        for i in 0..n_pages {
            let Some(phys) = (provider.page_phys)(desc_idx, i) else {
                rollback_shm_into_process(user_as, virt_base, i);
                (provider.release_region)(desc_idx);
                return Err(ShmMapError::InvalidRegion);
            };
            let frame = Frame::containing(crate::memory::core::PhysAddr::new(phys));
            let virt = VirtAddr::new(virt_base.as_u64().saturating_add((i * PAGE_SIZE) as u64));
            // SAFETY: virt est dans l'espace user du processus cible,
            // frame est une page SHM allouée et initialisée.
            if let Err(_) = unsafe { user_as.map_page(virt, frame, page_flags, &pt_alloc) } {
                rollback_shm_into_process(user_as, virt_base, i);
                (provider.release_region)(desc_idx);
                return Err(ShmMapError::AllocFailed);
            }
        }
    }

    // ── 6. Enregistrer dans SHM_MAPPING_TABLE ────────────────────────────
    let mapping_idx =
        (provider.register_mapping)(desc_idx, pid, virt_base.as_u64(), writable, n_pages).map_err(
            |err| {
                rollback_shm_into_process(user_as, virt_base, n_pages);
                (provider.release_region)(desc_idx);
                err
            },
        )?;

    Ok(ShmMapIntoResult {
        virt_base: virt_base.as_u64(),
        n_pages,
        mapping_idx,
    })
}
