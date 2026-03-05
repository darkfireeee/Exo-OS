// kernel/src/process/group/pgrp.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Groupes de processus POSIX (PGID) — Exo-OS Couche 1.5
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicU32, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::process::core::pid::Pid;

/// Identifiant de groupe de processus.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
#[repr(transparent)]
pub struct PgId(pub u32);

impl PgId {
    pub const KERNEL: Self = Self(0);
}

const MAX_PGROUPS: usize = 2048;

/// Descripteur d'un groupe de processus.
#[repr(C)]
pub struct ProcessGroup {
    /// PGID = PID du leader.
    pub pgid:       PgId,
    /// SID de la session contenant ce groupe.
    pub sid:        AtomicU32,
    /// Nombre de membres vivants.
    pub members:    AtomicU32,
    /// slot valide.
    pub valid:      AtomicU32,
}

impl ProcessGroup {
    const fn empty() -> Self {
        Self {
            pgid:    PgId(0),
            sid:     AtomicU32::new(0),
            members: AtomicU32::new(0),
            valid:   AtomicU32::new(0),
        }
    }
}

/// Table globale des groupes de processus.
pub struct PGroupTable {
    slots: [ProcessGroup; MAX_PGROUPS],
    lock:  SpinLock<()>,
    count: AtomicU32,
}

unsafe impl Sync for PGroupTable {}

impl PGroupTable {
    const fn new() -> Self {
        const EMPTY: ProcessGroup = ProcessGroup::empty();
        Self {
            slots: [EMPTY; MAX_PGROUPS],
            lock:  SpinLock::new(()),
            count: AtomicU32::new(0),
        }
    }

    /// Crée ou rejoint un groupe de processus.
    pub fn create_or_join(
        &self,
        pgid: PgId,
        sid:  u32,
    ) -> bool {
        let _guard = self.lock.lock();
        // Cherche si le groupe existe déjà.
        for slot in &self.slots {
            if slot.valid.load(Ordering::Acquire) == 1 && slot.pgid == pgid {
                slot.members.fetch_add(1, Ordering::AcqRel);
                return true;
            }
        }
        // Créer un nouveau groupe.
        for slot in &self.slots {
            if slot.valid.load(Ordering::Acquire) == 0 {
                // SAFETY: write_lock exclusif; slot libre (valid==0), mutation unique.
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

    /// Quitte un groupe.
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

    /// Cherche un groupe par PGID.
    pub fn find(&self, pgid: PgId) -> Option<&ProcessGroup> {
        for slot in &self.slots {
            if slot.valid.load(Ordering::Acquire) == 1 && slot.pgid == pgid {
                return Some(slot);
            }
        }
        None
    }

    /// Envoie un signal à tous les membres d'un groupe.
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

    pub fn active_count(&self) -> u32 { self.count.load(Ordering::Relaxed) }
}

/// Table globale des groupes de processus.
pub static PGROUP_TABLE: PGroupTable = PGroupTable::new();

/// setpgid(2) : place le processus `pid` dans le groupe `pgid`.
pub fn setpgid(
    pid:  Pid,
    pgid: PgId,
) -> Result<(), PgidError> {
    use crate::process::core::registry::PROCESS_REGISTRY;
    let pcb = PROCESS_REGISTRY.find_by_pid(pid)
        .ok_or(PgidError::NoSuchProcess)?;
    // Ne pas changer le groupe d'un leader de session.
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
