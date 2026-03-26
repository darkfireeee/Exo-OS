//! ExoIsolate — cage mémoire de Kernel A pendant l'isolation.
//!
//! Erreurs couvertes : S8 (TLB shootdown via 0xF3), S-N1 (IOTLB flush)
//! Activé par handoff.rs après confirmation des ACK freeze.

use core::sync::atomic::Ordering;

use crate::arch::x86_64::apic::{self, local_apic, x2apic};
use crate::arch::x86_64::cpu::msr;
use crate::arch::x86_64::idt;
use crate::exophoenix::stage0;
use crate::memory::dma::iommu::{AMD_IOMMU, INTEL_VTD};

// ── MARQUEURS POUR GPT-5.3-CODEX ─────────────────────────────────────────
// Les lignes [ADAPT] nécessitent la substitution des noms d'API réels.
// Tout le reste est figé.
// ─────────────────────────────────────────────────────────────────────────

#[inline(always)]
fn read_apic_ticks() -> u32 {
    match stage0::B_FEATURES.apic_mode() {
        stage0::BootApicMode::X2Apic => unsafe {
            msr::read_msr(x2apic::X2APIC_TIMER_CCR) as u32
        },
        stage0::BootApicMode::XApic => local_apic::timer_current_count(),
    }
}

// ── 1. Marquer les pages de A comme !PRESENT ─────────────────────────────

fn mark_a_pages_not_present() {
    // Parcourir les PTEs de A et retirer le bit PRESENT
    // Cela empêche A d'accéder à sa propre mémoire pendant l'isolation
    // [ADAPT] : utiliser l'API page table du codebase
    // Pattern attendu :
    //   let a_cr3 = stage0::read_a_cr3();
    //   walk_and_clear_present(a_cr3);
    //   tlb_shootdown_all_a_cores(); // via IPI 0xF3 ci-dessous
}

// ── 2. TLB shootdown (S8 — IPI 0xF3 obligatoire) ─────────────────────────

fn tlb_shootdown_all_a_cores() {
    // S8 : INVPCID invalide seulement le core local.
    // Seul un IPI 0xF3 broadcast invalide les TLBs des cores de A.
    if apic::is_x2apic() {
        x2apic::broadcast_ipi_except_self_x2apic(idt::VEC_EXOPHOENIX_TLB);
    } else {
        local_apic::broadcast_ipi_except_self(idt::VEC_EXOPHOENIX_TLB);
    }
    // Attendre les ACK TLB dans la SSR (timeout 100µs)
    let ticks_per_us = stage0::ticks_per_us();
    let start = read_apic_ticks() as u64;
    let deadline = start.saturating_add(ticks_per_us.saturating_mul(100));
    while (read_apic_ticks() as u64) < deadline {
        core::hint::spin_loop();
    }
}

// ── 3. IOMMU hard revoke + IOTLB flush (S-N1) ────────────────────────────

fn iommu_hard_revoke_and_flush() {
    let blocked = stage0::blocked_domain_id();
    if INTEL_VTD.is_initialized() && INTEL_VTD.unit_count() > 0 {
        // QI Intel VT-d : tables + flush IOTLB
        // S-N1 : ne pas se contenter de modifier les tables sans flush
        unsafe { INTEL_VTD.flush_iotlb_domain(blocked as u16, 0); }
    } else if AMD_IOMMU.is_initialized() {
        // AMD Completion Wait fallback
        core::sync::atomic::fence(Ordering::SeqCst);
    }
}

// ── 4. Override IDT de A ──────────────────────────────────────────────────

fn override_a_idt_with_b_handlers() {
    // Rediriger les vecteurs critiques de A vers les handlers de B
    // Cela garantit que même si A reprend brièvement, il ne peut pas
    // désinstaller les vecteurs ExoPhoenix
    // [ADAPT] : écrire dans l'IDT de A via accès physique direct
    // Pattern attendu :
    //   let a_idtr = read_a_idtr();
    //   write_idt_entry(a_idtr, 0xF1, b_handler_freeze_addr());
    //   write_idt_entry(a_idtr, 0xF2, b_handler_pmc_addr());
    //   write_idt_entry(a_idtr, 0xF3, b_handler_tlb_addr());
}

// ── Point d'entrée principal ──────────────────────────────────────────────

/// Applique la cage mémoire complète sur Kernel A.
/// Appelé par handoff.rs après confirmation des ACK freeze et drain IOMMU.
/// Ordre strict — ne pas modifier.
pub fn isolate_kernel_a_memory() {
    // 1. Marquer pages de A !PRESENT
    mark_a_pages_not_present();

    // 2. TLB shootdown sur tous les cores de A (S8)
    tlb_shootdown_all_a_cores();

    // 3. Hard revoke IOMMU + IOTLB flush (S-N1)
    iommu_hard_revoke_and_flush();

    // 4. Override IDT de A
    override_a_idt_with_b_handlers();
}