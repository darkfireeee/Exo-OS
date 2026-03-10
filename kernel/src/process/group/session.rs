// kernel/src/process/group/session.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Sessions POSIX — Exo-OS Couche 1.5
// ═══════════════════════════════════════════════════════════════════════════════


use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::process::core::pid::Pid;

/// Identifiant de session.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
#[repr(transparent)]
pub struct SessionId(pub u32);

impl SessionId {
    pub const KERNEL: Self = Self(0);
}

/// Nombre maximum de sessions simultanées.
const MAX_SESSIONS: usize = 1024;

/// Descripteur d'une session POSIX.
#[repr(C)]
pub struct Session {
    /// SID = PID du leader de session.
    pub sid:        SessionId,
    /// PID du leader de session.
    pub leader_pid: AtomicU32,
    /// Terminal de contrôle associé (0 = aucun).
    pub ctty:       AtomicU64,
    /// Numéro de références.
    pub refcount:   AtomicU32,
    /// Slot valid.
    pub valid:      AtomicU32,
}

impl Session {
    const fn empty() -> Self {
        Self {
            sid:        SessionId(0),
            leader_pid: AtomicU32::new(0),
            ctty:       AtomicU64::new(0),
            refcount:   AtomicU32::new(0),
            valid:      AtomicU32::new(0),
        }
    }
}

/// Table globale des sessions (tableau statique).
pub struct SessionTable {
    slots: [Session; MAX_SESSIONS],
    lock:  SpinLock<()>,
    count: AtomicU32,
}

// SAFETY : Session ne contient que des atomiques + SpinLock, accès serialisé.
unsafe impl Sync for SessionTable {}

impl SessionTable {
    const fn new() -> Self {
        const EMPTY: Session = Session::empty();
        Self {
            slots: [EMPTY; MAX_SESSIONS],
            lock:  SpinLock::new(()),
            count: AtomicU32::new(0),
        }
    }

    /// Crée une nouvelle session avec `leader` comme leader.
    pub fn create(&self, leader: Pid) -> Option<SessionId> {
        let _guard = self.lock.lock();
        for slot in &self.slots {
            if slot.valid.load(Ordering::Acquire) == 0 {
                // SAFETY: write_lock exclusif; slot libre (valid==0), accès unique.
                unsafe {
                    (*(slot as *const Session as *mut Session)).sid = SessionId(leader.0);
                }
                slot.leader_pid.store(leader.0, Ordering::Release);
                slot.ctty.store(0, Ordering::Release);
                slot.refcount.store(1, Ordering::Release);
                slot.valid.store(1, Ordering::Release);
                self.count.fetch_add(1, Ordering::Relaxed);
                return Some(SessionId(leader.0));
            }
        }
        None // Max sessions atteint
    }

    /// Cherche une session par SID.
    pub fn find(&self, sid: SessionId) -> Option<&Session> {
        for slot in &self.slots {
            if slot.valid.load(Ordering::Acquire) == 1
               && slot.sid == sid
            {
                return Some(slot);
            }
        }
        None
    }

    /// Supprime une session quand son refcount atteint 0.
    pub fn release(&self, sid: SessionId) {
        let _guard = self.lock.lock();
        for slot in &self.slots {
            if slot.valid.load(Ordering::Acquire) == 1 && slot.sid == sid {
                let rc = slot.refcount.fetch_sub(1, Ordering::AcqRel);
                if rc == 1 {
                    slot.valid.store(0, Ordering::Release);
                    self.count.fetch_sub(1, Ordering::Relaxed);
                }
                return;
            }
        }
    }

    pub fn active_count(&self) -> u32 { self.count.load(Ordering::Relaxed) }
}

/// Table globale des sessions.
pub static SESSION_TABLE: SessionTable = SessionTable::new();

/// setsid(2) : crée une nouvelle session pour le processus courant.
pub fn setsid(caller_pid: Pid) -> Result<SessionId, SidError> {
    // Vérifier que le caller n'est pas déjà leader de groupe.
    use crate::process::core::registry::PROCESS_REGISTRY;
    let pcb = PROCESS_REGISTRY.find_by_pid(caller_pid)
        .ok_or(SidError::NoSuchProcess)?;
    if pcb.is_pgroup_leader() {
        return Err(SidError::AlreadyLeader);
    }
    let sid = SESSION_TABLE.create(caller_pid)
        .ok_or(SidError::TooManySessions)?;
    pcb.set_session_id(sid.0);
    pcb.set_pgroup_id(caller_pid.0);
    Ok(sid)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidError {
    NoSuchProcess,
    AlreadyLeader,
    TooManySessions,
}
