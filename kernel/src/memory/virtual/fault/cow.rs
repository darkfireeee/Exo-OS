// kernel/src/memory/virtual/fault/cow.rs
//
// CoW fault handler — gère un write sur une page marquée Copy-on-Write.
// Couche 0 — aucune dépendance externe sauf `spin`.

use super::handler::FaultAllocator;
use super::{FaultContext, FaultResult};
use crate::memory::core::{PageFlags, VirtAddr, PAGE_SIZE};
use crate::memory::cow::tracker::COW_TRACKER;
use crate::memory::virt::address_space::tlb::flush_single;
use crate::memory::virt::page_table::PageTableEntry;
use crate::memory::virt::vma::VmaDescriptor;

/// Traite un CoW fault (write sur page en lecture seule avec flag COW).
pub fn handle_cow_fault<A: FaultAllocator>(
    ctx: &FaultContext,
    vma: &VmaDescriptor,
    alloc: &A,
) -> FaultResult {
    let page_addr = VirtAddr::new(ctx.fault_addr.as_u64() & !(PAGE_SIZE as u64 - 1));

    let old_raw = alloc.read_pte_raw(page_addr);
    let old_entry = PageTableEntry::from_raw(old_raw);
    let old_frame = match old_entry.frame() {
        Some(frame) => frame,
        None => match alloc.translate(page_addr) {
            Some(phys) => crate::memory::core::Frame::containing(phys),
            None => {
                // Page pas encore mappée + flag COW → demand paging d'abord.
                return super::demand_paging::handle_demand_paging(ctx, vma, alloc);
            }
        },
    };

    // Construire un AtomicRefCount temporaire pour décider si copie est nécessaire.
    // En production, la table de refcounts est FRAME_TABLE (non créée ici pour Couche 0).
    // Pour rester indépendant, on utilise l'heuristique : si le physmap montre
    // que la page est partagée (via un compteur dans FrameDesc), on copie.
    // Ici : toujours copier (chemin safe).
    let new_frame = match alloc.alloc_nonzeroed() {
        Ok(f) => f,
        Err(_) => {
            return FaultResult::Oom {
                addr: ctx.fault_addr,
            }
        }
    };

    // Copier les données de l'ancien frame vers le nouveau.
    // SAFETY: Les deux frames sont mappés dans le physmap kernel.
    unsafe {
        let src = (crate::memory::core::layout::PHYS_MAP_BASE.as_u64()
            + old_frame.start_address().as_u64()) as *const u8;
        let dst = (crate::memory::core::layout::PHYS_MAP_BASE.as_u64()
            + new_frame.start_address().as_u64()) as *mut u8;
        // SAFETY: src et dst sont des frames physiques distincts, taille PAGE_SIZE.
        core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE);
    }

    // Remap la page avec les nouveaux flags (writable, supprimer COW).
    let new_flags = vma.page_flags & !PageFlags::COW | PageFlags::WRITABLE;
    let new_raw = PageTableEntry::from_page_flags(new_frame, new_flags).raw();
    match alloc.compare_exchange_pte_raw(page_addr, old_raw, new_raw) {
        Ok(_) => {
            let remaining = COW_TRACKER.dec(old_frame);
            if remaining == 0 {
                alloc.free_frame(old_frame);
            }
            vma.record_cow_break();
            // SAFETY: adresse canonique.
            unsafe {
                flush_single(page_addr);
            }
            FaultResult::Handled
        }
        Err(actual_raw) => {
            alloc.free_frame(new_frame);
            let actual = PageTableEntry::from_raw(actual_raw);
            if actual.is_present() {
                // Un autre CPU a probablement gagné la course et a déjà cassé le CoW.
                unsafe {
                    flush_single(page_addr);
                }
                FaultResult::Handled
            } else {
                super::demand_paging::handle_demand_paging(ctx, vma, alloc)
            }
        }
    }
}
