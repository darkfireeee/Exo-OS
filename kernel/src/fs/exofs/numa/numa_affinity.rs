// SPDX-License-Identifier: MIT
// ExoFS NUMA — Carte d'affinité CPU↔nœud NUMA
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─── Constants ────────────────────────────────────────────────────────────────

pub const MAX_NUMA_NODES: usize = 8;
pub const MAX_CPUS:       usize = 256;
/// Sentinel : CPU non assigné à un nœud.
pub const CPU_NODE_NONE:  u8    = u8::MAX;

// ─── NumaNodeId ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct NumaNodeId(pub u8);

impl NumaNodeId {
    pub const UNSET: NumaNodeId = NumaNodeId(CPU_NODE_NONE);
    pub fn is_valid(self) -> bool { (self.0 as usize) < MAX_NUMA_NODES }
    pub fn idx(self) -> usize { self.0 as usize }
}

// ─── CpuId ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct CpuId(pub u32);

impl CpuId {
    pub fn is_valid(self) -> bool { (self.0 as usize) < MAX_CPUS }
    pub fn idx(self) -> usize { self.0 as usize }
}

// ─── AffinityNodeEntry ────────────────────────────────────────────────────────

/// Slot d'un nœud NUMA dans la table d'affinité.
#[derive(Clone, Copy, Debug)]
pub struct AffinityNodeEntry {
    /// Liste des CPU sur ce nœud (indices dans le tableau cpu_nodes).
    pub cpu_count: u16,
    /// Indice du premier CPU dans la liste compacte (non utilisé si cpu_count=0).
    /// Informations complementaires du nœud.
    pub memory_mb:  u64,
    pub active:     bool,
    /// Distance locale (self→self = 10 par convention ACPI SLIT).
    pub local_dist: u8,
}

impl AffinityNodeEntry {
    pub const fn empty() -> Self {
        Self { cpu_count: 0, memory_mb: 0, active: false, local_dist: 10 }
    }
}

// ─── AffinityInner ────────────────────────────────────────────────────────────

struct AffinityInner {
    /// cpu_nodes[cpu_id] = NumaNodeId ou CPU_NODE_NONE.
    cpu_nodes: [u8; MAX_CPUS],
    /// nodes[node_id] = informations du nœud.
    nodes: [AffinityNodeEntry; MAX_NUMA_NODES],
    /// Matrice de distance inter-nœuds (SLIT ACPI, 8×8).
    dist_matrix: [[u8; MAX_NUMA_NODES]; MAX_NUMA_NODES],
    /// Nombre de nœuds actifs.
    active_nodes: u8,
}

impl AffinityInner {
    const fn new() -> Self {
        Self {
            cpu_nodes:    [CPU_NODE_NONE; MAX_CPUS],
            nodes:        [AffinityNodeEntry::empty(); MAX_NUMA_NODES],
            dist_matrix:  [[20u8; MAX_NUMA_NODES]; MAX_NUMA_NODES],
            active_nodes: 0,
        }
    }
}

// ─── AffinityMap ──────────────────────────────────────────────────────────────

/// Registre d'affinité CPU↔nœud NUMA (tableau plat, sans BTreeMap, sans SpinLock externe).
pub struct AffinityMap {
    inner: UnsafeCell<AffinityInner>,
    lock:  AtomicU64,
}

unsafe impl Sync for AffinityMap {}
unsafe impl Send for AffinityMap {}

impl AffinityMap {
    pub const fn new_const() -> Self {
        Self {
            inner: UnsafeCell::new(AffinityInner::new()),
            lock:  AtomicU64::new(0),
        }
    }

    fn acquire(&self) {
        while self.lock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }
    fn release(&self) { self.lock.store(0, Ordering::Release); }

    // ── Enregistrement ────────────────────────────────────────────────────────

    /// Déclare un nœud NUMA avec sa capacité mémoire en MiB.
    pub fn register_node(&self, node: NumaNodeId, memory_mb: u64) -> ExofsResult<()> {
        if !node.is_valid() { return Err(ExofsError::InvalidArgument); }
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let inner = unsafe { &mut *self.inner.get() };
        let entry = &mut inner.nodes[node.idx()];
        if !entry.active {
            inner.active_nodes = inner.active_nodes.saturating_add(1);
        }
        entry.active    = true;
        entry.memory_mb = memory_mb;
        // Distance locale = 10 (standard ACPI SLIT)
        inner.dist_matrix[node.idx()][node.idx()] = 10;
        self.release();
        Ok(())
    }

    /// Associe un CPU à un nœud NUMA.
    pub fn register_cpu(&self, cpu: CpuId, node: NumaNodeId) -> ExofsResult<()> {
        if !cpu.is_valid()  { return Err(ExofsError::InvalidArgument); }
        if !node.is_valid() { return Err(ExofsError::InvalidArgument); }
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let inner = unsafe { &mut *self.inner.get() };
        // Activer le nœud si besoin
        if !inner.nodes[node.idx()].active {
            inner.nodes[node.idx()].active = true;
            inner.active_nodes = inner.active_nodes.saturating_add(1);
        }
        // Si le CPU était sur un autre nœud, décrémenter l'ancien
        let old_node = inner.cpu_nodes[cpu.idx()];
        if old_node != CPU_NODE_NONE && old_node as usize != node.idx() {
            let on = old_node as usize;
            if on < MAX_NUMA_NODES {
                inner.nodes[on].cpu_count =
                    inner.nodes[on].cpu_count.saturating_sub(1);
            }
        }
        inner.cpu_nodes[cpu.idx()] = node.0;
        if old_node != node.0 {
            inner.nodes[node.idx()].cpu_count =
                inner.nodes[node.idx()].cpu_count.saturating_add(1);
        }
        self.release();
        Ok(())
    }

    /// Définit la distance entre deux nœuds (symétrique).
    pub fn set_distance(&self, a: NumaNodeId, b: NumaNodeId, dist: u8) -> ExofsResult<()> {
        if !a.is_valid() || !b.is_valid() { return Err(ExofsError::InvalidArgument); }
        if dist == 0 { return Err(ExofsError::InvalidArgument); }
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let inner = unsafe { &mut *self.inner.get() };
        inner.dist_matrix[a.idx()][b.idx()] = dist;
        inner.dist_matrix[b.idx()][a.idx()] = dist;
        self.release();
        Ok(())
    }

    // ── Lecture ───────────────────────────────────────────────────────────────

    /// Retourne le nœud NUMA d'un CPU (ou None).
    pub fn node_of_cpu(&self, cpu: CpuId) -> Option<NumaNodeId> {
        if !cpu.is_valid() { return None; }
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let inner = unsafe { &*self.inner.get() };
        let n = inner.cpu_nodes[cpu.idx()];
        self.release();
        if n == CPU_NODE_NONE { None } else { Some(NumaNodeId(n)) }
    }

    /// Retourne le nombre de CPUs sur un nœud.
    pub fn cpu_count_of_node(&self, node: NumaNodeId) -> u16 {
        if !node.is_valid() { return 0; }
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let c = unsafe { (*self.inner.get()).nodes[node.idx()].cpu_count };
        self.release();
        c
    }

    /// Retourne les informations d'un nœud.
    pub fn node_entry(&self, node: NumaNodeId) -> Option<AffinityNodeEntry> {
        if !node.is_valid() { return None; }
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let e = unsafe { (*self.inner.get()).nodes[node.idx()] };
        self.release();
        if e.active { Some(e) } else { None }
    }

    /// Distance entre deux nœuds.
    pub fn distance(&self, a: NumaNodeId, b: NumaNodeId) -> u8 {
        if !a.is_valid() || !b.is_valid() { return u8::MAX; }
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let d = unsafe { (*self.inner.get()).dist_matrix[a.idx()][b.idx()] };
        self.release();
        d
    }

    /// Nombre de nœuds actifs.
    pub fn active_node_count(&self) -> usize {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let n = unsafe { (*self.inner.get()).active_nodes as usize };
        self.release();
        n
    }

    /// Liste des nœuds actifs (OOM-02, RECUR-01).
    pub fn active_nodes(&self) -> ExofsResult<Vec<NumaNodeId>> {
        let mut v = Vec::new();
        v.try_reserve(MAX_NUMA_NODES).map_err(|_| ExofsError::NoMemory)?;
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let inner = unsafe { &*self.inner.get() };
        let mut i = 0usize;
        while i < MAX_NUMA_NODES {
            if inner.nodes[i].active {
                v.try_reserve(1).map_err(|_| { self.release(); ExofsError::NoMemory })?;
                v.push(NumaNodeId(i as u8));
            }
            i = i.wrapping_add(1);
        }
        self.release();
        Ok(v)
    }

    /// Liste des CPU sur un nœud donné (RECUR-01 : while).
    pub fn cpus_of_node(&self, node: NumaNodeId) -> ExofsResult<Vec<CpuId>> {
        if !node.is_valid() { return Err(ExofsError::InvalidArgument); }
        let mut v = Vec::new();
        v.try_reserve(64).map_err(|_| ExofsError::NoMemory)?;
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let inner = unsafe { &*self.inner.get() };
        let mut i = 0usize;
        while i < MAX_CPUS {
            if inner.cpu_nodes[i] == node.0 {
                v.try_reserve(1).map_err(|_| { self.release(); ExofsError::NoMemory })?;
                v.push(CpuId(i as u32));
            }
            i = i.wrapping_add(1);
        }
        self.release();
        Ok(v)
    }

    /// Nœud le plus proche du nœud `from` (distance minimale parmi actifs, RECUR-01).
    pub fn nearest_node(&self, from: NumaNodeId) -> Option<NumaNodeId> {
        if !from.is_valid() { return None; }
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let inner = unsafe { &*self.inner.get() };
        let mut best_dist = u8::MAX;
        let mut best_node: Option<NumaNodeId> = None;
        let mut i = 0usize;
        while i < MAX_NUMA_NODES {
            if i != from.idx() && inner.nodes[i].active {
                let d = inner.dist_matrix[from.idx()][i];
                if d < best_dist { best_dist = d; best_node = Some(NumaNodeId(i as u8)); }
            }
            i = i.wrapping_add(1);
        }
        self.release();
        best_node
    }

    /// Nœud avec la plus grande mémoire disponible (RECUR-01).
    pub fn largest_memory_node(&self) -> Option<NumaNodeId> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let inner = unsafe { &*self.inner.get() };
        let mut best_mb  = 0u64;
        let mut best: Option<NumaNodeId> = None;
        let mut i = 0usize;
        while i < MAX_NUMA_NODES {
            if inner.nodes[i].active && inner.nodes[i].memory_mb > best_mb {
                best_mb = inner.nodes[i].memory_mb;
                best    = Some(NumaNodeId(i as u8));
            }
            i = i.wrapping_add(1);
        }
        self.release();
        best
    }

    /// Réinitialise toutes les associations.
    pub fn reset(&self) {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let inner = unsafe { &mut *self.inner.get() };
        *inner = AffinityInner::new();
        self.release();
    }
}

/// Singleton global de la carte d'affinité.
pub static AFFINITY_MAP: AffinityMap = AffinityMap::new_const();

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!CpuId(256).is_valid());
    }

    #[test]
    fn test_register_node() {
        let m = AffinityMap::new_const();
        m.register_node(NumaNodeId(0), 8192).unwrap();
        let e = m.node_entry(NumaNodeId(0)).unwrap();
        assert_eq!(e.memory_mb, 8192);
        assert!(e.active);
    }

    #[test]
    fn test_register_cpu() {
        let m = AffinityMap::new_const();
        m.register_node(NumaNodeId(0), 4096).unwrap();
        m.register_cpu(CpuId(0), NumaNodeId(0)).unwrap();
        m.register_cpu(CpuId(1), NumaNodeId(0)).unwrap();
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
        assert!(m.register_cpu(CpuId(256), NumaNodeId(0)).is_err());
    }

    #[test]
    fn test_node_of_cpu_unregistered() {
        let m = AffinityMap::new_const();
        assert!(m.node_of_cpu(CpuId(42)).is_none());
    }

    #[test]
    fn test_set_distance() {
        let m = AffinityMap::new_const();
        m.register_node(NumaNodeId(0), 4096).unwrap();
        m.register_node(NumaNodeId(1), 4096).unwrap();
        m.set_distance(NumaNodeId(0), NumaNodeId(1), 40).unwrap();
        assert_eq!(m.distance(NumaNodeId(0), NumaNodeId(1)), 40);
        assert_eq!(m.distance(NumaNodeId(1), NumaNodeId(0)), 40);
    }

    #[test]
    fn test_nearest_node() {
        let m = AffinityMap::new_const();
        m.register_node(NumaNodeId(0), 4096).unwrap();
        m.register_node(NumaNodeId(1), 4096).unwrap();
        m.register_node(NumaNodeId(2), 4096).unwrap();
        m.set_distance(NumaNodeId(0), NumaNodeId(1), 40).unwrap();
        m.set_distance(NumaNodeId(0), NumaNodeId(2), 20).unwrap();
        let near = m.nearest_node(NumaNodeId(0)).unwrap();
        assert_eq!(near, NumaNodeId(2));
    }

    #[test]
    fn test_active_nodes() {
        let m = AffinityMap::new_const();
        m.register_node(NumaNodeId(0), 4096).unwrap();
        m.register_node(NumaNodeId(2), 4096).unwrap();
        let nodes = m.active_nodes().unwrap();
        assert_eq!(nodes.len(), 2);
        assert!(nodes.contains(&NumaNodeId(0)));
        assert!(nodes.contains(&NumaNodeId(2)));
    }

    #[test]
    fn test_cpus_of_node() {
        let m = AffinityMap::new_const();
        m.register_node(NumaNodeId(0), 4096).unwrap();
        m.register_cpu(CpuId(10), NumaNodeId(0)).unwrap();
        m.register_cpu(CpuId(11), NumaNodeId(0)).unwrap();
        let cpus = m.cpus_of_node(NumaNodeId(0)).unwrap();
        assert_eq!(cpus.len(), 2);
        assert!(cpus.contains(&CpuId(10)));
        assert!(cpus.contains(&CpuId(11)));
    }

    #[test]
    fn test_largest_memory_node() {
        let m = AffinityMap::new_const();
        m.register_node(NumaNodeId(0), 4096).unwrap();
        m.register_node(NumaNodeId(1), 16384).unwrap();
        let n = m.largest_memory_node().unwrap();
        assert_eq!(n, NumaNodeId(1));
    }

    #[test]
    fn test_cpu_migration_between_nodes() {
        let m = AffinityMap::new_const();
        m.register_node(NumaNodeId(0), 4096).unwrap();
        m.register_node(NumaNodeId(1), 4096).unwrap();
        m.register_cpu(CpuId(5), NumaNodeId(0)).unwrap();
        assert_eq!(m.cpu_count_of_node(NumaNodeId(0)), 1);
        // Déplacer vers nœud 1
        m.register_cpu(CpuId(5), NumaNodeId(1)).unwrap();
        assert_eq!(m.cpu_count_of_node(NumaNodeId(0)), 0);
        assert_eq!(m.cpu_count_of_node(NumaNodeId(1)), 1);
    }

    #[test]
    fn test_reset() {
        let m = AffinityMap::new_const();
        m.register_node(NumaNodeId(0), 4096).unwrap();
        m.register_cpu(CpuId(0), NumaNodeId(0)).unwrap();
        m.reset();
        assert!(m.node_of_cpu(CpuId(0)).is_none());
        assert_eq!(m.active_node_count(), 0);
    }
}
