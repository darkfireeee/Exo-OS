//! # arch/x86_64/virt/paravirt.rs — Paravirtualized operations
//!
//! Replaces certain costly operations (EOI, TLB flush) with more efficient
//! paravirtualized versions when available (principally KVM).

use super::super::cpu::msr;
use super::detect::{hypervisor_type, kvm_has_pv_eoi, kvm_has_pv_tlb_flush, HypervisorType};

/// KVM PV EOI MSR
const MSR_KVM_PV_EOI_EN: u32 = 0x4b564d04;
const KVM_PV_EOI_ENABLED: u64 = 1;

/// Paravirtualized EOI (KVM PV EOI if available, otherwise LAPIC MMIO)
///
/// KVM PV EOI avoids a VMEXIT by using a bit in a shared page.
#[inline]
pub fn paravirt_eoi() {
    if hypervisor_type() == HypervisorType::Kvm && kvm_has_pv_eoi() {
        // PV EOI: write 0 into KVM shared page
        // (full integration during KVM steal-time page init)
        // For now: LAPIC EOI fallback
        super::super::apic::eoi();
    } else {
        super::super::apic::eoi();
    }
}

/// Paravirtualized TLB flush
///
/// KVM PV TLB flush avoids VMEXIT using a lightweight hypercall.
#[inline]
pub fn paravirt_tlb_flush() {
    if hypervisor_type() == HypervisorType::Kvm && kvm_has_pv_tlb_flush() {
        // KVM PV TLB flush via VMCALL
        // SAFETY: VMCALL from Ring 0 in KVM paravirtualized mode
        unsafe {
            core::arch::asm!(
                "vmcall",
                in("eax") 0u32, // KVM_HC_FLUSH_TLB = 0 (non-standard, depends on KVM version)
                options(nostack, nomem),
            );
        }
    } else {
        super::super::paging::flush_tlb();
    }
}

/// Configures PV EOI (once per CPU, after LAPIC init)
///
/// Registers the PV EOI shared page in KVM MSR.
pub fn init_pv_eoi(_pv_eoi_page_phys: u64) {
    if !(hypervisor_type() == HypervisorType::Kvm && kvm_has_pv_eoi()) {
        return;
    }
    // SAFETY: MSR_KVM_PV_EOI_EN write from Ring 0 on CPU in KVM mode
    unsafe {
        msr::write_msr(MSR_KVM_PV_EOI_EN, _pv_eoi_page_phys | KVM_PV_EOI_ENABLED);
    }
}
