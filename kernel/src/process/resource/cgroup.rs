// kernel/src/process/resource/cgroup.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// cgroups v2 (basique) — Exo-OS Couche 1.5
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;

/// Nombre maximum de cgroups.
const MAX_CGROUPS: usize = 256;
/// Référence au cgroup racine.
pub const ROOT_CGROUP_ID: u32 = 0;

/// Contrôleurs cgroup v2 supportés.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum CgroupController {
    Cpu,
    Memory,
    Io,
    Pids,
}

/// Politiques CPU pour un cgroup.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct CpuPolicy {
    /// Quota CPU en µs par période (0 = illimité).
    pub quota_us:  u64,
    /// Période de quota (µs).
    pub period_us: u64,
    /// Poids relatif (1..10000 similaire CFS).
    pub weight:    u32,
}

impl CpuPolicy {
    const DEFAULT: Self = Self { quota_us: 0, period_us: 100_000, weight: 100 };
}

/// Politiques mémoire pour un cgroup.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct MemPolicy {
    /// Limite mémoire (octets, 0 = illimité).
    pub limit_bytes:  u64,
    /// Limite swap (octets).
    pub swap_bytes:   u64,
}

impl MemPolicy {
    const DEFAULT: Self = Self { limit_bytes: 0, swap_bytes: 0 };
}

/// Politiques PID pour un cgroup.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct PidPolicy {
    /// Nombre max de processus (0 = illimité).
    pub max:     u32,
}

impl PidPolicy {
    const DEFAULT: Self = Self { max: 0 };
}

/// Un cgroup (nœud de la hiérarchie).
#[repr(C)]
pub struct Cgroup {
    /// Identifiant unique (index dans la table).
    pub id:          u32,
    /// ID du cgroup parent (0 = racine).
    pub parent_id:   u32,
    /// Nombre de processus membres.
    pub pop:         AtomicU32,
    /// Compteur de références.
    pub refcount:    AtomicU32,
    /// Validité.
    pub valid:       AtomicU32,
    /// Verrou de configuration.
    pub lock:        SpinLock<()>,
    /// Politique CPU.
    pub cpu:         CpuPolicy,
    /// Politique mémoire.
    pub mem:         MemPolicy,
    /// Politique PID.
    pub pids:        PidPolicy,
    // Compteurs d'utilisation
    pub cpu_time_us:    AtomicU64,
    pub mem_usage:      AtomicU64,
    pub pid_count:      AtomicU32,
}

impl Cgroup {
    const fn empty() -> Self {
        Self {
            id:          0,
            parent_id:   0,
            pop:         AtomicU32::new(0),
            refcount:    AtomicU32::new(0),
            valid:       AtomicU32::new(0),
            lock:        SpinLock::new(()),
            cpu:         CpuPolicy::DEFAULT,
            mem:         MemPolicy::DEFAULT,
            pids:        PidPolicy::DEFAULT,
            cpu_time_us: AtomicU64::new(0),
            mem_usage:   AtomicU64::new(0),
            pid_count:   AtomicU32::new(0),
        }
    }
}

unsafe impl Sync for Cgroup {}

/// Table globale des cgroups.
pub struct CgroupTable {
    slots: [Cgroup; MAX_CGROUPS],
    lock:  SpinLock<()>,
    count: AtomicU32,
}

unsafe impl Sync for CgroupTable {}

impl CgroupTable {
    const fn new() -> Self {
        const EMPTY: Cgroup = Cgroup::empty();
        Self {
            slots: [EMPTY; MAX_CGROUPS],
            lock:  SpinLock::new(()),
            count: AtomicU32::new(0),
        }
    }

    /// Référence au cgroup racine.
    pub fn root(&self) -> &Cgroup {
        &self.slots[0]
    }

    /// Crée un nouveau cgroup enfant de `parent_id`.
    pub fn create(&self, parent_id: u32) -> Option<u32> {
        let _guard = self.lock.lock();
        for (idx, slot) in self.slots.iter().enumerate() {
            if slot.valid.load(Ordering::Acquire) == 0 && idx > 0 {
                // SAFETY : on tient le write_lock exclusif ; le slot est libre (valid==0)
                // et ne sera accessible qu'après store(valid, 1). Mutation unique.
                unsafe {
                    let s = slot as *const Cgroup as *mut Cgroup;
                    (*s).id        = idx as u32;
                    (*s).parent_id = parent_id;
                    (*s).cpu       = CpuPolicy::DEFAULT;
                    (*s).mem       = MemPolicy::DEFAULT;
                    (*s).pids      = PidPolicy::DEFAULT;
                }
                slot.pop.store(0, Ordering::Release);
                slot.refcount.store(1, Ordering::Release);
                slot.cpu_time_us.store(0, Ordering::Release);
                slot.mem_usage.store(0, Ordering::Release);
                slot.pid_count.store(0, Ordering::Release);
                slot.valid.store(1, Ordering::Release);
                self.count.fetch_add(1, Ordering::Relaxed);
                return Some(idx as u32);
            }
        }
        None
    }

    /// Accède à un cgroup par ID.
    pub fn get(&self, id: u32) -> Option<&Cgroup> {
        let id = id as usize;
        if id >= MAX_CGROUPS { return None; }
        let slot = &self.slots[id];
        if slot.valid.load(Ordering::Acquire) == 1 { Some(slot) } else { None }
    }

    /// Ajoute un processus à un cgroup.
    pub fn add_process(&self, cg_id: u32) {
        if let Some(cg) = self.get(cg_id) {
            cg.pop.fetch_add(1, Ordering::Relaxed);
            cg.pid_count.fetch_add(1, Ordering::Relaxed);
            // Vérifier limite PID
            let max = cg.pids.max;
            if max > 0 && cg.pid_count.load(Ordering::Acquire) > max {
                // Signaler overflow (en pratique, le fork() doit vérifier AVANT d'ajouter).
                cg.pid_count.fetch_sub(1, Ordering::Relaxed);
                cg.pop.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    /// Retire un processus d'un cgroup.
    pub fn remove_process(&self, cg_id: u32) {
        if let Some(cg) = self.get(cg_id) {
            cg.pop.fetch_sub(1, Ordering::Relaxed);
            let c = cg.pid_count.load(Ordering::Acquire);
            if c > 0 { cg.pid_count.fetch_sub(1, Ordering::Relaxed); }
        }
    }

    /// Comptabilise du temps CPU (appelé par arch/timer).
    pub fn account_cpu(&self, cg_id: u32, us: u64) {
        if let Some(cg) = self.get(cg_id) {
            cg.cpu_time_us.fetch_add(us, Ordering::Relaxed);
        }
    }
}

/// Table globale.
static CGROUP_TABLE: CgroupTable = CgroupTable::new();

/// Handle vers un cgroup (type opaque utilisé dans le PCB).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct CgroupHandle(pub u32);

impl CgroupHandle {
    pub const ROOT: Self = Self(ROOT_CGROUP_ID);
}

/// Initialise le cgroup racine (appelé par process::init).
/// Les champs non-atomiques (id, parent_id) sont déjà 0 par construction statique.
pub fn init() {
    let root = &CGROUP_TABLE.slots[0];
    root.refcount.store(1, Ordering::Release);
    root.valid.store(1, Ordering::Release);
    CGROUP_TABLE.count.store(1, Ordering::Release);
}

/// Accède à la table globale.
pub fn cgroup_table() -> &'static CgroupTable { &CGROUP_TABLE }
