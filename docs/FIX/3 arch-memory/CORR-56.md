# CORR-56 — isolate.rs : Cage mémoire et IDT inexistantes (CRIT-02 + CRIT-03)

**Sévérité :** 🔴 CRITIQUE — BLOQUANT SÉCURITÉ  
**Fichier :** `kernel/src/exophoenix/isolate.rs`  
**Impact :** Isolation ExoPhoenix fictive — Kernel A garde accès total à sa mémoire et son IDT pendant la phase de cage

---

## Problème

```rust
// ÉTAT ACTUEL
fn mark_a_pages_not_present() {
    // [ADAPT] : walk_and_clear_present(a_cr3)
    // Fonction TOTALEMENT VIDE
}

fn override_a_idt_with_b_handlers() {
    // [ADAPT] : write_idt_entry(a_idtr, 0xF1, b_handler...)
    // Fonction TOTALEMENT VIDE
}
```

---

## Correction CRIT-02 — `mark_a_pages_not_present()`

```rust
/// Marque toutes les pages Ring 0 de Kernel A comme NOT_PRESENT dans ses page tables.
/// 
/// # Sécurité
/// - Appelé après IPI Freeze (tous les cores de A stoppés)
/// - Utilise le CR3 de A stocké dans la SSR (slot CORE_A_SLOT)
/// - TLB shootdown obligatoire après (géré par l'appelant dans isolate_kernel_a_memory)
/// 
/// # Invariant
/// Cette fonction ne modifie que les entrées PTE de A — les tables de pages
/// physiques de B sont inchangées.
fn mark_a_pages_not_present() {
    use crate::memory::core::{PHYS_MAP_BASE, PhysAddr};
    use crate::exophoenix::ssr;

    // Récupérer le CR3 de Kernel A depuis la SSR.
    // Stocké à l'offset SSR_A_CR3 lors du context save de A.
    let a_cr3_phys = unsafe {
        ssr::ssr_atomic(ssr::SSR_A_CR3_OFFSET).load(core::sync::atomic::Ordering::Acquire)
    };

    if a_cr3_phys == 0 {
        // A n'a jamais été démarré ou a déjà été isolé.
        return;
    }

    let a_cr3_phys = PhysAddr::new(a_cr3_phys);

    // Walker les 4 niveaux de tables de pages de A et vider le bit PRESENT.
    // SAFETY: A est gelé (IPI Freeze ACK reçu), CR3 est valide dans la physmap.
    unsafe {
        walk_and_clear_present_pml4(a_cr3_phys);
    }
}

/// Parcourt récursivement le PML4 de Kernel A et efface le bit PRESENT
/// sur toutes les entrées de niveau 1 (PTEs finales).
/// 
/// # SAFETY
/// - `pml4_phys` doit être un PML4 valide dans la physmap
/// - Kernel A doit être gelé (aucun accès concurrent possible)
unsafe fn walk_and_clear_present_pml4(pml4_phys: crate::memory::core::PhysAddr) {
    use crate::memory::core::PHYS_MAP_BASE;
    use crate::memory::virtual_::page_table::entry::PageTableFlags;

    const PRESENT: u64 = 1 << 0;
    const HUGE_PAGE: u64 = 1 << 7;
    const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

    let pml4_virt = PHYS_MAP_BASE.as_u64() + pml4_phys.as_u64();
    let pml4 = &*(pml4_virt as *const [core::sync::atomic::AtomicU64; 512]);

    for pml4e in pml4.iter() {
        let e = pml4e.load(core::sync::atomic::Ordering::Relaxed);
        if e & PRESENT == 0 { continue; }

        let pdpt_phys = e & ADDR_MASK;
        let pdpt_virt = PHYS_MAP_BASE.as_u64() + pdpt_phys;
        let pdpt = &*(pdpt_virt as *const [core::sync::atomic::AtomicU64; 512]);

        for pdpte in pdpt.iter() {
            let e = pdpte.load(core::sync::atomic::Ordering::Relaxed);
            if e & PRESENT == 0 { continue; }
            if e & HUGE_PAGE != 0 {
                // 1GB page — effacer le bit PRESENT directement.
                pdpte.fetch_and(!PRESENT, core::sync::atomic::Ordering::Release);
                continue;
            }

            let pd_phys = e & ADDR_MASK;
            let pd_virt = PHYS_MAP_BASE.as_u64() + pd_phys;
            let pd = &*(pd_virt as *const [core::sync::atomic::AtomicU64; 512]);

            for pde in pd.iter() {
                let e = pde.load(core::sync::atomic::Ordering::Relaxed);
                if e & PRESENT == 0 { continue; }
                if e & HUGE_PAGE != 0 {
                    // 2MB page.
                    pde.fetch_and(!PRESENT, core::sync::atomic::Ordering::Release);
                    continue;
                }

                let pt_phys = e & ADDR_MASK;
                let pt_virt = PHYS_MAP_BASE.as_u64() + pt_phys;
                let pt = &*(pt_virt as *const [core::sync::atomic::AtomicU64; 512]);

                for pte in pt.iter() {
                    // Effacer PRESENT sur chaque PTE finale.
                    pte.fetch_and(!PRESENT, core::sync::atomic::Ordering::Release);
                }
            }
        }
    }

    // Barrière mémoire globale après modification des tables de pages.
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}
```

---

## Correction CRIT-03 — `override_a_idt_with_b_handlers()`

```rust
/// Écrase les vecteurs ExoPhoenix (0xF1, 0xF2, 0xF3) dans l'IDT de Kernel A
/// avec les handlers de Kernel B, empêchant A de reprendre le contrôle.
///
/// # Protocole
/// - 0xF1 : IPI Freeze → handler B : stopper le core, écrire FREEZE_ACK_DONE
/// - 0xF2 : IPI Panic  → handler B : stopper, déclencher escalade vers Threat
/// - 0xF3 : IPI TLB    → handler B : invalider TLB local, écrire TLB_ACK_DONE
///
/// # SAFETY
/// - Kernel A doit être gelé avant l'appel
/// - a_idtr doit être stockée dans la SSR (SSR_A_IDTR_BASE + SSR_A_IDTR_LIMIT)
fn override_a_idt_with_b_handlers() {
    use crate::memory::core::PHYS_MAP_BASE;
    use crate::exophoenix::ssr;
    use crate::arch::x86_64::interrupts::IdtEntry;

    // Récupérer la base de l'IDT de A depuis la SSR.
    let a_idt_phys = unsafe {
        ssr::ssr_atomic(ssr::SSR_A_IDTR_BASE_OFFSET)
            .load(core::sync::atomic::Ordering::Acquire)
    };

    if a_idt_phys == 0 {
        return; // A pas encore initialisé.
    }

    let a_idt_virt = PHYS_MAP_BASE.as_u64() + a_idt_phys;

    // Symboles des handlers B exportés depuis exophoenix/interrupts.rs.
    extern "C" {
        fn exophoenix_b_freeze_handler();
        fn exophoenix_b_panic_handler();
        fn exophoenix_b_tlb_handler();
    }

    // Écrire les entrées IDT pour les vecteurs ExoPhoenix.
    // SAFETY: a_idt_virt pointe sur une IDT valide (256 entrées × 16 bytes = 4096 bytes).
    unsafe {
        let idt = a_idt_virt as *mut [IdtEntry; 256];

        // 0xF1 — IPI Freeze
        (*idt)[0xF1] = IdtEntry::new_kernel_interrupt(
            exophoenix_b_freeze_handler as u64,
            crate::arch::x86_64::gdt::KERNEL_CODE_SELECTOR,
            0, // IST 0 (stack du kernel courant)
        );

        // 0xF2 — IPI Panic
        (*idt)[0xF2] = IdtEntry::new_kernel_interrupt(
            exophoenix_b_panic_handler as u64,
            crate::arch::x86_64::gdt::KERNEL_CODE_SELECTOR,
            0,
        );

        // 0xF3 — IPI TLB
        (*idt)[0xF3] = IdtEntry::new_kernel_interrupt(
            exophoenix_b_tlb_handler as u64,
            crate::arch::x86_64::gdt::KERNEL_CODE_SELECTOR,
            0,
        );
    }

    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}
```

---

## SSR — Offsets nécessaires à ajouter

```rust
// Dans libs/exo-phoenix-ssr/src/lib.rs — ajouter :

/// Offset SSR : base physique de l'IDT de Kernel A (u64).
/// Écrit par Kernel A lors de son init IDT (étape 6 du boot).
pub const SSR_A_IDTR_BASE_OFFSET: usize = 0x200;

/// Offset SSR : CR3 physique de Kernel A (u64).
/// Écrit par Kernel A à chaque context switch (ou au boot).
pub const SSR_A_CR3_OFFSET: usize = 0x208;
```

---

**Dépendances :** CORR-58 (SSR physique → virtuel doit être résolu en premier)  
**Priorité :** Bloquant ExoPhoenix Phase 1 (isolation)
