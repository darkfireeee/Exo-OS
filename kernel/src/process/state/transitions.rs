// kernel/src/process/state/transitions.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ProcessControlBlock state machine — Exo-OS Layer 1.5
// ═══════════════════════════════════════════════════════════════════════════════
//
// Valid transitions:
//   Creating -> Running
//   Creating -> Zombie  (failure/exit during fork/exec)
//   Running  -> Sleeping | Stopped | Zombie
//   Running  -> Running (idempotent wake)
//   Sleeping -> Running
//   Stopped  -> Running (via SIGCONT or wake)
//   Zombie   -> Dead    (via reaper)
//   Dead     -> (terminal)

use crate::process::core::pcb::{ProcessControlBlock, ProcessState};

/// Reason for a transition.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StateTransition {
    /// Allocation complete, thread ready.
    Spawn,
    /// Thread put to sleep (waiting for I/O, mutex, etc.).
    Sleep,
    /// Thread awakened (I/O complete, mutex available).
    Wake,
    /// SIGSTOP received.
    Stop,
    /// SIGCONT received.
    Continue,
    /// do_exit() called; awaiting reaping.
    ExitToZombie,
    /// Reaping completed by the reaper.
    ZombieToDead,
}

/// Illegal transition error.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TransitionError {
    pub from: ProcessState,
    pub transition: StateTransition,
}

/// Valid transition table.
/// Returns the next state or Err if the transition is illegal.
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
