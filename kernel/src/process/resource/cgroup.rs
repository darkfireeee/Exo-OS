// kernel/src/process/resource/cgroup.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// cgroups v2 (basic) — Exo-OS Layer 1.5
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::sync::spinlock::SpinLock;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Maximum number of cgroups.
const MAX_CGROUPS: usize = 256;
/// Reference to root cgroup.
pub const ROOT_CGROUP_ID: u32 = 0;

/// Supported cgroup v2 controllers.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum CgroupController {
    Cpu,
    Memory,
    Io,
    Pids,
}

/// CPU policies for a cgroup.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct CpuPolicy {
    /// CPU quota in µs per period (0 = unlimited).
    pub quota_us: u64,
    /// Quota period (µs).
    pub period_us: u64,
    /// Relative weight (1..10000 similar to CFS).
    pub weight: u32,
}

impl CpuPolicy {
    const DEFAULT: Self = Self {
        quota_us: 0,
        period_us: 100_000,
        weight: 100,
    };
}

/// Memory policies for a cgroup.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct MemPolicy {
    /// Memory limit (bytes, 0 = unlimited).
    pub limit_bytes: u64,
    /// Swap limit (bytes).
    pub swap_bytes: u64,
}

impl MemPolicy {
    const DEFAULT: Self = Self {
        limit_bytes: 0,
        swap_bytes: 0,
    };
}

/// PID policies for a cgroup.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct PidPolicy {
    /// Maximum number of processes (0 = unlimited).
    pub max: u32,
}

impl PidPolicy {
    const DEFAULT: Self = Self { max: 0 };
}

/// A cgroup (hierarchy node).
#[repr(C)]
pub struct Cgroup {
    /// Unique identifier (index in table).
    pub id: u32,
    /// Parent cgroup ID (0 = root).
    pub parent_id: u32,
    /// Number of member processes.
    pub pop: AtomicU32,
    /// Reference counter.
    pub refcount: AtomicU32,
    /// Validity.
    pub valid: AtomicU32,
    /// Configuration lock.
    pub lock: SpinLock<()>,
    /// CPU policy.
    pub cpu: CpuPolicy,
    /// Memory policy.
    pub mem: MemPolicy,
    /// PID policy.
    pub pids: PidPolicy,
    // Usage counters
    pub cpu_time_us: AtomicU64,
    pub mem_usage: AtomicU64,
    pub pid_count: AtomicU32,
}

impl Cgroup {
    const fn empty() -> Self {
        Self {
            id: 0,
            parent_id: 0,
            pop: AtomicU32::new(0),
            refcount: AtomicU32::new(0),
            valid: AtomicU32::new(0),
            lock: SpinLock::new(()),
            cpu: CpuPolicy::DEFAULT,
            mem: MemPolicy::DEFAULT,
            pids: PidPolicy::DEFAULT,
            cpu_time_us: AtomicU64::new(0),
            mem_usage: AtomicU64::new(0),
            pid_count: AtomicU32::new(0),
        }
    }
}

unsafe impl Sync for Cgroup {}

/// Global cgroup table.
pub struct CgroupTable {
    slots: [Cgroup; MAX_CGROUPS],
    lock: SpinLock<()>,
    count: AtomicU32,
}

unsafe impl Sync for CgroupTable {}

impl Default for CgroupTable {
    fn default() -> Self {
        Self::new()
    }
}

impl CgroupTable {
    const fn new() -> Self {
        Self {
            slots: [const { Cgroup::empty() }; MAX_CGROUPS],
            lock: SpinLock::new(()),
            count: AtomicU32::new(0),
        }
    }

    /// Reference to root cgroup.
    pub fn root(&self) -> &Cgroup {
        &self.slots[0]
    }

    /// Creates a new cgroup child of `parent_id`.
    pub fn create(&self, parent_id: u32) -> Option<u32> {
        let _guard = self.lock.lock();
        for (idx, slot) in self.slots.iter().enumerate() {
            if slot.valid.load(Ordering::Acquire) == 0 && idx > 0 {
                // SAFETY: exclusive write_lock; free slot (valid==0), accessible only after store(valid, 1).
                unsafe {
                    let s = slot as *const Cgroup as *mut Cgroup;
                    (*s).id = idx as u32;
                    (*s).parent_id = parent_id;
                    (*s).cpu = CpuPolicy::DEFAULT;
                    (*s).mem = MemPolicy::DEFAULT;
                    (*s).pids = PidPolicy::DEFAULT;
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

    /// Accesses a cgroup by ID.
    pub fn get(&self, id: u32) -> Option<&Cgroup> {
        let id = id as usize;
        if id >= MAX_CGROUPS {
            return None;
        }
        let slot = &self.slots[id];
        if slot.valid.load(Ordering::Acquire) == 1 {
            Some(slot)
        } else {
            None
        }
    }

    /// Adds a process to a cgroup.
    pub fn add_process(&self, cg_id: u32) {
        if let Some(cg) = self.get(cg_id) {
            cg.pop.fetch_add(1, Ordering::Relaxed);
            cg.pid_count.fetch_add(1, Ordering::Relaxed);
            // Check PID limit
            let max = cg.pids.max;
            if max > 0 && cg.pid_count.load(Ordering::Acquire) > max {
                // Signal overflow (in practice, fork() must check BEFORE adding).
                cg.pid_count.fetch_sub(1, Ordering::Relaxed);
                cg.pop.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    /// Removes a process from a cgroup.
    pub fn remove_process(&self, cg_id: u32) {
        if let Some(cg) = self.get(cg_id) {
            cg.pop.fetch_sub(1, Ordering::Relaxed);
            let c = cg.pid_count.load(Ordering::Acquire);
            if c > 0 {
                cg.pid_count.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    /// Accounts for CPU time (called by arch/timer).
    pub fn account_cpu(&self, cg_id: u32, us: u64) {
        if let Some(cg) = self.get(cg_id) {
            cg.cpu_time_us.fetch_add(us, Ordering::Relaxed);
        }
    }
}

/// Global table.
static CGROUP_TABLE: CgroupTable = CgroupTable::new();
static CGROUP_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Handle to a cgroup (opaque type used in PCB).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct CgroupHandle(pub u32);

impl CgroupHandle {
    pub const ROOT: Self = Self(ROOT_CGROUP_ID);
}

/// Initializes root cgroup.
///
/// Idempotent: boot calls this before `scheduler::init()` so runqueues
/// can never observe a missing root cgroup, then `process::init()` calls it
/// again during full process initialization.
/// Non-atomic fields (id, parent_id) are already 0 by static construction.
pub fn init() {
    if CGROUP_INITIALIZED.swap(true, Ordering::AcqRel) {
        return;
    }
    let root = &CGROUP_TABLE.slots[0];
    root.refcount.store(1, Ordering::Release);
    root.valid.store(1, Ordering::Release);
    CGROUP_TABLE.count.store(1, Ordering::Release);
}

#[inline]
pub fn is_initialized() -> bool {
    CGROUP_INITIALIZED.load(Ordering::Acquire)
}

/// Accesses global table.
pub fn cgroup_table() -> &'static CgroupTable {
    &CGROUP_TABLE
}
