// kernel/src/scheduler/energy/frequency.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Gestion de la fréquence CPU — P-states et DVFS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le scheduling RT ajuste le budget de temps en tenant compte de la fréquence
// courante : un thread dont l'exécution doit terminer en 1ms à 3GHz
// a besoin de 1.5ms à 2GHz.
//
// Il y a jusqu'à MAX_PSTATES niveaux de fréquence disponibles, numérotés P0
// (liaison max) à Pn (minimum).
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::smp::topology::MAX_CPUS;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const MAX_PSTATES: usize = 16;

// ─────────────────────────────────────────────────────────────────────────────
// Table des P-states (fréquences nominales en MHz)
// ─────────────────────────────────────────────────────────────────────────────

static mut PSTATE_TABLE: [u32; MAX_PSTATES] = [0u32; MAX_PSTATES];
static PSTATE_COUNT: AtomicU32 = AtomicU32::new(0);

/// Enregistre la table des P-states disponibles (fréquences en MHz, du plus haut
/// au plus bas). Appelé depuis la plateforme au boot.
///
/// # Safety
/// Appelé une seule fois avant toute utilisation de la fréquence.
pub unsafe fn set_pstate_table(freqs_mhz: &[u32]) {
    let n = freqs_mhz.len().min(MAX_PSTATES);
    for i in 0..n {
        PSTATE_TABLE[i] = freqs_mhz[i];
    }
    PSTATE_COUNT.store(n as u32, Ordering::Release);
}

/// Fréquence du P-state `p` en MHz. Retourne 0 si hors bornes.
pub fn pstate_freq_mhz(p: usize) -> u32 {
    let n = PSTATE_COUNT.load(Ordering::Relaxed) as usize;
    if p < n {
        unsafe { PSTATE_TABLE[p] }
    } else {
        0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// P-state courant par CPU
// ─────────────────────────────────────────────────────────────────────────────

/// P-state courant de chaque CPU (index dans PSTATE_TABLE ; 0 = plus haute fréquence).
static CURRENT_PSTATE: [AtomicU32; MAX_CPUS] = {
    const INIT: AtomicU32 = AtomicU32::new(0);
    [INIT; MAX_CPUS]
};

pub fn current_pstate(cpu: usize) -> u32 {
    if cpu < MAX_CPUS {
        CURRENT_PSTATE[cpu].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// Retourne la fréquence courante du CPU `cpu` en MHz.
pub fn current_freq_mhz(cpu: usize) -> u32 {
    pstate_freq_mhz(current_pstate(cpu) as usize)
}

// ─────────────────────────────────────────────────────────────────────────────
// FFI vers arch pour changer le P-state
// ─────────────────────────────────────────────────────────────────────────────

extern "C" {
    fn arch_set_cpu_pstate(cpu: u32, pstate: u32);
}

/// Demande le passage du CPU `cpu` au P-state `p`.
///
/// # Safety
/// Appelé avec préemption désactivée.
pub unsafe fn set_pstate(cpu: usize, p: u32) {
    if cpu >= MAX_CPUS {
        return;
    }
    let n = PSTATE_COUNT.load(Ordering::Relaxed);
    let p = p.min(n.saturating_sub(1));
    CURRENT_PSTATE[cpu].store(p, Ordering::Relaxed);
    arch_set_cpu_pstate(cpu as u32, p);
}

// ─────────────────────────────────────────────────────────────────────────────
// Correction du budget RT selon la fréquence
// ─────────────────────────────────────────────────────────────────────────────

/// Ajuste un budget en ns défini à la fréquence maximale (P0) vers la fréquence
/// courante du CPU `cpu`.
///
/// budget_at_current_freq = budget_at_max × (freq_max / freq_current)
/// (avec freq_max = PSTATE_TABLE[0])
pub fn scale_budget_ns(budget_ns: u64, cpu: usize) -> u64 {
    let freq_max = pstate_freq_mhz(0);
    let freq_cur = current_freq_mhz(cpu);
    if freq_cur == 0 || freq_max == 0 {
        return budget_ns;
    }
    // Scaling integer-safe : budget × freq_max / freq_cur
    ((budget_ns as u128 * freq_max as u128) / freq_cur as u128) as u64
}

pub static PSTATE_CHANGES: AtomicU64 = AtomicU64::new(0);
