// kernel/src/scheduler/energy/c_states.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// C-States — gestion des états de veille CPU
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE CSTATE-01 : fetch_min pour la contrainte RT.
//   Lorsqu'un thread RT est actif, le C-state autorisé ne peut dépasser C1.
//   La mise à jour utilise `fetch_min` pour garantir que le C-state le plus
//   restrictif est toujours respecté.
//
// C-states supportés :
//   C0 = actif (aucune action)
//   C1 = HLT (quelques microsecondes de latence de réveil)
//   C2 = MWAIT + hint (dizaines de µs)
//   C3 = deep sleep (centaines de µs — interdit en présence de threads RT)
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU8, Ordering};
use crate::scheduler::smp::topology::MAX_CPUS;

// ─────────────────────────────────────────────────────────────────────────────
// Niveaux C-state
// ─────────────────────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CState {
    C0 = 0,   // Actif
    C1 = 1,   // HLT
    C2 = 2,   // MWAIT/light
    C3 = 3,   // Deep sleep
}

impl CState {
    /// Latence de sortie estimée (ns).
    pub fn exit_latency_ns(self) -> u64 {
        match self {
            CState::C0 => 0,
            CState::C1 => 1_000,        // 1µs
            CState::C2 => 10_000,       // 10µs
            CState::C3 => 200_000,      // 200µs
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Contrainte C-state par CPU (fetch_min — RÈGLE CSTATE-01)
// ─────────────────────────────────────────────────────────────────────────────

/// Contrainte maximale de C-state par CPU (0 = aucune contrainte = C3 autorisé).
/// Un thread RT impose C1 en utilisant `fetch_min(C1 as u8)`.
static CSTATE_MAX: [AtomicU8; MAX_CPUS] = {
    const INIT: AtomicU8 = AtomicU8::new(CState::C3 as u8);
    [INIT; MAX_CPUS]
};

/// Applique la contrainte RT : force C1 maximum sur le CPU `cpu`.
/// Utilise `fetch_min` pour respecter RÈGLE CSTATE-01.
pub fn constrain_rt(cpu: usize) {
    if cpu < MAX_CPUS {
        CSTATE_MAX[cpu].fetch_min(CState::C1 as u8, Ordering::AcqRel);
    }
}

/// Relâche la contrainte RT sur le CPU `cpu`.
/// Appelé quand le dernier thread RT quitte le CPU.
pub fn release_rt_constraint(cpu: usize) {
    if cpu < MAX_CPUS {
        CSTATE_MAX[cpu].store(CState::C3 as u8, Ordering::Release);
    }
}

/// Retourne le C-state maximum autorisé sur le CPU `cpu`.
pub fn max_allowed_cstate(cpu: usize) -> CState {
    if cpu >= MAX_CPUS { return CState::C1; }
    match CSTATE_MAX[cpu].load(Ordering::Acquire) {
        0 => CState::C0,
        1 => CState::C1,
        2 => CState::C2,
        _ => CState::C3,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sélection du C-state optimal
// ─────────────────────────────────────────────────────────────────────────────

/// Sélectionne le C-state optimal pour le CPU `cpu` selon le temps d'inactivité
/// prévu `idle_ns`.
///
/// Règle : entrer dans un C-state uniquement si le bénéfice énergétique compense
/// la latence de sortie (point mort = exit_latency × overhead_factor).
pub fn select_cstate(cpu: usize, idle_ns: u64) -> CState {
    let max = max_allowed_cstate(cpu);
    let candidates = [CState::C3, CState::C2, CState::C1, CState::C0];
    for cs in candidates {
        if cs > max { continue; }
        // Entrer dans ce C-state uniquement si on économise au moins 2× la latence de sortie.
        let threshold = cs.exit_latency_ns().saturating_mul(2);
        if idle_ns >= threshold {
            return cs;
        }
    }
    CState::C0
}

// ─────────────────────────────────────────────────────────────────────────────
// Entrée dans le C-state
// ─────────────────────────────────────────────────────────────────────────────

/// Entre dans le C-state demandé sur le CPU courant.
///
/// # Safety
/// IRQ doivent être activées avant l'instruction HLT/MWAIT.
pub unsafe fn enter_cstate(cs: CState) {
    match cs {
        CState::C0 => {}
        CState::C1 => {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
        CState::C2 | CState::C3 => {
            // MWAIT avec hint approprié.
            let hint: u32 = match cs {
                CState::C2 => 0x10, // C2 sub-state 0
                CState::C3 => 0x20, // C3 sub-state 0
                _           => 0x00,
            };
            // MONITOR/MWAIT : surveille une zone mémoire.
            // En pratique, on surveille get_current_tcb().need_resched.
            core::arch::asm!(
                "monitor",       // monitore [rax]
                "mwait",         // attend événement ou MONITOR range
                in("eax") 0u32,  // adresse MONITOR (simplifié)
                in("ecx") 0u32,  // hints extension = 0
                in("edx") hint,  // hints
                options(nomem, nostack, preserves_flags),
            );
        }
    }
}

/// Initialise le sous-système C-states pour `nr_cpus` CPUs.
///
/// # Safety
/// Appelé une seule fois depuis scheduler::init().
pub unsafe fn init(nr_cpus: usize) {
    for cpu in 0..nr_cpus.min(MAX_CPUS) {
        CSTATE_MAX[cpu].store(CState::C3 as u8, Ordering::Relaxed);
    }
}
