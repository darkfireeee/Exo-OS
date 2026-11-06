// kernel/src/process/group/job_control.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// POSIX job control (tcsetpgrp / SIGTTIN / SIGTTOU) — Exo-OS
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

/// Controlling terminal: stores the PGID of the foreground group.
/// Only one controlling TTY per session.
#[repr(C)]
pub struct ControlTerminal {
    /// Foreground process group PGID.
    pub fg_pgid: AtomicU32,
    /// Terminal number (minor number).
    pub tty_dev: AtomicU32,
    /// SID elevating this terminal.
    pub owner_sid: AtomicU32,
}

impl Default for ControlTerminal {
    fn default() -> Self {
        Self::new()
    }
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

/// Controlling terminals table (at most one per session).
const MAX_CTTY: usize = 256;

struct CttyTable {
    slots: [ControlTerminal; MAX_CTTY],
}

unsafe impl Sync for CttyTable {}

impl CttyTable {
    const fn new() -> Self {
        Self {
            slots: [const { ControlTerminal::new() }; MAX_CTTY],
        }
    }

    fn find_by_sid(&self, sid: u32) -> Option<&ControlTerminal> {
        self.slots.iter().find(|slot| slot.owner_sid.load(Ordering::Acquire) == sid)
    }

    #[allow(dead_code)]
    fn find_free(&self) -> Option<&ControlTerminal> {
        self.slots.iter().find(|slot| slot.owner_sid.load(Ordering::Acquire) == 0)
    }
}

static CTTY_TABLE: CttyTable = CttyTable::new();

/// tcsetpgrp(fd, pgid): sets the foreground group of the TTY.
pub fn tcsetpgrp(caller_pid: Pid, pgid: PgId) -> Result<(), JobControlError> {
    use crate::process::core::registry::PROCESS_REGISTRY;
    let pcb = PROCESS_REGISTRY
        .find_by_pid(caller_pid)
        .ok_or(JobControlError::NotTerminal)?;
    let sid = pcb.session_id();

    // Find controlling terminal for this session.
    let ctty = CTTY_TABLE
        .find_by_sid(sid)
        .ok_or(JobControlError::NotTerminal)?;

    // Verify that the target group is in the same session.
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

/// tcgetpgrp(): returns the PGID of the foreground group.
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

/// Checks if `caller_pid` is in the foreground group.
/// If not and TOSTOP is active, sends SIGTTOU.
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
        None => return true, // No controlling TTY: OK
    };
    let fg = ctty.fg_pgid.load(Ordering::Acquire);
    let my_pgid = pcb.pgroup_id();
    if my_pgid != fg {
        // Background group: send SIGTTOU
        let _ = send_signal_to_pid(caller_pid, Signal::SIGTTOU);
        return false;
    }
    true
}

/// Checks if `caller_pid` can read from the controlling TTY.
/// If not, sends SIGTTIN.
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
