// kernel/src/process/state/transitions.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Machine à états du ProcessControlBlock — Exo-OS Couche 1.5
// ═══════════════════════════════════════════════════════════════════════════════
//
// Transitions valides :
//   Creating → Running
//   Creating → Zombie  (échec/exit pendant fork/exec)
//   Running  → Sleeping | Stopped | Zombie
//   Running  → Running (wake idempotent)
//   Sleeping → Running
//   Stopped  → Running (via SIGCONT ou wake)
//   Zombie   → Dead    (via reaper)
//   Dead     → (terminal)

use crate::process::core::pcb::{ProcessControlBlock, ProcessState};

/// Raison d'une transition.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StateTransition {
    /// Allocation terminée, thread prêt.
    Spawn,
    /// Thread mis en sommeil (attente E/S, mutex, etc.).
    Sleep,
    /// Thread réveillé (E/S terminée, mutex disponible).
    Wake,
    /// SIGSTOP reçu.
    Stop,
    /// SIGCONT reçu.
    Continue,
    /// do_exit() appelé ; en attente de récolte.
    ExitToZombie,
    /// Récolte terminée par le reaper.
    ZombieToDead,
}

/// Erreur de transition illégale.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TransitionError {
    pub from: ProcessState,
    pub transition: StateTransition,
}

/// Table des transitions valides.
/// Retourne l'état suivant ou Err si la transition est illégale.
pub fn transition(
    pcb: &ProcessControlBlock,
    tr: StateTransition,
) -> Result<ProcessState, TransitionError> {
    let current = pcb.state();
    let next = match (current, tr) {
        (ProcessState::Creating, StateTransition::Spawn) => ProcessState::Running,
        (ProcessState::Creating, StateTransition::ExitToZombie) => ProcessState::Zombie,
        (ProcessState::Running, StateTransition::Wake) => ProcessState::Running,
        (ProcessState::Running, StateTransition::Sleep) => ProcessState::Sleeping,
        (ProcessState::Running, StateTransition::Stop) => ProcessState::Stopped,
        (ProcessState::Running, StateTransition::ExitToZombie) => ProcessState::Zombie,
        (ProcessState::Sleeping, StateTransition::Wake) => ProcessState::Running,
        (ProcessState::Sleeping, StateTransition::Stop) => ProcessState::Stopped,
        (ProcessState::Sleeping, StateTransition::ExitToZombie) => ProcessState::Zombie,
        (ProcessState::Stopped, StateTransition::Continue) => ProcessState::Running,
        (ProcessState::Stopped, StateTransition::Wake) => ProcessState::Running,
        (ProcessState::Stopped, StateTransition::ExitToZombie) => ProcessState::Zombie,
        (ProcessState::Zombie, StateTransition::ZombieToDead) => ProcessState::Dead,
        _ => {
            return Err(TransitionError {
                from: current,
                transition: tr,
            })
        }
    };
    pcb.set_state(next);
    Ok(next)
}

#[inline]
pub fn is_valid_transition(from: ProcessState, tr: StateTransition) -> bool {
    matches!(
        (from, tr),
        (ProcessState::Creating, StateTransition::Spawn)
            | (ProcessState::Creating, StateTransition::ExitToZombie)
            | (ProcessState::Running, StateTransition::Wake)
            | (ProcessState::Running, StateTransition::Sleep)
            | (ProcessState::Running, StateTransition::Stop)
            | (ProcessState::Running, StateTransition::ExitToZombie)
            | (ProcessState::Sleeping, StateTransition::Wake)
            | (ProcessState::Sleeping, StateTransition::Stop)
            | (ProcessState::Sleeping, StateTransition::ExitToZombie)
            | (ProcessState::Stopped, StateTransition::Continue)
            | (ProcessState::Stopped, StateTransition::Wake)
            | (ProcessState::Stopped, StateTransition::ExitToZombie)
            | (ProcessState::Zombie, StateTransition::ZombieToDead)
    )
}
