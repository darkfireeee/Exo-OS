//! numa_affinity.rs — Carte d'affinité CPU↔nœud NUMA pour ExoFS (no_std).

use crate::scheduler::sync::spinlock::SpinLock;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

pub static AFFINITY_MAP: AffinityMap = AffinityMap::new_const();

/// Association CPU-id → NUMA node-id.
pub struct AffinityMap {
    inner: SpinLock<AffinityInner>,
}

struct AffinityInner {
    cpu_to_node: BTreeMap<u32, u8>,
    node_to_cpus: BTreeMap<u8, Vec<u32>>,
}

impl AffinityMap {
    pub const fn new_const() -> Self {
        Self { inner: SpinLock::new(AffinityInner {
            cpu_to_node:  BTreeMap::new(),
            node_to_cpus: BTreeMap::new(),
        }) }
    }

    /// Enregistre un CPU sur un nœud NUMA.
    pub fn register_cpu(&self, cpu_id: u32, node: u8) -> Result<(), &'static str> {
        let mut g = self.inner.lock();
        g.cpu_to_node.insert(cpu_id, node);
        let cpus = g.node_to_cpus.entry(node).or_insert_with(Vec::new);
        cpus.try_reserve(1).map_err(|_| "OOM")?;
        if !cpus.contains(&cpu_id) {
            cpus.push(cpu_id);
        }
        Ok(())
    }

    /// Retourne le nœud NUMA associé à ce CPU.
    pub fn node_of_cpu(&self, cpu_id: u32) -> Option<u8> {
        self.inner.lock().cpu_to_node.get(&cpu_id).copied()
    }

    /// Retourne la liste des CPUs sur ce nœud.
    pub fn cpus_of_node(&self, node: u8) -> Vec<u32> {
        self.inner.lock().node_to_cpus.get(&node).cloned().unwrap_or_default()
    }

    pub fn n_nodes(&self) -> usize {
        self.inner.lock().node_to_cpus.len()
    }
}
