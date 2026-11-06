// kernel/src/process/group/pgrp.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// POSIX process groups (PGID) — Exo-OS Layer 1.5
// ═══════════════════════════════════════════════════════════════════════════════

use crate::process::core::pid::Pid;
use crate::scheduler::sync::spinlock::SpinLock;
use core::sync::atomic::{AtomicU32, Ordering};

/// Process group identifier.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
#[repr(transparent)]
pub struct PgId(pub u32);

impl PgId {
    pub const KERNEL: Self = Self(0);
}

const MAX_PGROUPS: usize = 2048;

/// Process group descriptor.
#[repr(C)]
pub struct ProcessGroup {
    /// PGID = leader's PID.
    pub pgid: PgId,
    /// SID of the session containing this group.
    pub sid: AtomicU32,
    /// Number of living members.
    pub members: AtomicU32,
    /// Valid slot.
    pub valid: AtomicU32,
}

impl ProcessGroup {
    const fn empty() -> Self {
        Self {
            pgid: PgId(0),
            sid: AtomicU32::new(0),
            members: AtomicU32::new(0),
            valid: AtomicU32::new(0),
        }
    }
}

/// Global process groups table.
pub struct PGroupTable {
    slots: [ProcessGroup; MAX_PGROUPS],
    lock: SpinLock<()>,
    count: AtomicU32,
}

unsafe impl Sync for PGroupTable {}

impl PGroupTable {
    const fn new() -> Self {
        Self {
            slots: [const { ProcessGroup::empty() }; MAX_PGROUPS],
            lock: SpinLock::new(()),
            count: AtomicU32::new(0),
        }
    }

    /// Creates or joins a process group.
    pub fn create_or_join(&self, pgid: PgId, sid: u32) -> bool {
        let _guard = self.lock.lock();
        // Check if group already exists.
        for slot in &self.slots {
            if slot.valid.load(Ordering::Acquire) == 1 && slot.pgid == pgid {
                slot.members.fetch_add(1, Ordering::AcqRel);
                return true;
            }
        }
        // Create a new group.
        for slot in &self.slots {
            if slot.valid.load(Ordering::Acquire) == 0 {
                // SAFETY: exclusive write_lock; free slot (valid==0), unique mutation.
                unsafe {
                    (*(slot as *const ProcessGroup as *mut ProcessGroup)).pgid = pgid;
                }
                slot.sid.store(sid, Ordering::Release);
                slot.members.store(1, Ordering::Release);
                slot.valid.store(1, Ordering::Release);
                self.count.fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    /// Leaves a group.
    pub fn leave(&self, pgid: PgId) {
        let _guard = self.lock.lock();
        for slot in &self.slots {
            if slot.valid.load(Ordering::Acquire) == 1 && slot.pgid == pgid {
                let m = slot.members.fetch_sub(1, Ordering::AcqRel);
                if m == 1 {
                    slot.valid.store(0, Ordering::Release);
                    self.count.fetch_sub(1, Ordering::Relaxed);
                }
                return;
            }
        }
    }

    /// Finds a group by PGID.
    pub fn find(&self, pgid: PgId) -> Option<&ProcessGroup> {
        self.slots
            .iter()
            .find(|slot| slot.valid.load(Ordering::Acquire) == 1 && slot.pgid == pgid)
    }

    /// Sends a signal to all members of a group.
    pub fn signal_group(&self, pgid: PgId, sig: crate::process::signal::Signal) {
        use crate::process::core::registry::PROCESS_REGISTRY;
        PROCESS_REGISTRY.for_each(|pcb| {
            let proc_pgid = pcb.pgroup_id();
            if proc_pgid == pgid.0 {
                let pid = pcb.pid();
                let _ = crate::process::signal::delivery::send_signal_to_pid(pid, sig);
            }
        });
    }

    pub fn active_count(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }
}

/// Global process groups table.
pub static PGROUP_TABLE: PGroupTable = PGroupTable::new();

/// setpgid(2): places process `pid` into group `pgid`.
pub fn setpgid(pid: Pid, pgid: PgId) -> Result<(), PgidError> {
    use crate::process::core::registry::PROCESS_REGISTRY;
    let pcb = PROCESS_REGISTRY
        .find_by_pid(pid)
        .ok_or(PgidError::NoSuchProcess)?;
    // Do not change group of a session leader.
    if pcb.is_session_leader() {
        return Err(PgidError::SessionLeader);
    }
    let sid = pcb.session_id();
    let old_pgid = PgId(pcb.pgroup_id());
    let new_pgid = if pgid.0 == 0 { PgId(pid.0) } else { pgid };
    PGROUP_TABLE.leave(old_pgid);
    if !PGROUP_TABLE.create_or_join(new_pgid, sid) {
        return Err(PgidError::TooManyGroups);
    }
    pcb.set_pgroup_id(new_pgid.0);
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PgidError {
    NoSuchProcess,
    SessionLeader,
    TooManyGroups,
}
