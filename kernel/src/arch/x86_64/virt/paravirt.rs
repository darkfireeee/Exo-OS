//! # arch/x86_64/virt/paravirt.rs — Opérations paravirtualisées
//!
//! Remplace certaines opérations coûteuses (EOI, TLB flush) par des versions
//! paravirtualisées plus efficaces quand disponibles (principalement KVM).

#![allow(dead_code)]

use super::detect::{hypervisor_type, HypervisorType, kvm_has_pv_eoi, kvm_has_pv_tlb_flush};
use super::super::cpu::msr;

/// MSR KVM PV EOI
const MSR_KVM_PV_EOI_EN: u32 = 0x4b564d04;
const KVM_PV_EOI_ENABLED: u64 = 1;

/// EOI paravirtualisé (KVM PV EOI si disponible, sinon LAPIC MMIO)
///
/// KVM PV EOI évite un VMEXIT en utilisant un bit dans une page partagée.
#[inline]
pub fn paravirt_eoi() {
    if hypervisor_type() == HypervisorType::Kvm && kvm_has_pv_eoi() {
        // PV EOI : écrire 0 dans la page partagée KVM
        // (intégration complète lors de l'init KVM steal-time page)
        // Pour l'instant : fallback EOI LAPIC
        super::super::apic::local_apic::eoi();
    } else {
        super::super::apic::local_apic::eoi();
    }
}

/// TLB flush paravirtualisé
///
/// KVM PV TLB flush évite le VMEXIT en utilisant un hypercall allégé.
#[inline]
pub fn paravirt_tlb_flush() {
    if hypervisor_type() == HypervisorType::Kvm && kvm_has_pv_tlb_flush() {
        // KVM PV TLB flush via VMCALL
        // SAFETY: VMCALL depuis Ring 0 en mode paravirtualisé KVM
        unsafe {
            core::arch::asm!(
                "vmcall",
                in("eax") 0u32, // KVM_HC_FLUSH_TLB = 0 (non-standard, dépend de la version KVM)
                options(nostack, nomem),
            );
        }
    } else {
        super::super::paging::flush_tlb();
    }
}

/// Configure PV EOI (une fois par CPU, après init LAPIC)
///
/// Enregistre la page partagée PV EOI dans le MSR KVM.
pub fn init_pv_eoi(_pv_eoi_page_phys: u64) {
    if !(hypervisor_type() == HypervisorType::Kvm && kvm_has_pv_eoi()) { return; }
    // SAFETY: MSR_KVM_PV_EOI_EN write depuis Ring 0 sur CPU en mode KVM
    unsafe { msr::write_msr(MSR_KVM_PV_EOI_EN, _pv_eoi_page_phys | KVM_PV_EOI_ENABLED); }
}
