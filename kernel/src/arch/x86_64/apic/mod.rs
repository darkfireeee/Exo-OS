//! # arch/x86_64/apic — Advanced Programmable Interrupt Controller
//!
//! Gestion complète du LAPIC (Local APIC) et de l'I/O APIC pour Exo-OS.
//!
//! ## Sous-modules
//! - `local_apic` : LAPIC MMIO + x2APIC MSR
//! - `io_apic`    : I/O APIC MMIO, redirection table
//! - `x2apic`     : détection + bascule x2APIC
//! - `ipi`        : envoi d'IPIs vers les autres CPUs

pub mod local_apic;
pub mod io_apic;
pub mod x2apic;
pub mod ipi;

pub use local_apic::{eoi, init_local_apic, lapic_id, ApicMode};
pub use ipi::{
    send_ipi_wakeup, send_ipi_reschedule, send_ipi_tlb_shootdown,
    send_ipi_cpu_hotplug, broadcast_ipi_panic,
};

use core::sync::atomic::{AtomicBool, Ordering};

/// Mode APIC courant du système
static X2APIC_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Initialise l'APIC system complet (BSP only — appelé depuis boot/early_init)
///
/// Séquence :
/// 1. Détecter le support x2APIC (CPUID)
/// 2. Activer LAPIC (x2APIC ou xAPIC MMIO)
/// 3. Configurer le vecteur spurious
/// 4. Mettre en place le timer LAPIC
pub fn init_apic_system() {
    let features = super::cpu::features::cpu_features();

    if features.has_x2apic() {
        x2apic::enable_x2apic();
        // Masquer tous les LVT entries x2APIC (état BIOS peut laisser LINT0/LVT indéfinis)
        x2apic::mask_all_lvt_x2apic();
        X2APIC_ACTIVE.store(true, Ordering::Release);
    } else {
        // init_local_apic() : active xAPIC + masque TOUS les LVT (LINT0, THERMAL, PERF, CMCI,
        // ERROR). Sans cela, le BIOS QEMU peut laisser LINT0 avec vecteur 0x8E non-masqué :
        // quand le PIC envoie une IRQ, LINT0 délivre vecteur 0x8E → IDT[0x8E] absent → #GP.
        local_apic::init_local_apic();
        X2APIC_ACTIVE.store(false, Ordering::Release);
    }

    // Configurer vecteur spurious (0xFF) + soft-enable LAPIC
    local_apic::set_spurious_vector(super::idt::VEC_SPURIOUS);

    // Timer : utiliser TSC-Deadline si disponible, sinon one-shot
    if features.has_tsc_deadline() {
        local_apic::timer_init_tsc_deadline(super::idt::VEC_IRQ_TIMER);
    } else {
        local_apic::timer_init_oneshot(super::idt::VEC_IRQ_TIMER);
    }
}

/// Initialise le LAPIC de l'AP courant (appelé depuis smp/init)
pub fn init_ap_local_apic() {
    if X2APIC_ACTIVE.load(Ordering::Acquire) {
        x2apic::enable_x2apic();
    } else {
        local_apic::enable_xapic();
    }
    local_apic::set_spurious_vector(super::idt::VEC_SPURIOUS);
}

/// Retourne `true` si x2APIC est actif
#[inline(always)]
pub fn is_x2apic() -> bool {
    X2APIC_ACTIVE.load(Ordering::Relaxed)
}
