// kernel/src/process/group/session.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// POSIX sessions — Exo-OS Layer 1.5
// ═══════════════════════════════════════════════════════════════════════════════

use crate::process::core::pid::Pid;
use crate::scheduler::sync::spinlock::SpinLock;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Session identifier.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
#[repr(transparent)]
pub struct SessionId(pub u32);

impl SessionId {
    pub const KERNEL: Self = Self(0);
}

/// Maximum number of simultaneous sessions.
const MAX_SESSIONS: usize = 1024;

/// POSIX session descriptor.
#[repr(C)]
pub struct Session {
    /// SID = session leader PID.
    pub sid: SessionId,
    /// Session leader PID.
    pub leader_pid: AtomicU32,
    /// Associated controlling terminal (0 = none).
    pub ctty: AtomicU64,
    /// Reference count.
    pub refcount: AtomicU32,
    /// Valid slot.
    pub valid: AtomicU32,
}

impl Session {
    const fn empty() -> Self {
        Self {
            sid: SessionId(0),
            leader_pid: AtomicU32::new(0),
            ctty: AtomicU64::new(0),
            refcount: AtomicU32::new(0),
            valid: AtomicU32::new(0),
        }
    }
}

/// Global session table (static array).
pub struct SessionTable {
    slots: [Session; MAX_SESSIONS],
    lock: SpinLock<()>,
    count: AtomicU32,
}

// SAFETY: Session contains only atomics + SpinLock, serialized access.
unsafe impl Sync for SessionTable {}

impl Default for SessionTable {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionTable {
    const fn new() -> Self {
        Self {
            slots: [const { Session::empty() }; MAX_SESSIONS],
            lock: SpinLock::new(()),
            count: AtomicU32::new(0),
        }
    }

    /// Creates a new session with `leader` as leader.
    pub fn create(&self, leader: Pid) -> Option<SessionId> {
        let _guard = self.lock.lock();
        for slot in &self.slots {
            if slot.valid.load(Ordering::Acquire) == 0 {
                // SAFETY: exclusive write_lock; free slot (valid==0), unique access.
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
        None // Max sessions reached
    }

    /// Finds a session by SID.
    pub fn find(&self, sid: SessionId) -> Option<&Session> {
        self.slots
            .iter()
            .find(|slot| slot.valid.load(Ordering::Acquire) == 1 && slot.sid == sid)
    }

    /// Releases a session when its refcount reaches 0.
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

    pub fn active_count(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }
}

/// Global session table.
pub static SESSION_TABLE: SessionTable = SessionTable::new();

/// setsid(2): creates a new session for current process.
pub fn setsid(caller_pid: Pid) -> Result<SessionId, SidError> {
    // Verify caller is not already group leader.
    use crate::process::core::registry::PROCESS_REGISTRY;
    let pcb = PROCESS_REGISTRY
        .find_by_pid(caller_pid)
        .ok_or(SidError::NoSuchProcess)?;
    if pcb.is_pgroup_leader() {
        return Err(SidError::AlreadyLeader);
    }
    let sid = SESSION_TABLE
        .create(caller_pid)
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
