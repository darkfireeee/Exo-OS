//! # arch/x86_64/apic/x2apic.rs — x2APIC (MSR mode)
//!
//! Le mode x2APIC remplace l'accès MMIO du xAPIC par des MSRs.
//! L'espace d'adressage APIC ID passe de 8 bits à 32 bits.
//!
//! ## Plage MSR x2APIC
//! Les registres LAPIC sont accédés via MSR 0x800–0x8FF.
//! Chaque registre MMIO offset 0xXY0 devient MSR 0x800 + (0xXY0 >> 4).

use super::super::cpu::msr;

// ── Constantes MSR x2APIC ─────────────────────────────────────────────────────

const X2APIC_BASE: u32 = 0x800;

pub const X2APIC_ID: u32 = X2APIC_BASE + 0x02; // 0x802
pub const X2APIC_VER: u32 = X2APIC_BASE + 0x03;
pub const X2APIC_TPR: u32 = X2APIC_BASE + 0x08;
pub const X2APIC_PPR: u32 = X2APIC_BASE + 0x0A;
pub const X2APIC_EOI: u32 = X2APIC_BASE + 0x0B;
pub const X2APIC_LDR: u32 = X2APIC_BASE + 0x0D;
pub const X2APIC_SIVR: u32 = X2APIC_BASE + 0x0F;
pub const X2APIC_ISR0: u32 = X2APIC_BASE + 0x10;
pub const X2APIC_ESR: u32 = X2APIC_BASE + 0x28;
pub const X2APIC_LVT_CMCI: u32 = X2APIC_BASE + 0x2F;
pub const X2APIC_ICR: u32 = X2APIC_BASE + 0x30; // ICR 64 bits (combiné hi+lo)
pub const X2APIC_LVT_TIMER: u32 = X2APIC_BASE + 0x32;
pub const X2APIC_LVT_THERMAL: u32 = X2APIC_BASE + 0x33;
pub const X2APIC_LVT_PERF: u32 = X2APIC_BASE + 0x34;
pub const X2APIC_LVT_LINT0: u32 = X2APIC_BASE + 0x35;
pub const X2APIC_LVT_LINT1: u32 = X2APIC_BASE + 0x36;
pub const X2APIC_LVT_ERROR: u32 = X2APIC_BASE + 0x37;
pub const X2APIC_TIMER_ICR: u32 = X2APIC_BASE + 0x38;
pub const X2APIC_TIMER_CCR: u32 = X2APIC_BASE + 0x39;
pub const X2APIC_TIMER_DCR: u32 = X2APIC_BASE + 0x3E;
pub const X2APIC_SELF_IPI: u32 = X2APIC_BASE + 0x3F; // Self-IPI (x2APIC uniquement)

const MSR_IA32_APICBASE: u32 = 0x001B;
const APICBASE_ENABLE: u64 = 1 << 11;
const APICBASE_EXTD: u64 = 1 << 10;

// ── Lecture / écriture via MSR ────────────────────────────────────────────────

/// Lit un registre x2APIC (32 bits)
#[inline(always)]
pub fn x2apic_read32(reg: u32) -> u32 {
    // SAFETY: registre x2APIC validé, Ring 0 seulement
    unsafe { msr::read_msr(reg) as u32 }
}

/// Lit un registre x2APIC (64 bits, ICR uniquement)
#[inline(always)]
pub fn x2apic_read64(reg: u32) -> u64 {
    // SAFETY: reg doit être X2APIC_ICR pour les 64 bits
    unsafe { msr::read_msr(reg) }
}

/// Écrit un registre x2APIC (32 bits)
#[inline(always)]
pub fn x2apic_write32(reg: u32, val: u32) {
    // SAFETY: registre x2APIC validé, Ring 0 uniquement
    unsafe {
        msr::write_msr(reg, val as u64);
    }
}

/// Écrit ICR 64 bits (envoi IPI unique — pas d'attente delivery status en x2APIC)
#[inline(always)]
pub fn x2apic_write_icr(val: u64) {
    // SAFETY: MSR X2APIC_ICR est Write-Only en x2APIC
    unsafe {
        msr::write_msr(X2APIC_ICR, val);
    }
}

// ── Activation x2APIC ─────────────────────────────────────────────────────────

/// Masque tous les LVT en mode x2APIC.
///
/// Doit être appelé juste après `enable_x2apic()` pour s'assurer que les
/// entrées LVT laissées par le BIOS (ex. LINT0 avec vecteur 0x8E) ne peuvent
/// pas livrer d'interruptions vers des vecteurs IDT non-enregistrés → #GP.
pub fn mask_all_lvt_x2apic() {
    const MASKED: u32 = 0x0001_0000; // bit 16 = mask
    x2apic_write32(X2APIC_LVT_THERMAL, MASKED);
    x2apic_write32(X2APIC_LVT_PERF, MASKED);
    x2apic_write32(X2APIC_LVT_CMCI, MASKED);
    x2apic_write32(X2APIC_LVT_LINT0, MASKED);
    x2apic_write32(X2APIC_LVT_LINT1, 0x0000_0400); // NMI, non masqué
    x2apic_write32(X2APIC_LVT_ERROR, 0x0000_00FE); // vecteur 0xFE, non masqué
                                                   // Effacer ESR
    x2apic_write32(X2APIC_ESR, 0);
    x2apic_write32(X2APIC_ESR, 0);
    // Task Priority : accepter toutes les interruptions
    x2apic_write32(X2APIC_TPR, 0);
}

/// Active le mode x2APIC (bits 10+11 du MSR IA32_APICBASE)
///
/// Transition : xAPIC (bit 11 seul) → x2APIC (bits 11+10)
/// **Irréversible** : une fois activé, le retour xAPIC nécessite un reset.
pub fn enable_x2apic() {
    // SAFETY: transition xAPIC → x2APIC ; Ring 0, CPU supporte x2APIC (vérifié par cpu::features)
    let apicbase = unsafe { msr::read_msr(MSR_IA32_APICBASE) };
    // SAFETY: écriture MSR APICBASE pour activer x2APIC
    unsafe {
        msr::write_msr(
            MSR_IA32_APICBASE,
            apicbase | APICBASE_ENABLE | APICBASE_EXTD,
        );
    }

    // Activation LAPIC soft via SIVR
    let sivr = x2apic_read32(X2APIC_SIVR) | 0x100;
    x2apic_write32(X2APIC_SIVR, sivr);
}

// ── Primitives x2APIC ─────────────────────────────────────────────────────────

/// EOI en mode x2APIC
#[inline(always)]
pub fn eoi_x2apic() {
    x2apic_write32(X2APIC_EOI, 0);
}

/// ID LAPIC 32 bits en mode x2APIC
#[inline]
pub fn x2apic_id() -> u32 {
    x2apic_read32(X2APIC_ID)
}

/// Envoie un IPI en mode x2APIC
///
/// ICR est un registre 64 bits unique — pas de delivery status à attendre.
/// Format ICR x2APIC :
///   bits 7:0   = vector
///   bits 10:8  = delivery mode
///   bit  11    = destination mode (0=physical, 1=logical)
///   bit  14    = level (1=assert)
///   bit  15    = trigger mode (0=edge, 1=level)
///   bits 19:18 = destination shorthand
///   bits 63:32 = destination (APIC ID 32 bits)
pub fn send_ipi_x2apic(dest: u32, vector: u8, delivery_mode: u64) {
    let icr: u64 = ((dest as u64) << 32)
        | (1 << 14)          // level assert
        | delivery_mode
        | (vector as u64);
    x2apic_write_icr(icr);
}

/// Broadcast IPI vers tous sauf soi-même en x2APIC
pub fn broadcast_ipi_except_self_x2apic(vector: u8) {
    // shorthand 0b11 = all excluding self
    let icr: u64 = (0b11u64 << 18) | (1 << 14) | (vector as u64);
    x2apic_write_icr(icr);
}

/// Self-IPI en x2APIC (registre dédié MSR 0x83F)
#[inline]
pub fn self_ipi_x2apic(vector: u8) {
    x2apic_write32(X2APIC_SELF_IPI, vector as u32);
}

/// Configuration LVT timer en mode TSC-Deadline
pub fn timer_init_tsc_deadline_x2apic(vector: u8) {
    let mode_tsc_deadline = 0b10 << 17;
    x2apic_write32(X2APIC_LVT_TIMER, mode_tsc_deadline | (vector as u32));
}

/// Programme la prochaine deadline TSC (même qu'en xAPIC)
#[inline]
pub fn timer_set_deadline_x2apic(deadline: u64) {
    // SAFETY: MSR TSC_DEADLINE disponible si CPUID validé
    unsafe {
        msr::write_msr(msr::MSR_TSC_DEADLINE, deadline);
    }
}
