//! # cpu/msr.rs — Modèle-Specific Registers x86_64
//!
//! Lecture et écriture des MSR via RDMSR / WRMSR.
//! Les constantes MSR référencées par les autres modules sont groupées ici.
//!
//! ## RÈGLE SÉCURITÉ
//! Ces fonctions sont `unsafe` — l'appelant garantit que le MSR existe
//! sur le CPU cible et que la valeur écrite ne viole aucun invariant.


use core::sync::atomic::Ordering;

// ── Constantes MSR ────────────────────────────────────────────────────────────

/// MSR IA32_APIC_BASE : adresse base LAPIC + flags enable/x2apic
pub const MSR_IA32_APIC_BASE: u32 = 0x0000_001B;

/// MSR IA32_EFER : Extended Feature Enable Register (LME, NXE, SCE…)
pub const MSR_IA32_EFER: u32 = 0xC000_0080;

/// Bit NXE dans EFER (No-Execute Enable)
pub const EFER_NXE: u64 = 1 << 11;

/// Bit SCE dans EFER (SysCall Enable)
pub const EFER_SCE: u64 = 1 << 0;

/// Bit LME dans EFER (Long Mode Enable)
pub const EFER_LME: u64 = 1 << 8;

/// Bit LMA dans EFER (Long Mode Active)
pub const EFER_LMA: u64 = 1 << 10;

/// MSR STAR : segments CS/SS pour SYSCALL/SYSRET
pub const MSR_STAR: u32 = 0xC000_0081;

/// MSR LSTAR : adresse 64 bits de l'handler SYSCALL
pub const MSR_LSTAR: u32 = 0xC000_0082;

/// MSR CSTAR : adresse handler SYSCALL compat (32 bits)
pub const MSR_CSTAR: u32 = 0xC000_0083;

/// MSR SFMASK : masque RFLAGS appliqué lors d'un SYSCALL
pub const MSR_SFMASK: u32 = 0xC000_0084;

/// MSR GS_BASE : base du segment GS (kernel)
pub const MSR_GS_BASE: u32 = 0xC000_0101;

/// MSR KERNEL_GS_BASE : base shadow GS (userspace)
pub const MSR_KERNEL_GS_BASE: u32 = 0xC000_0102;

/// MSR FS_BASE : base du segment FS (userspace TLS)
pub const MSR_FS_BASE: u32 = 0xC000_0100;

/// MSR TSC_AUX : valeur auxiliaire retournée par RDTSCP (CPU ID logique)
pub const MSR_TSC_AUX: u32 = 0xC000_0103;

/// MSR IA32_TSC_DEADLINE : deadline timer LAPIC TSC
pub const MSR_TSC_DEADLINE: u32 = 0x0000_06E0;

/// MSR IA32_PKRS : Supervisor Protection Keys Rights for User pages
pub const MSR_IA32_PKRS: u32 = 0x0000_06E1;

/// MSR IA32_PL0_SSP — Ring 0 Shadow Stack Pointer (CET, Intel SDM Vol.4 §2.1)
/// Contient le SSP du thread courant en mode Ring 0.
/// Doit être sauvegardé/restauré à chaque context switch si CET-SS est actif.
/// FIX-CET-01
pub const MSR_IA32_PL0_SSP: u32 = 0x0000_06A4;

/// MSR IA32_PMC0..7 : compteurs performances
pub const MSR_IA32_PMC0: u32 = 0x0000_00C1;

/// MSR IA32_PERFEVTSEL0 : sélection événements PMU
pub const MSR_IA32_PERFEVTSEL0: u32 = 0x0000_0186;

/// MSR IA32_FIXED_CTR0 : compteur fixe cycles
pub const MSR_IA32_FIXED_CTR0: u32 = 0x0000_0309;

/// MSR IA32_FIXED_CTR_CTRL : contrôle compteurs fixes
pub const MSR_IA32_FIXED_CTR_CTRL: u32 = 0x0000_038D;

/// MSR IA32_PERF_GLOBAL_CTRL : activation globale PMU
pub const MSR_IA32_PERF_GLOBAL_CTRL: u32 = 0x0000_038F;

/// MSR IA32_SPEC_CTRL : contrôle IBRS/STIBP/SSBD
pub const MSR_IA32_SPEC_CTRL: u32 = 0x0000_0048;

/// Bit IBRS dans SPEC_CTRL
pub const SPEC_CTRL_IBRS: u64 = 1 << 0;

/// Bit STIBP dans SPEC_CTRL
pub const SPEC_CTRL_STIBP: u64 = 1 << 1;

/// Bit SSBD dans SPEC_CTRL
pub const SPEC_CTRL_SSBD: u64 = 1 << 2;

/// MSR IA32_PRED_CMD : commandes prédiction (IBPB flush)
pub const MSR_IA32_PRED_CMD: u32 = 0x0000_0049;

/// Bit IBPB dans PRED_CMD
pub const PRED_CMD_IBPB: u64 = 1 << 0;

/// MSR IA32_FLUSH_CMD : flush L1D cache (MDS)
pub const MSR_IA32_FLUSH_CMD: u32 = 0x0000_010B;

/// Bit L1D_FLUSH dans FLUSH_CMD
pub const FLUSH_CMD_L1D: u64 = 1 << 0;

/// MSR IA32_ARCH_CAP : capacités architecturales (RDCL_NO, IBRS_ALL…)
pub const MSR_IA32_ARCH_CAP: u32 = 0x0000_010A;

/// Bit RDCL_NO : CPU non vulnérable à Meltdown
pub const ARCH_CAP_RDCL_NO: u64    = 1 << 0;

/// Bit IBRS_ALL : IBRS fonctionne en mode "always on"
pub const ARCH_CAP_IBRS_ALL: u64   = 1 << 1;

/// Bit RSBA : RSB alternative prediction possible
pub const ARCH_CAP_RSBA: u64       = 1 << 2;

/// Bit SSB_NO : CPU non vulnérable à Spectre v4
pub const ARCH_CAP_SSB_NO: u64     = 1 << 4;

/// MSR IA32_TSC_ADJUST : ajustement TSC (migration VM)
pub const MSR_IA32_TSC_ADJUST: u32 = 0x0000_003B;

/// MSR IA32_MTRR_DEF_TYPE : type mémoire par défaut
pub const MSR_IA32_MTRR_DEF_TYPE: u32 = 0x0000_02FF;

/// MSR IA32_PAT : Page Attribute Table
pub const MSR_IA32_PAT: u32 = 0x0000_0277;

/// Valeur PAT par défaut (WB, WT, UC-, UC, WB, WT, UC-, UC)
pub const PAT_DEFAULT: u64 = 0x0007_0406_0007_0406;

// ── Fonctions de lecture/écriture ─────────────────────────────────────────────

/// Lit un MSR 64 bits
///
/// # SAFETY
/// L'appelant garantit :
/// - Le CPU courant supporte ce MSR (vérifié via CPUID si nécessaire)
/// - Ce MSR est lisible depuis Ring 0
/// - Aucune exception #GP ne sera déclenchée
#[inline(always)]
pub unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: délégué à l'appelant ; rdmsr est privilégié Ring 0
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx")  msr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Écrit un MSR 64 bits
///
/// # SAFETY
/// L'appelant garantit :
/// - Le CPU courant supporte ce MSR en écriture
/// - La valeur `val` respecte les contraintes documentées du MSR
/// - Ce MSR est accessible en écriture depuis Ring 0
#[inline(always)]
pub unsafe fn write_msr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    // SAFETY: délégué à l'appelant ; wrmsr est privilégié Ring 0
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx")  msr,
            in("eax")  lo,
            in("edx")  hi,
            options(nostack, nomem)
        );
    }
}

/// Lit un MSR et applique un masque AND (lecture de bits)
///
/// # SAFETY
/// Mêmes garanties que `read_msr`.
#[inline(always)]
pub unsafe fn read_msr_bits(msr: u32, mask: u64) -> u64 {
    // SAFETY: délégué à l'appelant
    unsafe { read_msr(msr) & mask }
}

/// Modifie des bits d'un MSR (read-modify-write atomique du point de vue du CPU)
///
/// # SAFETY
/// Mêmes garanties que `write_msr` et `read_msr`.
/// L'appelant doit s'assurer que la modification est cohérente avec l'état du système.
#[inline(always)]
pub unsafe fn set_msr_bits(msr: u32, bits: u64) {
    // SAFETY: délégué à l'appelant
    unsafe {
        let val = read_msr(msr);
        write_msr(msr, val | bits);
    }
}

/// Efface des bits d'un MSR
///
/// # SAFETY
/// Mêmes garanties que `set_msr_bits`.
#[inline(always)]
pub unsafe fn clear_msr_bits(msr: u32, bits: u64) {
    // SAFETY: délégué à l'appelant
    unsafe {
        let val = read_msr(msr);
        write_msr(msr, val & !bits);
    }
}

// ── SWAPGS ────────────────────────────────────────────────────────────────────

/// Swap GS_BASE avec KERNEL_GS_BASE (entrée SYSCALL / sortie vers userspace)
///
/// # SAFETY
/// Doit être exécuté en paire. Un SWAPGS sans son opposé corrompt le GS
/// et provoque un accès per-CPU vers une adresse invalide.
#[inline(always)]
pub unsafe fn swapgs() {
    // SAFETY: délégué à l'appelant — doit être utilisé en paire
    unsafe {
        core::arch::asm!("swapgs", options(nostack, nomem));
    }
}

// ── RDTSCP ───────────────────────────────────────────────────────────────────

/// Lit TSC + CPU ID logique atomiquement (RDTSCP)
///
/// Retourne `(tsc_value, cpu_id_aux)`.
/// Le cpu_id_aux est configuré par le noyau dans MSR_TSC_AUX.
///
/// # SAFETY
/// Nécessite CPUID.80000001H:EDX[27] = 1 (RDTSCP supporté).
/// Utiliser `CpuFeatures::has_rdtscp()` avant d'appeler.
#[inline(always)]
pub unsafe fn rdtscp() -> (u64, u32) {
    let lo: u32;
    let hi: u32;
    let aux: u32;
    // SAFETY: délégué à l'appelant — RDTSCP est une instruction sérialisante légère
    unsafe {
        core::arch::asm!(
            "rdtscp",
            out("eax") lo,
            out("edx") hi,
            out("ecx") aux,
            options(nostack, nomem)
        );
    }
    (((hi as u64) << 32) | (lo as u64), aux)
}

// ── PAT Configuration ────────────────────────────────────────────────────────

/// Configure la Page Attribute Table avec les valeurs par défaut Exo-OS
///
/// Index PAT :
///   0: WB  (Write-Back)  — défaut normal
///   1: WT  (Write-Through)
///   2: UC- (Uncacheable-minus, ignoré si MTRR=UC)
///   3: UC  (Uncacheable strict)
///   4: WB  (redondant — pour compatibilité)
///   5: WT
///   6: UC-
///   7: WC  (Write-Combining — pour framebuffer)
pub fn configure_pat() {
    // SAFETY: MSR_IA32_PAT est supporté sur tout CPU x86_64 moderne.
    // La valeur PAT_WC remplace l'entrée 7 par Write-Combining (0x01).
    const PAT_WB:  u64 = 0x06;
    const PAT_WT:  u64 = 0x04;
    const PAT_UCM: u64 = 0x07;
    const PAT_UC:  u64 = 0x00;
    const PAT_WC:  u64 = 0x01;

    let pat = PAT_WB
        | (PAT_WT  << 8)
        | (PAT_UCM << 16)
        | (PAT_UC  << 24)
        | (PAT_WB  << 32)
        | (PAT_WT  << 40)
        | (PAT_UCM << 48)
        | (PAT_WC  << 56);

    // SAFETY: PAT supporté sur tout x86_64; CPU-local, aucun tlb shootdown nécessaire.
    unsafe { write_msr(MSR_IA32_PAT, pat); }
}

// ── Statistiques MSR ─────────────────────────────────────────────────────────

static MSR_READ_COUNT:  core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static MSR_WRITE_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

#[cfg(feature = "msr_stats")]
#[inline(always)]
fn increment_read() {
    MSR_READ_COUNT.fetch_add(1, Ordering::Relaxed);
}

#[cfg(feature = "msr_stats")]
#[inline(always)]
fn increment_write() {
    MSR_WRITE_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Retourne `(reads, writes)` depuis le démarrage
pub fn msr_stats() -> (u64, u64) {
    (
        MSR_READ_COUNT.load(Ordering::Relaxed),
        MSR_WRITE_COUNT.load(Ordering::Relaxed),
    )
}
