// SPDX-License-Identifier: GPL-3.0
// ExoFS NUMA — CPU ↔ NUMA node affinity map
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::scheduler::smp::topology::MAX_CPUS as SCHED_MAX_CPUS;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

// ─── Constants ────────────────────────────────────────────────────────────────

pub const MAX_NUMA_NODES: usize = 8;
/// Maximum number of CPUs supported by the NUMA subsystem.
/// Must remain identical to `scheduler::smp::topology::MAX_CPUS`.
pub const MAX_CPUS: usize = SCHED_MAX_CPUS;
/// Sentinel: CPU not assigned to a node.
pub const CPU_NODE_NONE: u8 = u8::MAX;

/// Compile-time assertion: consistency with canonical scheduler constant.
const _: () = assert!(
    MAX_CPUS == crate::scheduler::smp::topology::MAX_CPUS,
    "numa_affinity::MAX_CPUS doit correspondre à scheduler::topology::MAX_CPUS"
);

// ─── NumaNodeId ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct NumaNodeId(pub u8);

impl NumaNodeId {
    pub const UNSET: NumaNodeId = NumaNodeId(CPU_NODE_NONE);
    pub fn is_valid(self) -> bool {
        (self.0 as usize) < MAX_NUMA_NODES
    }
    pub fn idx(self) -> usize {
        self.0 as usize
    }
}

// ─── CpuId ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct CpuId(pub u32);

impl CpuId {
    pub fn is_valid(self) -> bool {
        (self.0 as usize) < MAX_CPUS
    }
    pub fn idx(self) -> usize {
        self.0 as usize
    }
}

// ─── AffinityNodeEntry ────────────────────────────────────────────────────────

/// NUMA node slot in the affinity table.
#[derive(Clone, Copy, Debug)]
pub struct AffinityNodeEntry {
    /// List of CPUs on this node (indices in the cpu_nodes array).
    pub cpu_count: u16,
    /// Index of the first CPU in the compact list (unused if cpu_count=0).
    /// Additional node information.
    pub memory_mb: u64,
    pub active: bool,
    /// Local distance (self→self = 10 by ACPI SLIT convention).
    pub local_dist: u8,
}

impl AffinityNodeEntry {
    pub const fn empty() -> Self {
        Self {
            cpu_count: 0,
            memory_mb: 0,
            active: false,
            local_dist: 10,
        }
    }
}

// ─── AffinityInner ────────────────────────────────────────────────────────────

struct AffinityInner {
    /// cpu_nodes[cpu_id] = NumaNodeId or CPU_NODE_NONE.
    cpu_nodes: [u8; MAX_CPUS],
    /// nodes[node_id] = node information.
    nodes: [AffinityNodeEntry; MAX_NUMA_NODES],
    /// Inter-node distance matrix (ACPI SLIT, 8×8).
    dist_matrix: [[u8; MAX_NUMA_NODES]; MAX_NUMA_NODES],
    /// Number of active nodes.
    active_nodes: u8,
}

impl AffinityInner {
    const fn new() -> Self {
        Self {
            cpu_nodes: [CPU_NODE_NONE; MAX_CPUS],
            nodes: [AffinityNodeEntry::empty(); MAX_NUMA_NODES],
            dist_matrix: [[20u8; MAX_NUMA_NODES]; MAX_NUMA_NODES],
            active_nodes: 0,
        }
    }
}

// ─── AffinityMap ──────────────────────────────────────────────────────────────

/// CPU ↔ NUMA node affinity register (flat array, no BTreeMap, no external SpinLock).
pub struct AffinityMap {
    inner: UnsafeCell<AffinityInner>,
    lock: AtomicU64,
}

// SAFETY: `inner` is protected by spinlock `lock`, serializing all
// reads/writes on the CPU↔NUMA tables.
unsafe impl Sync for AffinityMap {}
// SAFETY: no internal borrow depends on structure address; cross-thread
// transfer remains safe under the same locking protocol.
unsafe impl Send for AffinityMap {}

impl AffinityMap {
    pub const fn new_const() -> Self {
        Self {
            inner: UnsafeCell::new(AffinityInner::new()),
            lock: AtomicU64::new(0),
        }
    }

    fn acquire(&self) {
        while self
            .lock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }
    fn release(&self) {
        self.lock.store(0, Ordering::Release);
    }

    // ── Registration ──────────────────────────────────────────────────────────

    /// Registers a NUMA node with its memory capacity in MiB.
    pub fn register_node(&self, node: NumaNodeId, memory_mb: u64) -> ExofsResult<()> {
        if !node.is_valid() {
            return Err(ExofsError::InvalidArgument);
        }
        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let inner = unsafe { &mut *self.inner.get() };
        let entry = &mut inner.nodes[node.idx()];
        if !entry.active {
            inner.active_nodes = inner.active_nodes.saturating_add(1);
        }
        entry.active = true;
        entry.memory_mb = memory_mb;
        // Local distance = 10 (ACPI SLIT standard)
        inner.dist_matrix[node.idx()][node.idx()] = 10;
        self.release();
        Ok(())
    }

    /// Associates a CPU with a NUMA node.
    pub fn register_cpu(&self, cpu: CpuId, node: NumaNodeId) -> ExofsResult<()> {
        if !cpu.is_valid() {
            return Err(ExofsError::InvalidArgument);
        }
        if !node.is_valid() {
            return Err(ExofsError::InvalidArgument);
        }
        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let inner = unsafe { &mut *self.inner.get() };
        // Activate node if needed
        if !inner.nodes[node.idx()].active {
            inner.nodes[node.idx()].active = true;
            inner.active_nodes = inner.active_nodes.saturating_add(1);
        }
        // If CPU was on another node, decrement previous one
        let old_node = inner.cpu_nodes[cpu.idx()];
        if old_node != CPU_NODE_NONE && old_node as usize != node.idx() {
            let on = old_node as usize;
            if on < MAX_NUMA_NODES {
                inner.nodes[on].cpu_count = inner.nodes[on].cpu_count.saturating_sub(1);
            }
        }
        inner.cpu_nodes[cpu.idx()] = node.0;
        if old_node != node.0 {
            inner.nodes[node.idx()].cpu_count = inner.nodes[node.idx()].cpu_count.saturating_add(1);
        }
        self.release();
        Ok(())
    }

    /// Defines distance between two nodes (symmetric).
    pub fn set_distance(&self, a: NumaNodeId, b: NumaNodeId, dist: u8) -> ExofsResult<()> {
        if !a.is_valid() || !b.is_valid() {
            return Err(ExofsError::InvalidArgument);
        }
        if dist == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let inner = unsafe { &mut *self.inner.get() };
        inner.dist_matrix[a.idx()][b.idx()] = dist;
        inner.dist_matrix[b.idx()][a.idx()] = dist;
        self.release();
        Ok(())
    }

    // ── Reads ─────────────────────────────────────────────────────────────────

    /// Returns the NUMA node of a CPU (or None).
    pub fn node_of_cpu(&self, cpu: CpuId) -> Option<NumaNodeId> {
        if !cpu.is_valid() {
            return None;
        }
        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired prior.
        let inner = unsafe { &*self.inner.get() };
        let n = inner.cpu_nodes[cpu.idx()];
        self.release();
        if n == CPU_NODE_NONE {
            None
        } else {
            Some(NumaNodeId(n))
        }
    }

    /// Returns the number of CPUs on a node.
    pub fn cpu_count_of_node(&self, node: NumaNodeId) -> u16 {
        if !node.is_valid() {
            return 0;
        }
        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let c = unsafe { (*self.inner.get()).nodes[node.idx()].cpu_count };
        self.release();
        c
    }

    /// Returns information for a node.
    pub fn node_entry(&self, node: NumaNodeId) -> Option<AffinityNodeEntry> {
        if !node.is_valid() {
            return None;
        }
        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let e = unsafe { (*self.inner.get()).nodes[node.idx()] };
        self.release();
        if e.active {
            Some(e)
        } else {
            None
        }
    }

    /// Distance between two nodes.
    pub fn distance(&self, a: NumaNodeId, b: NumaNodeId) -> u8 {
        if !a.is_valid() || !b.is_valid() {
            return u8::MAX;
        }
        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let d = unsafe { (*self.inner.get()).dist_matrix[a.idx()][b.idx()] };
        self.release();
        d
    }

    /// Number of active nodes.
    pub fn active_node_count(&self) -> usize {
        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let n = unsafe { (*self.inner.get()).active_nodes as usize };
        self.release();
        n
    }

    /// List of active nodes (OOM-02, RECUR-01).
    pub fn active_nodes(&self) -> ExofsResult<Vec<NumaNodeId>> {
        let mut v = Vec::new();
        v.try_reserve(MAX_NUMA_NODES)
            .map_err(|_| ExofsError::NoMemory)?;
        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let inner = unsafe { &*self.inner.get() };
        let mut i = 0usize;
        while i < MAX_NUMA_NODES {
            if inner.nodes[i].active {
                v.try_reserve(1).map_err(|_| {
                    self.release();
                    ExofsError::NoMemory
                })?;
                v.push(NumaNodeId(i as u8));
            }
            i = i.wrapping_add(1);
        }
        self.release();
        Ok(v)
    }

    /// List of CPUs on a given node (RECUR-01: while).
    pub fn cpus_of_node(&self, node: NumaNodeId) -> ExofsResult<Vec<CpuId>> {
        if !node.is_valid() {
            return Err(ExofsError::InvalidArgument);
        }
        let mut v = Vec::new();
        v.try_reserve(64).map_err(|_| ExofsError::NoMemory)?;
        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let inner = unsafe { &*self.inner.get() };
        let mut i = 0usize;
        while i < MAX_CPUS {
            if inner.cpu_nodes[i] == node.0 {
                v.try_reserve(1).map_err(|_| {
                    self.release();
                    ExofsError::NoMemory
                })?;
                v.push(CpuId(i as u32));
            }
            i = i.wrapping_add(1);
        }
        self.release();
        Ok(v)
    }

    /// Nearest node to node `from` (minimum distance among active, RECUR-01).
    pub fn nearest_node(&self, from: NumaNodeId) -> Option<NumaNodeId> {
        if !from.is_valid() {
            return None;
        }
        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let inner = unsafe { &*self.inner.get() };
        let mut best_dist = u8::MAX;
        let mut best_node: Option<NumaNodeId> = None;
        let mut i = 0usize;
        while i < MAX_NUMA_NODES {
            if i != from.idx() && inner.nodes[i].active {
                let d = inner.dist_matrix[from.idx()][i];
                if d < best_dist {
                    best_dist = d;
                    best_node = Some(NumaNodeId(i as u8));
                }
            }
            i = i.wrapping_add(1);
        }
        self.release();
        best_node
    }

    /// Node with largest available memory (RECUR-01).
    pub fn largest_memory_node(&self) -> Option<NumaNodeId> {
        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let inner = unsafe { &*self.inner.get() };
        let mut best_mb = 0u64;
        let mut best: Option<NumaNodeId> = None;
        let mut i = 0usize;
        while i < MAX_NUMA_NODES {
            if inner.nodes[i].active && inner.nodes[i].memory_mb > best_mb {
                best_mb = inner.nodes[i].memory_mb;
                best = Some(NumaNodeId(i as u8));
            }
            i = i.wrapping_add(1);
        }
        self.release();
        best
    }

    /// Resets all associations.
    pub fn reset(&self) {
        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let inner = unsafe { &mut *self.inner.get() };
        *inner = AffinityInner::new();
        self.release();
    }
}

/// Global singleton of the affinity map.
pub static AFFINITY_MAP: AffinityMap = AffinityMap::new_const();

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ok<T, E: core::fmt::Debug>(res: Result<T, E>) -> T {
        match res {
            Ok(value) => value,
            Err(err) => panic!("unexpected error: {err:?}"),
        }
    }

    fn some<T>(value: Option<T>) -> T {
        match value {
            Some(inner) => inner,
            None => panic!("unexpected None"),
        }
    }

    #[test]
    fn test_numa_node_id_valid() {
        assert!(NumaNodeId(0).is_valid());
        assert!(NumaNodeId(7).is_valid());
        assert!(!NumaNodeId(8).is_valid());
        assert!(!NumaNodeId::UNSET.is_valid());
    }

    #[test]
    fn test_cpu_id_valid() {
        assert!(CpuId(0).is_valid());
        assert!(CpuId(255).is_valid());
        assert!(!CpuId(MAX_CPUS as u32).is_valid());
    }

    #[test]
    fn test_register_node() {
        let m = AffinityMap::new_const();
        ok(m.register_node(NumaNodeId(0), 8192));
        let e = some(m.node_entry(NumaNodeId(0)));
        assert_eq!(e.memory_mb, 8192);
        assert!(e.active);
    }

    #[test]
    fn test_register_cpu() {
        let m = AffinityMap::new_const();
        ok(m.register_node(NumaNodeId(0), 4096));
        ok(m.register_cpu(CpuId(0), NumaNodeId(0)));
        ok(m.register_cpu(CpuId(1), NumaNodeId(0)));
        assert_eq!(m.node_of_cpu(CpuId(0)), Some(NumaNodeId(0)));
        assert_eq!(m.cpu_count_of_node(NumaNodeId(0)), 2);
    }

    #[test]
    fn test_register_cpu_invalid_node() {
        let m = AffinityMap::new_const();
        assert!(m.register_cpu(CpuId(0), NumaNodeId(8)).is_err());
    }

    #[test]
    fn test_register_cpu_invalid_cpu() {
        let m = AffinityMap::new_const();
        assert!(m
            .register_cpu(CpuId(MAX_CPUS as u32), NumaNodeId(0))
            .is_err());
    }

    #[test]
    fn test_node_of_cpu_unregistered() {
        let m = AffinityMap::new_const();
        assert!(m.node_of_cpu(CpuId(42)).is_none());
    }

    #[test]
    fn test_set_distance() {
        let m = AffinityMap::new_const();
        ok(m.register_node(NumaNodeId(0), 4096));
        ok(m.register_node(NumaNodeId(1), 4096));
        ok(m.set_distance(NumaNodeId(0), NumaNodeId(1), 40));
        assert_eq!(m.distance(NumaNodeId(0), NumaNodeId(1)), 40);
        assert_eq!(m.distance(NumaNodeId(1), NumaNodeId(0)), 40);
    }

    #[test]
    fn test_nearest_node() {
        let m = AffinityMap::new_const();
        ok(m.register_node(NumaNodeId(0), 4096));
        ok(m.register_node(NumaNodeId(1), 4096));
        ok(m.register_node(NumaNodeId(2), 4096));
        ok(m.set_distance(NumaNodeId(0), NumaNodeId(1), 40));
        ok(m.set_distance(NumaNodeId(0), NumaNodeId(2), 20));
        let near = some(m.nearest_node(NumaNodeId(0)));
        assert_eq!(near, NumaNodeId(2));
    }

    #[test]
    fn test_active_nodes() {
        let m = AffinityMap::new_const();
        ok(m.register_node(NumaNodeId(0), 4096));
        ok(m.register_node(NumaNodeId(2), 4096));
        let nodes = ok(m.active_nodes());
        assert_eq!(nodes.len(), 2);
        assert!(nodes.contains(&NumaNodeId(0)));
        assert!(nodes.contains(&NumaNodeId(2)));
    }

    #[test]
    fn test_cpus_of_node() {
        let m = AffinityMap::new_const();
        ok(m.register_node(NumaNodeId(0), 4096));
        ok(m.register_cpu(CpuId(10), NumaNodeId(0)));
        ok(m.register_cpu(CpuId(11), NumaNodeId(0)));
        let cpus = ok(m.cpus_of_node(NumaNodeId(0)));
        assert_eq!(cpus.len(), 2);
        assert!(cpus.contains(&CpuId(10)));
        assert!(cpus.contains(&CpuId(11)));
    }

    #[test]
    fn test_largest_memory_node() {
        let m = AffinityMap::new_const();
        ok(m.register_node(NumaNodeId(0), 4096));
        ok(m.register_node(NumaNodeId(1), 16384));
        let n = some(m.largest_memory_node());
        assert_eq!(n, NumaNodeId(1));
    }

    #[test]
    fn test_cpu_migration_between_nodes() {
        let m = AffinityMap::new_const();
        ok(m.register_node(NumaNodeId(0), 4096));
        ok(m.register_node(NumaNodeId(1), 4096));
        ok(m.register_cpu(CpuId(5), NumaNodeId(0)));
        assert_eq!(m.cpu_count_of_node(NumaNodeId(0)), 1);
        // Move to node 1
        ok(m.register_cpu(CpuId(5), NumaNodeId(1)));
        assert_eq!(m.cpu_count_of_node(NumaNodeId(0)), 0);
        assert_eq!(m.cpu_count_of_node(NumaNodeId(1)), 1);
    }

    #[test]
    fn test_reset() {
        let m = AffinityMap::new_const();
        ok(m.register_node(NumaNodeId(0), 4096));
        ok(m.register_cpu(CpuId(0), NumaNodeId(0)));
        m.reset();
        assert!(m.node_of_cpu(CpuId(0)).is_none());
        assert_eq!(m.active_node_count(), 0);
    }
}
