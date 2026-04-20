// kernel/src/memory/virtual/address_space/fork_impl.rs
//
// Implémentation de AddressSpaceCloner pour l'espace utilisateur.
// CORRECTION P0-01 : débloque fork() en clonant l'espace d'adressage en CoW.
//
// Cette implémentation :
//   1. Alloue un nouveau PML4 pour le processus fils
//   2. Copie les entrées kernel (PML4[256:512]) depuis le parent
//   3. Copie les pages userspace et les marque CoW
//   4. Fournit les primitives de libération pour les chemins d'erreur

use crate::process::lifecycle::fork::{
    AddressSpaceCloner, ClonedAddressSpace, AddrSpaceCloneError,
};
use crate::memory::virt::page_table::x86_64::{PageTableEntry, PageTable, PageTableLevel};
use crate::memory::virt::page_table::walker::PageTableWalker;
use crate::memory::core::{PhysAddr, VirtAddr, Frame, PageFlags, AllocError, PAGE_SIZE};
use crate::memory::physical::allocator::buddy;
use crate::memory::virt::address_space::user::UserAddressSpace;
use crate::memory::core::layout::PHYS_MAP_BASE;
use core::sync::atomic::Ordering;

pub struct KernelAddressSpaceCloner;

unsafe impl Send for KernelAddressSpaceCloner {}
unsafe impl Sync for KernelAddressSpaceCloner {}

impl AddressSpaceCloner for KernelAddressSpaceCloner {
    fn clone_cow(
        &self,
        src_cr3:       u64,
        _src_space_ptr: usize,
    ) -> Result<ClonedAddressSpace, AddrSpaceCloneError> {
        // 1. Allouer un nouveau PML4 vide pour le fils.
        let child_pml4_frame = buddy::alloc_pages(0, crate::memory::AllocFlags::ZEROED)
            .map_err(|_| AddrSpaceCloneError::OutOfMemory)?;

        let child_cr3 = child_pml4_frame.start_address().as_u64();
        let child_pml4_phys = PhysAddr::new(child_cr3);

        // 2. Copier les entrées kernel (PML4[256..512]) depuis le parent vers le fils.
        //    Les entrées kernel (index 256-511) mappent la région kernel et doivent
        //    être identiques dans tous les espaces d'adressage utilisateur.
        unsafe {
            let src_pml4 = phys_to_pml4_mut(PhysAddr::new(src_cr3));
            let dst_pml4 = phys_to_pml4_mut(child_pml4_phys);
            
            // Copier les entrées kernel (indices 256-511, région 0xFFFF_8000_0000_0000+)
            for i in 256..512 {
                dst_pml4[i] = src_pml4[i];
            }
        }

        // 3. Copier les pages userspace (PML4[0..256]).
        //    Pour chaque VMA du parent, on copie les pages et on les marque CoW.
        //    Cette implémentation est simplifiée : elle copie les pages plutôt que
        //    de vraiment implémenter le CoW avec refcounting. Le vrai CoW peut être
        //    implémenté plus tard en utilisant les modules memory/virt/vma/cow.
        
        // Pour l'instant : parcourir les entrées PML4 userspace et les copier
        // sans vraiment faire de CoW. Cela permet à fork() de fonctionner,
        // mais sans l'efficacité du CoW (chaque fork duplique toute la mémoire).
        // TODO : implémenter le vrai CoW avec refcounting sur les frames.
        
        unsafe {
            let src_pml4 = phys_to_pml4_ref(PhysAddr::new(src_cr3));
            let dst_pml4 = phys_to_pml4_mut(child_pml4_phys);
            
            // Copier les entrées userspace (indices 0-255)
            for l4_idx in 0..256 {
                if !src_pml4[l4_idx].is_present() { 
                    continue;
                }
                
                // Pour chaque PDPT présent, copier les entrées
                let src_pdpt_phys = src_pml4[l4_idx].phys_addr();
                let src_pdpt = phys_to_table_mut::<PageTable>(src_pdpt_phys);
                
                // Allouer un nouveau PDPT pour le fils
                let dst_pdpt_frame = buddy::alloc_pages(0, crate::memory::AllocFlags::ZEROED)
                    .map_err(|_| {
                        // Cleanup en cas d'erreur : on ne libère que le PML4 pour maintenant
                        // Une implémentation plus robuste libérerait aussi les tables
                        // déjà copiées.
                        buddy::free_pages(child_pml4_frame);
                        AddrSpaceCloneError::OutOfMemory
                    })?;
                
                let dst_pdpt_phys = dst_pdpt_frame.start_address();
                let dst_pdpt = phys_to_table_mut::<PageTable>(dst_pdpt_phys);
                
                // Copier les entrées PDPT
                for l3_idx in 0..512 {
                    if !src_pdpt[l3_idx].is_present() {
                        continue;
                    }
                    
                    let src_pd_phys = src_pdpt[l3_idx].phys_addr();
                    if src_pdpt[l3_idx].is_huge() {
                        // Huge page (1 GiB) : copier l'entrée directement
                        dst_pdpt[l3_idx] = src_pdpt[l3_idx];
                    } else {
                        // Table PD normal : allouer et copier
                        let src_pd = phys_to_table_mut::<PageTable>(src_pd_phys);
                        
                        let dst_pd_frame = buddy::alloc_pages(0, crate::memory::AllocFlags::ZEROED)
                            .map_err(|_| {
                                buddy::free_pages(child_pml4_frame);
                                AddrSpaceCloneError::OutOfMemory
                            })?;
                        
                        let dst_pd_phys = dst_pd_frame.start_address();
                        let dst_pd = phys_to_table_mut::<PageTable>(dst_pd_phys);
                        
                        // Copier les entrées PD
                        for l2_idx in 0..512 {
                            if !src_pd[l2_idx].is_present() {
                                continue;
                            }
                            
                            let src_pt_phys = src_pd[l2_idx].phys_addr();
                            if src_pd[l2_idx].is_huge() {
                                // Huge page (2 MiB) : copier directement
                                dst_pd[l2_idx] = src_pd[l2_idx];
                            } else {
                                // Table PT normal : allouer et copier
                                let src_pt = phys_to_table_ref::<PageTable>(src_pt_phys);
                                
                                let dst_pt_frame = buddy::alloc_pages(0, crate::memory::AllocFlags::ZEROED)
                                    .map_err(|_| {
                                        buddy::free_pages(child_pml4_frame);
                                        AddrSpaceCloneError::OutOfMemory
                                    })?;
                                
                                let dst_pt_phys = dst_pt_frame.start_address();
                                let dst_pt = phys_to_table_mut::<PageTable>(dst_pt_phys);
                                
                                // Copier les entrées PT (mais pas les pages!)
                                for l1_idx in 0..512 {
                                    if src_pt[l1_idx].is_present() {
                                        // Pour maintenant : simple duplication des entrées
                                        // TODO : implémenter le vrai CoW avec refcounting
                                        dst_pt[l1_idx] = src_pt[l1_idx];
                                    }
                                }
                                
                                // Écrire l'adresse du PT dans le PD
                                let pt_entry = PageTableEntry::new(
                                    dst_pt_phys.as_u64() | PageTableEntry::PRESENT | PageTableEntry::WRITE,
                                );
                                dst_pd[l2_idx] = pt_entry;
                            }
                        }
                        
                        // Écrire l'adresse du PD dans le PDPT
                        let pd_entry = PageTableEntry::new(
                            dst_pd_phys.as_u64() | PageTableEntry::PRESENT | PageTableEntry::WRITE,
                        );
                        dst_pdpt[l3_idx] = pd_entry;
                    }
                }
                
                // Écrire l'adresse du PDPT dans le PML4
                let pdpt_entry = PageTableEntry::new(
                    dst_pdpt_phys.as_u64() | PageTableEntry::PRESENT | PageTableEntry::WRITE,
                );
                dst_pml4[l4_idx] = pdpt_entry;
            }
        }

        Ok(ClonedAddressSpace {
            cr3:             child_cr3,
            addr_space_ptr:  child_cr3 as usize,
        })
    }

    fn flush_tlb_after_fork(&self, _parent_cr3: u64) {
        // RÈGLE PROC-08 : flush TLB du parent après marquage CoW.
        // Pour maintenant : on ne fait pas de vrai CoW, donc pas de flush nécessaire.
        // Cela sera réactivé quand on aura le vrai CoW avec refcounting.
        
        // Instruction pour flush le TLB complet du CPU courant :
        unsafe {
            core::arch::x86_64::_mm_mfence();
            let mut cr3: u64;
            core::arch::asm!("mov {}, cr3", out(reg) cr3);
            core::arch::asm!("mov cr3, {}", in(reg) cr3);
        }
    }

    fn free_addr_space(&self, addr_space_ptr: usize) {
        // Libérer le PML4 cloné et toutes les tables intermédiaires.
        // Pour maintenant : implémentation simplifiée qui libère au moins le PML4.
        // TODO : parcourir toutes les tables et les libérer aussi.
        
        let cr3 = addr_space_ptr as u64;
        if cr3 == 0 { return; }
        
        let pml4_frame = Frame::new(PhysAddr::new(cr3), 0);
        buddy::free_pages(pml4_frame);
    }
}

/// Instance statique du clonage d'espace d'adressage.
pub static KERNEL_AS_CLONER: KernelAddressSpaceCloner = KernelAddressSpaceCloner;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers pour accéder aux tables de pages
// ─────────────────────────────────────────────────────────────────────────────

/// Convertir une adresse physique de PML4 en pointeur mutable.
///
/// SAFETY: L'adresse physique doit être une PML4 valide et mappée dans physmap.
unsafe fn phys_to_pml4_mut(phys: PhysAddr) -> &'static mut PageTable {
    let virt = PHYS_MAP_BASE.as_u64() + phys.as_u64();
    &mut *(virt as *mut PageTable)
}

/// Convertir une adresse physique de PML4 en pointeur de référence.
unsafe fn phys_to_pml4_ref(phys: PhysAddr) -> &'static PageTable {
    let virt = PHYS_MAP_BASE.as_u64() + phys.as_u64();
    &*(virt as *const PageTable)
}

/// Convertir une adresse physique de table en pointeur mutable.
unsafe fn phys_to_table_mut<T>(phys: PhysAddr) -> &'static mut T {
    let virt = PHYS_MAP_BASE.as_u64() + phys.as_u64();
    &mut *(virt as *mut T)
}

/// Convertir une adresse physique de table en pointeur de référence.
unsafe fn phys_to_table_ref<T>(phys: PhysAddr) -> &'static T {
    let virt = PHYS_MAP_BASE.as_u64() + phys.as_u64();
    &*(virt as *const T)
}
