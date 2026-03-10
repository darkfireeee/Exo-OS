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

use crate::memory::core::{VirtAddr, PageFlags, PAGE_SIZE};
use crate::memory::virt::address_space::{UserAddressSpace, USER_MMAP_BASE};
use crate::memory::virt::vma::{VmaDescriptor, VmaFlags, VmaBacking};

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
            MmapError::InvalidLength      => -22, // EINVAL
            MmapError::InvalidAddress     => -22, // EINVAL
            MmapError::OutOfVirtualMemory => -12, // ENOMEM
            MmapError::NoAddressSpace     => -12, // ENOMEM
            MmapError::AllocFailed        => -12, // ENOMEM
            MmapError::NotMapped          => -14, // EFAULT
            MmapError::PermissionDenied   => -13, // EACCES
        }
    }
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
    if as_ptr.is_null() { None } else { Some(as_ptr) }
}

// ─────────────────────────────────────────────────────────────────────────────
// Constantes POSIX (prot / map flags)
// ─────────────────────────────────────────────────────────────────────────────

/// Protection en lecture (PROT_READ).
const PROT_READ:   u32 = 1;
/// Protection en écriture (PROT_WRITE).
const PROT_WRITE:  u32 = 2;
/// Protection en exécution (PROT_EXEC).
const PROT_EXEC:   u32 = 4;

/// Mapping partagé (MAP_SHARED).
const MAP_SHARED:  u32 = 0x01;
/// Mapping privé (MAP_PRIVATE).
#[allow(dead_code)]
const MAP_PRIVATE: u32 = 0x02;
/// Adresse fixée (MAP_FIXED).
const MAP_FIXED:   u32 = 0x10;
/// Mapping anonyme (MAP_ANONYMOUS / MAP_ANON).
const MAP_ANON:    u32 = 0x20;

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
    if prot & PROT_READ  != 0 { f = f | VmaFlags::READ; }
    if prot & PROT_WRITE != 0 { f = f | VmaFlags::WRITE; }
    if prot & PROT_EXEC  != 0 { f = f | VmaFlags::EXEC; }
    if map_flags & MAP_SHARED != 0 { f = f | VmaFlags::SHARED; }
    if map_flags & MAP_FIXED  != 0 { f = f | VmaFlags::FIXED; }
    if map_flags & MAP_ANON   != 0 { f = f | VmaFlags::ANONYMOUS; }
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
    addr:  u64,
    len:   usize,
    prot:  u32,
    flags: u32,
    _fd:   i32,
    _off:  u64,
) -> Result<usize, MmapError> {
    if len == 0 {
        return Err(MmapError::InvalidLength);
    }

    // Alignement sur PAGE_SIZE
    let len_aligned = (len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    // SAFETY: le pointeur est valide tant qu'on traite le syscall de ce thread.
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };

    user_as.stats.mmap_calls.fetch_add(1, Ordering::Relaxed);

    // Recherche d'un gap virtuel libre
    let hint = if addr != 0 { Some(VirtAddr::new(addr)) } else { None };
    let base = user_as
        .find_free_gap(len_aligned, hint)
        .ok_or(MmapError::OutOfVirtualMemory)?;

    let start = base;
    let end   = VirtAddr::new(base.as_u64() + len_aligned as u64);

    let page_flags = prot_to_page_flags(prot);
    let vma_flags  = prot_to_vma_flags(prot, flags) | VmaFlags::ANONYMOUS;

    // Allouer et insérer le descripteur VMA
    let vma     = Box::new(VmaDescriptor::new(start, end, vma_flags, page_flags, VmaBacking::Anonymous));
    let vma_ptr = Box::into_raw(vma);

    // SAFETY: vma_ptr est valide, non-null, exclusif.
    let inserted = unsafe { user_as.insert_vma(vma_ptr) };
    if !inserted {
        // Libérer si l'insertion échoue (ex: chevauchement avec MAP_FIXED)
        // SAFETY: nous sommes les seuls propriétaires.
        let _ = unsafe { Box::from_raw(vma_ptr) };
        return Err(MmapError::InvalidAddress);
    }

    user_as.stats.vma_count.fetch_add(1, Ordering::Relaxed);
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
    if addr == 0 || addr % PAGE_SIZE as u64 != 0 {
        return Err(MmapError::InvalidAddress);
    }

    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    // SAFETY: user_as_ptr est non-null (garanti par get_current_user_as) et vit le temps du syscall.
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };

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
            user_as.stats.vma_count.fetch_sub(1, Ordering::Relaxed);

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
                    crate::memory::core::types::PhysAddr::new(0)); BATCH];
                let mut count = 0usize;

                // Phase 1 : démappe jusqu'à BATCH pages (flush local intégré).
                while cursor < vma_end.as_u64() && count < BATCH {
                    // SAFETY: adresses user canoniques.
                    if let Some(f) = unsafe {
                        user_as.unmap_page(VirtAddr::new(cursor))
                    } {
                        frames[count] = f;
                        count += 1;
                    }
                    cursor += PAGE_SIZE as u64;
                }

                if count == 0 { continue; }

                // Phase 2 : TLB shootdown synchrone (tous CPUs) — V-04.
                let batch_start = VirtAddr::new(
                    vma_start.as_u64().max(cursor - (count as u64 * PAGE_SIZE as u64))
                );
                let batch_end = VirtAddr::new(cursor);
                // SAFETY: plage canonique user, appelé hors IRQ.
                unsafe {
                    crate::memory::virt::shootdown_sync(
                        crate::memory::virt::TlbFlushType::Range {
                            start: batch_start,
                            end:   batch_end,
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
    if addr == 0 || addr % PAGE_SIZE as u64 != 0 {
        return Err(MmapError::InvalidAddress);
    }
    if len == 0 {
        return Err(MmapError::InvalidLength);
    }

    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    // SAFETY: user_as_ptr est non-null (garanti par get_current_user_as) et vit le temps du syscall.
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };

    let vma_const_ptr = user_as
        .find_vma(VirtAddr::new(addr))
        .ok_or(MmapError::NotMapped)?;

    // SAFETY: La VMA est valide et protégée par le verrou de l'address space.
    let vma: &mut VmaDescriptor = unsafe { &mut *(vma_const_ptr as *mut VmaDescriptor) };

    let new_page_flags = prot_to_page_flags(prot);

    // Mettre à jour les flags de page dans la VMA (pour les futurs demand-faults).
    vma.page_flags = new_page_flags;

    // Mettre à jour les flags VMA (READ/WRITE/EXEC uniquement).
    let perm_mask = VmaFlags::READ | VmaFlags::WRITE | VmaFlags::EXEC;
    let keep      = VmaFlags::from_bits(vma.flags.bits() & !perm_mask.bits());
    let mut new_perm = VmaFlags::NONE;
    if prot & PROT_READ  != 0 { new_perm = new_perm | VmaFlags::READ; }
    if prot & PROT_WRITE != 0 { new_perm = new_perm | VmaFlags::WRITE; }
    if prot & PROT_EXEC  != 0 { new_perm = new_perm | VmaFlags::EXEC; }
    vma.flags = keep | new_perm;

    // Appliquer les nouveaux flags sur les PTEs déjà présentes dans la table.
    // Les pages non encore faultées (demand paging) reçoivent une Err ignorée :
    // elles hériteront des nouveaux flags lors de leur prochain fault via vma.page_flags.
    let len_aligned = (len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let range_start = VirtAddr::new(addr);
    let range_end   = VirtAddr::new(addr + len_aligned as u64);

    let mut walker = crate::memory::virt::page_table::PageTableWalker::new(user_as.pml4_phys());
    let mut cursor = range_start.as_u64();
    while cursor < range_end.as_u64() {
        // remap_flags → Err si page absente (demand paging) → ignoré intentionnellement.
        let _ = walker.remap_flags(VirtAddr::new(cursor), new_page_flags);
        cursor += PAGE_SIZE as u64;
    }

    // Invalider le TLB pour toute la plage modifiée.
    // SAFETY: adresses user canoniques dans [range_start, range_end).
    unsafe {
        crate::memory::virt::address_space::tlb::flush_range(range_start, range_end);
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// do_brk
// ─────────────────────────────────────────────────────────────────────────────

/// Adresse de base du heap utilisateur — début de la zone [4 GiB … USER_MMAP_BASE).
/// Stockée ici uniquement comme constante de référence ; l'état réel est dans
/// `UserAddressSpace::heap_end` (per-process, sans state global).
const BRK_BASE: u64 = 0x0000_0001_0000_0000; // 4 GiB

/// Étend ou réduit le heap utilisateur (per-process via `UserAddressSpace::heap_end`).
///
/// - `addr == 0` : retourne le break courant (ou `BRK_BASE` si jamais initialisé).
/// - `addr != 0` : déplace le break vers `addr` (arrondi au prochain multiple
///   de PAGE_SIZE) et crée la VMA correspondante si le heap s'étend.
///
/// Retourne la nouvelle valeur du break.
pub fn do_brk(addr: u64) -> Result<u64, MmapError> {
    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    // SAFETY: user_as_ptr est non-null (garanti par get_current_user_as) et vit le temps du syscall.
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };

    // Initialiser le break au démarrage (première fois : heap_end == 0).
    let cur = {
        let v = user_as.heap_end.load(Ordering::Acquire);
        if v == 0 { BRK_BASE } else { v }
    };

    if addr == 0 {
        return Ok(cur);
    }

    // Vérifier que l'adresse est dans une plage valide
    if addr < BRK_BASE || addr > USER_MMAP_BASE {
        return Err(MmapError::InvalidAddress);
    }

    // Aligner sur PAGE_SIZE
    let new_brk = (addr + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);

    if new_brk > cur {
        // Extension du heap : créer une VMA pour la nouvelle région
        let start      = VirtAddr::new(cur);
        let end        = VirtAddr::new(new_brk);
        let page_flags = PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::USER | PageFlags::NO_EXECUTE;
        let vma_flags  = VmaFlags::READ | VmaFlags::WRITE | VmaFlags::ANONYMOUS | VmaFlags::HEAP;

        let vma     = Box::new(VmaDescriptor::new(start, end, vma_flags, page_flags, VmaBacking::Anonymous));
        let vma_ptr = Box::into_raw(vma);

        // SAFETY: vma_ptr valide, non-null, exclusif — issu de Box::into_raw.
        let inserted = unsafe { user_as.insert_vma(vma_ptr) };
        if !inserted {
            // SAFETY: vma_ptr est encore exclusif — insert_vma a échoué, on reprend la propriété.
            let _ = unsafe { Box::from_raw(vma_ptr) };
            return Err(MmapError::OutOfVirtualMemory);
        }

        user_as.stats.vma_count.fetch_add(1, Ordering::Relaxed);
    }
    // Réduction : on ne retire pas les VMAs (comportement Linux sur shrink).

    // Mettre à jour le break per-process (pas de state global).
    user_as.heap_end.store(new_brk, Ordering::Release);
    Ok(new_brk)
}
