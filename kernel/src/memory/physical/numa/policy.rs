// kernel/src/memory/physical/numa/policy.rs
//
// NUMA allocation policy per process / thread.
//
// Each thread possesses a `NumaPolicy` stored in its TCB (by process/).
// This module defines policy types and application helpers
// without depending on process/ (dependency inversion via trait).
//
// Policy types:
//   • Default        — allocate on local node of current CPU
//   • Bind(mask)     — allocate exclusively on mask nodes
//   • Preferred(nid) — prefer node `nid`, fallback on others
//   • Interleave(mask) — round-robin across mask nodes
//
// LAYER 0 — no scheduler/process/ipc/fs dependencies.

use core::sync::atomic::{AtomicU64, Ordering};
use spin::RwLock;

use super::distance::closest_node;
use super::node::{MAX_NUMA_NODES, NUMA_NODES, NUMA_NODE_INVALID};

// ─────────────────────────────────────────────────────────────────────────────
// NumaPolicy Type
// ─────────────────────────────────────────────────────────────────────────────

/// NUMA node mask (one bit per node, bit i = node i).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NumaNodeMask(pub u64);

impl NumaNodeMask {
    pub const ALL: Self = Self(u64::MAX);
    pub const NONE: Self = Self(0);

    #[inline]
    pub fn from_node(nid: u32) -> Self {
        if nid >= 64 {
            return Self::NONE;
        }
        Self(1u64 << nid)
    }

    #[inline]
    pub fn contains(&self, nid: u32) -> bool {
        if nid >= 64 {
            return false;
        }
        self.0 & (1u64 << nid) != 0
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// First node in mask.
    #[inline]
    pub fn first_set(&self) -> Option<u32> {
        if self.0 == 0 {
            return None;
        }
        Some(self.0.trailing_zeros())
    }

    /// Number of nodes in mask.
    #[inline]
    pub fn count(&self) -> u32 {
        self.0.count_ones()
    }
}

/// NUMA allocation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumaPolicy {
    /// Allocate on local node of current CPU.
    Default,
    /// Allocate exclusively on nodes in mask.
    /// Fails if no node in mask has memory.
    Bind(NumaNodeMask),
    /// Prefer node `nid`, fallback on others by increasing distance.
    Preferred(u32),
    /// Round-robin across nodes in mask.
    Interleave(NumaNodeMask),
}

impl NumaPolicy {
    /// Selects target node taking into account current CPU and
    /// thread interleave counter.
    ///
    /// `cpu_id`: current CPU to determine local node.
    /// `interleave_counter`: counter dedicated to Interleave policy.
    pub fn select_node(&self, cpu_id: u32, interleave_counter: u64) -> u32 {
        match self {
            NumaPolicy::Default => {
                let origin = NUMA_NODES.node_for_cpu(cpu_id);
                closest_node(origin, 1)
            }
            NumaPolicy::Bind(mask) => {
                if mask.is_empty() {
                    return NUMA_NODE_INVALID;
                }
                // Iterate mask nodes by increasing distance.
                let origin = NUMA_NODES.node_for_cpu(cpu_id);
                let (sorted, len) = super::distance::NUMA_DISTANCE.sorted_nodes_from(origin);
                for &nid in &sorted[..len] {
                    if mask.contains(nid) {
                        if let Some(node) = NUMA_NODES.get(nid) {
                            if node.free_pages() > 0 {
                                return nid;
                            }
                        }
                    }
                }
                NUMA_NODE_INVALID
            }
            NumaPolicy::Preferred(preferred_nid) => {
                // Try preferred node first.
                if let Some(node) = NUMA_NODES.get(*preferred_nid) {
                    if node.free_pages() > 0 {
                        return *preferred_nid;
                    }
                }
                // Fallback by distance from preferred node.
                closest_node(*preferred_nid, 1)
            }
            NumaPolicy::Interleave(mask) => {
                if mask.is_empty() {
                    return NUMA_NODE_INVALID;
                }
                let count = mask.count();
                let slot = (interleave_counter % count as u64) as u32;
                // Find `slot`-th bit in mask.
                let mut cur = 0u32;
                for nid in 0..MAX_NUMA_NODES as u32 {
                    if mask.contains(nid) {
                        if cur == slot {
                            return nid;
                        }
                        cur += 1;
                    }
                }
                NUMA_NODE_INVALID
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Default system policy
// ─────────────────────────────────────────────────────────────────────────────

/// Kernel policy (all threads inherit if not defined).
static SYSTEM_NUMA_POLICY: RwLock<NumaPolicy> = RwLock::new(NumaPolicy::Default);

#[inline]
pub fn get_system_policy() -> NumaPolicy {
    *SYSTEM_NUMA_POLICY.read()
}

pub fn set_system_policy(policy: NumaPolicy) {
    *SYSTEM_NUMA_POLICY.write() = policy;
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistics
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct NumaPolicyStats {
    pub default_allocs: AtomicU64,
    pub bind_allocs: AtomicU64,
    pub preferred_allocs: AtomicU64,
    pub interleave_allocs: AtomicU64,
    pub bind_failures: AtomicU64,
}

impl NumaPolicyStats {
    const fn new() -> Self {
        Self {
            default_allocs: AtomicU64::new(0),
            bind_allocs: AtomicU64::new(0),
            preferred_allocs: AtomicU64::new(0),
            interleave_allocs: AtomicU64::new(0),
            bind_failures: AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for NumaPolicyStats {}
pub static NUMA_POLICY_STATS: NumaPolicyStats = NumaPolicyStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Node selection with stats tracking
// ─────────────────────────────────────────────────────────────────────────────

/// Selects target node according to `policy` and records stats.
pub fn select_node(policy: &NumaPolicy, cpu_id: u32, interleave_ctr: u64) -> u32 {
    let nid = policy.select_node(cpu_id, interleave_ctr);
    match policy {
        NumaPolicy::Default => NUMA_POLICY_STATS
            .default_allocs
            .fetch_add(1, Ordering::Relaxed),
        NumaPolicy::Bind(_) => {
            if nid == NUMA_NODE_INVALID {
                NUMA_POLICY_STATS
                    .bind_failures
                    .fetch_add(1, Ordering::Relaxed)
            } else {
                NUMA_POLICY_STATS
                    .bind_allocs
                    .fetch_add(1, Ordering::Relaxed)
            }
        }
        NumaPolicy::Preferred(_) => NUMA_POLICY_STATS
            .preferred_allocs
            .fetch_add(1, Ordering::Relaxed),
        NumaPolicy::Interleave(_) => NUMA_POLICY_STATS
            .interleave_allocs
            .fetch_add(1, Ordering::Relaxed),
    };
    nid
}

// ─────────────────────────────────────────────────────────────────────────────
// Injection trait for scheduler
// ─────────────────────────────────────────────────────────────────────────────

/// Trait that scheduler must implement to provide current NUMA node.
/// Allows memory/ to remain in Layer 0.
pub trait NumaCpuProvider {
    fn current_cpu_id(&self) -> u32;
    fn thread_interleave_counter(&self) -> u64;
    fn thread_numa_policy(&self) -> NumaPolicy;
}

/// Static BSP implementation (before scheduler).
pub struct BspNumaProvider;
impl NumaCpuProvider for BspNumaProvider {
    fn current_cpu_id(&self) -> u32 {
        0
    }
    fn thread_interleave_counter(&self) -> u64 {
        0
    }
    fn thread_numa_policy(&self) -> NumaPolicy {
        NumaPolicy::Default
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialization
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    // System policy already Default.
}
