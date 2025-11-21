//! # Gestion de l'APIC (Advanced Programmable Interrupt Controller)
//!
//! Ce module se concentre sur l'utilisation du x2APIC, la version moderne et
//! performante de l'APIC. Le x2APIC est accessible via des MSRs, ce qui est
//! plus rapide que l'accès mémoire du mode xAPIC.

use crate::arch::x86_64::registers::{read_msr, write_msr};
use raw_cpuid::CpuId;

// Constantes pour les MSRs du x2APIC
const IA32_APIC_BASE: u32 = 0x1B;
const IA32_X2APIC_ID: u32 = 0x802;
const IA32_X2APIC_VERSION: u32 = 0x803;
const IA32_X2APIC_TPR: u32 = 0x808;
const IA32_X2APIC_PPR: u32 = 0x80A;
const IA32_X2APIC_EOI: u32 = 0x80B;
const IA32_X2APIC_LDR: u32 = 0x80D;
const IA32_X2APIC_SVR: u32 = 0x80F;
const IA32_X2APIC_ISR_BASE: u32 = 0x810;
const IA32_X2APIC_TMR_BASE: u32 = 0x818;
const IA32_X2APIC_IRR_BASE: u32 = 0x820;
const IA32_X2APIC_ESR: u32 = 0x828;
const IA32_X2APIC_ICR: u32 = 0x830;
const IA32_X2APIC_LVT_TIMER: u32 = 0x832;
const IA32_X2APIC_LVT_THERMAL: u32 = 0x833;
const IA32_X2APIC_LVT_PERFMON: u32 = 0x834;
const IA32_X2APIC_LVT_LINT0: u32 = 0x835;
const IA32_X2APIC_LVT_LINT1: u32 = 0x836;
const IA32_X2APIC_LVT_ERROR: u32 = 0x837;
const IA32_X2APIC_INIT_COUNT: u32 = 0x838;
const IA32_X2APIC_CUR_COUNT: u32 = 0x839;
const IA32_X2APIC_DIV_CONF: u32 = 0x83E;

/// Initialise le Local APIC (LAPIC) en mode x2APIC.
pub fn init() {
    if !is_x2apic_supported() {
        panic!("x2APIC non supporté. Exo-OS requiert x2APIC pour la performance.");
    }

    disable_pic();
    enable_x2apic();

    unsafe {
        // Activer le LAPIC et définir le vecteur pour les interruptions parasites (spurious).
        let svr_val = read_msr(IA32_X2APIC_SVR) | (1 << 8) | 0xFF; // Enable | Spurious Vector 0xFF
        write_msr(IA32_X2APIC_SVR, svr_val);

        // Accepter toutes les interruptions.
        write_msr(IA32_X2APIC_TPR, 0);
    }

    log::info!("LAPIC initialisé en mode x2APIC.");
}

/// Vérifie si le x2APIC est supporté par le CPU.
fn is_x2apic_supported() -> bool {
    CpuId::new().get_feature_info().map_or(false, |f| f.has_x2apic())
}

/// Active le mode x2APIC.
fn enable_x2apic() {
    unsafe {
        let mut apic_base = read_msr(IA32_APIC_BASE);
        // Bit 10: x2APIC enable, Bit 11: APIC global enable
        apic_base |= (1 << 10) | (1 << 11);
        write_msr(IA32_APIC_BASE, apic_base);
    }
}

/// Désactive l'ancien PIC 8259.
fn disable_pic() {
    unsafe {
        // ICW1: Start initialization
        asm!("out 0x20, al", in("al") 0x11u8);
        asm!("out 0xA0, al", in("al") 0x11u8);

        // ICW2: Remap IRQs to vectors 32-47
        asm!("out 0x21, al", in("al") 0x20u8);
        asm!("out 0xA1, al", in("al") 0x28u8);

        // ICW3: Setup cascading
        asm!("out 0x21, al", in("al") 0x04u8);
        asm!("out 0xA1, al", in("al") 0x02u8);

        // ICW4: 8086 mode
        asm!("out 0x21, al", in("al") 0x01u8);
        asm!("out 0xA1, al", in("al") 0x01u8);

        // Mask all interrupts
        asm!("out 0x21, al", in("al") 0xFFu8);
        asm!("out 0xA1, al", in("al") 0xFFu8);
    }
    log::debug!("PIC 8259 désactivé.");
}

/// Envoie un signal End-Of-Interrupt (EOI) au LAPIC.
/// Doit être appelé à la fin de chaque gestionnaire d'interruption.
#[inline]
pub fn eoi() {
    unsafe {
        write_msr(IA32_X2APIC_EOI, 0);
    }
}

/// Envoie un IPI (Inter-Processor Interrupt) de type INIT.
pub fn send_init_ipi(apic_id: u32) {
    // Delivery mode: 0b101 (INIT)
    let icr = (apic_id as u64) << 32 | (1 << 14) | (0b101 << 8);
    unsafe {
        write_msr(IA32_X2APIC_ICR, icr);
    }
}

/// Envoie un IPI (Inter-Processor Interrupt) de type SIPI (Start-up).
pub fn send_sipi_ipi(apic_id: u32, vector: u8) {
    // Delivery mode: 0b110 (SIPI)
    let icr = (apic_id as u64) << 32 | (1 << 14) | (0b110 << 8) | vector as u64;
    unsafe {
        write_msr(IA32_X2APIC_ICR, icr);
    }
}
