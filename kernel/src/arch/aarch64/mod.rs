//! # arch/aarch64 — Support AArch64
//!
//! Module d'architecture pour les cibles AArch64 (ARMv8-A 64 bits).
//!
//! ## État
//! Placeholder — l'implémentation complète sera réalisée lors du portage AArch64.
//! Les primitives ci-dessous permettent de compiler le kernel pour aarch64
//! sans erreur de symbole manquant.

// ── Primitives de base ────────────────────────────────────────────────────────

/// Lit le compteur de temps (CNTVCT_EL0)
#[inline(always)]
pub fn read_tsc() -> u64 {
    let val: u64;
    // SAFETY: CNTVCT_EL0 est lisible depuis EL0/EL1 sans restriction
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) val, options(nostack, nomem));
    }
    val
}

/// Arrête le CPU (WFI — Wait For Interrupt)
#[inline(always)]
pub fn halt_cpu() -> ! {
    loop {
        // SAFETY: WFI est une instruction d'attente — sortie par interruption
        unsafe {
            core::arch::asm!("wfi", options(nostack, nomem));
        }
    }
}

/// Désactive les interruptions (DAIF.I = 1)
#[inline(always)]
pub fn irq_disable() {
    // SAFETY: écriture du registre DAIF depuis EL1
    unsafe {
        core::arch::asm!("msr daifset, #2", options(nostack, nomem));
    }
}

/// Active les interruptions (DAIF.I = 0)
#[inline(always)]
pub fn irq_enable() {
    // SAFETY: écriture du registre DAIF depuis EL1
    unsafe {
        core::arch::asm!("msr daifclr, #2", options(nostack, nomem));
    }
}

/// Sauvegarde et désactive les interruptions
#[inline(always)]
pub fn irq_save() -> u64 {
    let daif: u64;
    // SAFETY: lecture DAIF
    unsafe {
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nostack, nomem));
    }
    irq_disable();
    daif
}

/// Restaure l'état des interruptions
#[inline(always)]
pub fn irq_restore(daif: u64) {
    // SAFETY: restauration DAIF précédemment sauvegardé
    unsafe {
        core::arch::asm!("msr daif, {}", in(reg) daif, options(nostack, nomem));
    }
}

/// Barrière mémoire complète (DMB ISH)
#[inline(always)]
pub fn memory_barrier() {
    // SAFETY: instruction barrière — aucun effet de bord sur l'état CPU
    unsafe {
        core::arch::asm!("dmb ish", options(nostack));
    }
}

/// Barrière de charge (DMB ISHLD)
#[inline(always)]
pub fn load_fence() {
    // SAFETY: barrière load-load
    unsafe {
        core::arch::asm!("dmb ishld", options(nostack));
    }
}

/// Barrière d'écriture (DMB ISHST)
#[inline(always)]
pub fn store_fence() {
    // SAFETY: barrière store-store
    unsafe {
        core::arch::asm!("dmb ishst", options(nostack));
    }
}

/// Invalide une page TLB à l'adresse virtuelle donnée
#[inline(always)]
pub fn flush_tlb_page(virt_addr: u64) {
    // SAFETY: TLBI VAAE1IS invalide une entrée TLB EL1 partagée
    unsafe {
        core::arch::asm!(
            "tlbi vaae1is, {}",
            "dsb ish",
            "isb",
            in(reg) virt_addr >> 12,
            options(nostack, nomem),
        );
    }
}

/// Invalide tout le TLB EL1 (toutes les ASID)
#[inline(always)]
pub fn flush_tlb() {
    // SAFETY: TLBI VMALLE1IS vide tout le TLB EL1
    unsafe {
        core::arch::asm!("tlbi vmalle1is", "dsb ish", "isb", options(nostack, nomem),);
    }
}

/// Délai actif en cycles (basé sur CNTVCT_EL0)
pub fn spin_delay_cycles(cycles: u64) {
    let start = read_tsc();
    while read_tsc().wrapping_sub(start) < cycles {
        core::hint::spin_loop();
    }
}

// ── ArchInfo ─────────────────────────────────────────────────────────────────

use super::ArchInfo;

/// Retourne les informations d'architecture AArch64
pub fn arch_info() -> ArchInfo {
    ArchInfo {
        arch_name: "aarch64",
        page_size: 4096,
        cache_line: 64,
        timer_freq_hz: read_cntfrq(),
    }
}

/// Lit la fréquence du compteur générique (CNTFRQ_EL0)
fn read_cntfrq() -> u64 {
    let val: u32;
    // SAFETY: CNTFRQ_EL0 lisible depuis EL0/EL1
    unsafe {
        core::arch::asm!("mrs {:x}, cntfrq_el0", out(reg) val, options(nostack, nomem));
    }
    val as u64
}
