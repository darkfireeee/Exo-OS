// kernel/src/process/group/job_control.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Contrôle de tache POSIX (tcsetpgrp / SIGTTIN / SIGTTOU) — Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════

use super::pgrp::PgId;
use crate::process::core::pid::Pid;
use core::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobControlError {
    NotSessionLeader,
    NotSameSession,
    NoSuchGroup,
    NotTerminal,
}

/// Terminal de contrôle : stocke le PGID du groupe au premier plan.
/// Un seul TTY de contrôle par session.
#[repr(C)]
pub struct ControlTerminal {
    /// PGID du groupe de processus au premier plan.
    pub fg_pgid: AtomicU32,
    /// Numéro du terminal (minor number).
    pub tty_dev: AtomicU32,
    /// SID qui élève ce terminal.
    pub owner_sid: AtomicU32,
}

impl ControlTerminal {
    pub const fn new() -> Self {
        Self {
            fg_pgid: AtomicU32::new(0),
            tty_dev: AtomicU32::new(0),
            owner_sid: AtomicU32::new(0),
        }
    }
}

/// Table des terminaux de contrôle (un par session au plus).
const MAX_CTTY: usize = 256;

struct CttyTable {
    slots: [ControlTerminal; MAX_CTTY],
}

unsafe impl Sync for CttyTable {}

impl CttyTable {
    const fn new() -> Self {
        const EMPTY: ControlTerminal = ControlTerminal::new();
        Self {
            slots: [EMPTY; MAX_CTTY],
        }
    }

    fn find_by_sid(&self, sid: u32) -> Option<&ControlTerminal> {
        for slot in &self.slots {
            if slot.owner_sid.load(Ordering::Acquire) == sid {
                return Some(slot);
            }
        }
        None
    }

    #[allow(dead_code)]
    fn find_free(&self) -> Option<&ControlTerminal> {
        for slot in &self.slots {
            if slot.owner_sid.load(Ordering::Acquire) == 0 {
                return Some(slot);
            }
        }
        None
    }
}

static CTTY_TABLE: CttyTable = CttyTable::new();

/// tcsetpgrp(fd, pgid) : définit le groupe de premier plan du TTY.
pub fn tcsetpgrp(caller_pid: Pid, pgid: PgId) -> Result<(), JobControlError> {
    use crate::process::core::registry::PROCESS_REGISTRY;
    let pcb = PROCESS_REGISTRY
        .find_by_pid(caller_pid)
        .ok_or(JobControlError::NotTerminal)?;
    let sid = pcb.session_id();

    // Trouver le terminal de contrôle de cette session.
    let ctty = CTTY_TABLE
        .find_by_sid(sid)
        .ok_or(JobControlError::NotTerminal)?;

    // Vérifier que le groupe cible est dans la même session.
    use super::pgrp::PGROUP_TABLE;
    let pgrp = PGROUP_TABLE
        .find(pgid)
        .ok_or(JobControlError::NoSuchGroup)?;
    if pgrp.sid.load(Ordering::Acquire) != sid {
        return Err(JobControlError::NotSameSession);
    }

    ctty.fg_pgid.store(pgid.0, Ordering::Release);
    Ok(())
}

/// tcgetpgrp() : retourne le PGID du groupe de premier plan.
pub fn tcgetpgrp(caller_pid: Pid) -> Result<PgId, JobControlError> {
    use crate::process::core::registry::PROCESS_REGISTRY;
    let pcb = PROCESS_REGISTRY
        .find_by_pid(caller_pid)
        .ok_or(JobControlError::NotTerminal)?;
    let sid = pcb.session_id();
    let ctty = CTTY_TABLE
        .find_by_sid(sid)
        .ok_or(JobControlError::NotTerminal)?;
    Ok(PgId(ctty.fg_pgid.load(Ordering::Acquire)))
}

/// Vérifie si `caller_pid` est dans le groupe de premier plan.
/// Si non et que TOSTOP est actif, envoie SIGTTOU.
pub fn check_tty_output(caller_pid: Pid) -> bool {
    use crate::process::core::registry::PROCESS_REGISTRY;
    use crate::process::signal::delivery::send_signal_to_pid;
    use crate::process::signal::Signal;

    let pcb = match PROCESS_REGISTRY.find_by_pid(caller_pid) {
        Some(p) => p,
        None => return true,
    };
    let sid = pcb.session_id();
    let ctty = match CTTY_TABLE.find_by_sid(sid) {
        Some(c) => c,
        None => return true, // Pas de TTY de contrôle : OK
    };
    let fg = ctty.fg_pgid.load(Ordering::Acquire);
    let my_pgid = pcb.pgroup_id();
    if my_pgid != fg {
        // Groupe d'arrière-plan : envoyer SIGTTOU
        let _ = send_signal_to_pid(caller_pid, Signal::SIGTTOU);
        return false;
    }
    true
}

/// Vérifie si `caller_pid` peut lire depuis le TTY de contrôle.
/// Si non, envoie SIGTTIN.
pub fn check_tty_input(caller_pid: Pid) -> bool {
    use crate::process::core::registry::PROCESS_REGISTRY;
    use crate::process::signal::delivery::send_signal_to_pid;
    use crate::process::signal::Signal;

    let pcb = match PROCESS_REGISTRY.find_by_pid(caller_pid) {
        Some(p) => p,
        None => return true,
    };
    let sid = pcb.session_id();
    let ctty = match CTTY_TABLE.find_by_sid(sid) {
        Some(c) => c,
        None => return true,
    };
    let fg = ctty.fg_pgid.load(Ordering::Acquire);
    let my_pgid = pcb.pgroup_id();
    if my_pgid != fg {
        let _ = send_signal_to_pid(caller_pid, Signal::SIGTTIN);
        return false;
    }
    true
}
