// kernel/src/scheduler/policies/deadline.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Politique SCHED_DEADLINE — EDF (Earliest Deadline First)
// ═══════════════════════════════════════════════════════════════════════════════
//
// SCHED_DEADLINE est une politique temps-réel stricte basée sur EDF.
// Chaque thread paramétré précise :
//   runtime_ns  — budget d'exécution par période
//   deadline_ns — échéance relative depuis l'activation
//   period_ns   — période de réactivation
//
// Test d'admission : Σ(runtime_i / period_i) ≤ 1.0
// Si le test échoue, `admit_thread()` retourne `Err(AdmissionDenied)`.
//
// Schedulability : à chaque wakeup, `deadline_abs = now + deadline_ns`.
// La run queue EDF trie par `deadline_abs` croissant.
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::core::task::{ThreadControlBlock, DeadlineParams, SchedPolicy};
use crate::scheduler::timer::clock::monotonic_ns;

// ─────────────────────────────────────────────────────────────────────────────
// Comptabilité d'admission
// ─────────────────────────────────────────────────────────────────────────────

/// Numérateur de la fraction utilisée = Σ runtime_i × SCALE / period_i.
/// SCALE = 1_000_000 pour éviter les flottants.
const ADMIT_SCALE: u64 = 1_000_000;

/// Utilisation cumulée × SCALE (max = ADMIT_SCALE = 100%).
static ADMITTED_UTILIZATION: AtomicU64 = AtomicU64::new(0);
/// Nombre de threads DEADLINE admis.
pub static DEADLINE_THREADS_ADMITTED: AtomicU64 = AtomicU64::new(0);
/// Nombre de refus d'admission.
pub static DEADLINE_ADMISSION_DENIED: AtomicU64 = AtomicU64::new(0);
/// Expirations de deadline détectées.
pub static DEADLINE_MISSES: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Erreur d'admission
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadlineError {
    /// Test d'admission échoué (surcharge).
    AdmissionDenied,
    /// Paramètres invalides (period=0, deadline>period, runtime>deadline).
    InvalidParams,
    /// Manque de capacité dans la run queue EDF.
    QueueFull,
}

// ─────────────────────────────────────────────────────────────────────────────
// Validation des paramètres
// ─────────────────────────────────────────────────────────────────────────────

fn validate_params(p: &DeadlineParams) -> Result<(), DeadlineError> {
    if p.period_ns == 0 { return Err(DeadlineError::InvalidParams); }
    if p.deadline_ns > p.period_ns { return Err(DeadlineError::InvalidParams); }
    if p.runtime_ns == 0 || p.runtime_ns > p.deadline_ns {
        return Err(DeadlineError::InvalidParams);
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Test d'admission
// ─────────────────────────────────────────────────────────────────────────────

/// Tente d'admettre un nouveau thread DEADLINE.
///
/// Effectue `Σ(runtime/period) ≤ 1.0` via CAS sur `ADMITTED_UTILIZATION`.
/// Retourne `Ok(fraction)` (fraction = runtime/period × SCALE) si admis,
/// ou `Err(AdmissionDenied)` si la charge dépasse la capacité.
pub fn admit_thread(p: &DeadlineParams) -> Result<u64, DeadlineError> {
    validate_params(p)?;

    // fraction = runtime_ns × SCALE / period_ns
    let fraction = p.runtime_ns.saturating_mul(ADMIT_SCALE) / p.period_ns;

    // CAS loop pour mise à jour atomique.
    loop {
        let current = ADMITTED_UTILIZATION.load(Ordering::Acquire);
        let new_util = current.saturating_add(fraction);
        if new_util > ADMIT_SCALE {
            DEADLINE_ADMISSION_DENIED.fetch_add(1, Ordering::Relaxed);
            return Err(DeadlineError::AdmissionDenied);
        }
        if ADMITTED_UTILIZATION
            .compare_exchange(current, new_util, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            DEADLINE_THREADS_ADMITTED.fetch_add(1, Ordering::Relaxed);
            return Ok(fraction);
        }
        // Contention CAS — réessai.
        core::hint::spin_loop();
    }
}

/// Libère la fraction d'utilisation lors de la terminaison/descheduling du thread.
///
/// # BUG-FIX E : utiliser fetch_update + saturating_sub pour éviter l'underflow
/// silencieux sur u64. L'ancien `fetch_sub` pouvait faire passer
/// ADMITTED_UTILIZATION et DEADLINE_THREADS_ADMITTED à u64::MAX≈1.8×10¹⁹ si
/// `release_thread()` était appelé en excès (bug de double-libération).
pub fn release_thread(fraction: u64) {
    let _ = ADMITTED_UTILIZATION.fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
        Some(v.saturating_sub(fraction))
    });
    let _ = DEADLINE_THREADS_ADMITTED.fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
        Some(v.saturating_sub(1))
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Gestion de l'échéance absolue
// ─────────────────────────────────────────────────────────────────────────────

/// Recalcule `deadline_abs` au réveil d'un thread DEADLINE.
///
/// EDF : upon activation, deadline_abs = now + deadline_params.deadline_ns
pub fn refresh_deadline(tcb: &mut ThreadControlBlock) {
    let now = monotonic_ns();
    let deadline_ns = tcb.deadline_params.deadline_ns;
    tcb.deadline_abs.store(now.saturating_add(deadline_ns), Ordering::Release);
}

/// Vérifie si le thread a manqué son échéance.
/// Incrémente `DEADLINE_MISSES` et retourne `true` si c'est le cas.
pub fn check_deadline_miss(tcb: &ThreadControlBlock) -> bool {
    let now = monotonic_ns();
    let deadline_abs = tcb.deadline_abs.load(Ordering::Relaxed);
    if now > deadline_abs && deadline_abs != 0 {
        DEADLINE_MISSES.fetch_add(1, Ordering::Relaxed);
        return true;
    }
    false
}

/// Retourne le temps restant (en ns) avant l'échéance absolue du thread.
/// Retourne 0 si l'échéance est déjà dépassée ou non initialisée.
pub fn remaining_budget(tcb: &ThreadControlBlock) -> u64 {
    let now          = monotonic_ns();
    let deadline_abs = tcb.deadline_abs.load(Ordering::Relaxed);
    if deadline_abs == 0 || now >= deadline_abs { 0 }
    else { deadline_abs - now }
}

/// Appelé lors du tick timer pour un thread DEADLINE.
/// Décrémente le budget restant, retourne `true` si le budget est épuisé
/// (le thread doit se bloquer jusqu'à sa prochaine période).
pub fn deadline_tick(tcb: &ThreadControlBlock, elapsed_ns: u64) -> bool {
    if tcb.policy != SchedPolicy::Deadline { return false; }
    let budget = remaining_budget(tcb);
    elapsed_ns >= budget
}
