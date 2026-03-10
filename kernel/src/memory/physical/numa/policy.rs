// kernel/src/memory/physical/numa/policy.rs
//
// Politique d'allocation NUMA par processus / thread.
//
// Chaque thread possède une `NumaPolicy` stockée dans son TCB (par process/).
// Ce module définit les types de politique et les helpers d'application
// sans dépendre de process/ (inversion de dépendance via trait).
//
// Types de politique :
//   • Default        — allouer sur le nœud local du CPU courant
//   • Bind(mask)     — allouer exclusivement sur les nœuds du masque
//   • Preferred(nid) — préférer le nœud `nid`, fallback sur les autres
//   • Interleave(mask) — round-robin entre les nœuds du masque
//
// COUCHE 0 — pas de dépendance scheduler/process/ipc/fs.

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, Ordering};
use spin::RwLock;

use super::node::{NUMA_NODES, NUMA_NODE_INVALID, MAX_NUMA_NODES};
use super::distance::closest_node;

// ─────────────────────────────────────────────────────────────────────────────
// Type NumaPolicy
// ─────────────────────────────────────────────────────────────────────────────

/// Masque de nœuds NUMA (un bit par nœud, bit i = nœud i).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NumaNodeMask(pub u64);

impl NumaNodeMask {
    pub const ALL: Self = Self(u64::MAX);
    pub const NONE: Self = Self(0);

    #[inline]
    pub fn from_node(nid: u32) -> Self {
        if nid >= 64 { return Self::NONE; }
        Self(1u64 << nid)
    }

    #[inline]
    pub fn contains(&self, nid: u32) -> bool {
        if nid >= 64 { return false; }
        self.0 & (1u64 << nid) != 0
    }

    #[inline]
    pub fn is_empty(&self) -> bool { self.0 == 0 }

    /// Premier nœud du masque.
    #[inline]
    pub fn first_set(&self) -> Option<u32> {
        if self.0 == 0 { return None; }
        Some(self.0.trailing_zeros())
    }

    /// Nombre de nœuds dans le masque.
    #[inline]
    pub fn count(&self) -> u32 { self.0.count_ones() }
}

/// Politique d'allocation NUMA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumaPolicy {
    /// Allouer sur le nœud local du CPU courant.
    Default,
    /// Allouer exclusivement sur les nœuds du masque.
    /// Échec si aucun nœud du masque n'a de mémoire.
    Bind(NumaNodeMask),
    /// Préférer le nœud `nid`, fallback sur les autres par distance croissante.
    Preferred(u32),
    /// Round-robin entre les nœuds du masque.
    Interleave(NumaNodeMask),
}

impl NumaPolicy {
    /// Sélectionne le nœud cible compte tenu du CPU courant et du compteur
    /// interleave du thread.
    ///
    /// `cpu_id` : CPU courant pour déterminer le nœud local.
    /// `interleave_counter` : compteur dédié à la politique Interleave.
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
                // Parcourir les nœuds du masque par distance croissante.
                let origin = NUMA_NODES.node_for_cpu(cpu_id);
                let (sorted, len) = super::distance::NUMA_DISTANCE.sorted_nodes_from(origin);
                for i in 0..len {
                    let nid = sorted[i];
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
                // Essayer le nœud préféré d'abord.
                if let Some(node) = NUMA_NODES.get(*preferred_nid) {
                    if node.free_pages() > 0 {
                        return *preferred_nid;
                    }
                }
                // Fallback par distance depuis le nœud préféré.
                closest_node(*preferred_nid, 1)
            }
            NumaPolicy::Interleave(mask) => {
                if mask.is_empty() {
                    return NUMA_NODE_INVALID;
                }
                let count = mask.count();
                let slot = (interleave_counter % count as u64) as u32;
                // Trouver le `slot`-ième bit du masque.
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
// Politique système par défaut
// ─────────────────────────────────────────────────────────────────────────────

/// Politique du kernel (tous les threads héritent si non définie).
static SYSTEM_NUMA_POLICY: RwLock<NumaPolicy> = RwLock::new(NumaPolicy::Default);

#[inline]
pub fn get_system_policy() -> NumaPolicy {
    *SYSTEM_NUMA_POLICY.read()
}

pub fn set_system_policy(policy: NumaPolicy) {
    *SYSTEM_NUMA_POLICY.write() = policy;
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct NumaPolicyStats {
    pub default_allocs:    AtomicU64,
    pub bind_allocs:       AtomicU64,
    pub preferred_allocs:  AtomicU64,
    pub interleave_allocs: AtomicU64,
    pub bind_failures:     AtomicU64,
}

impl NumaPolicyStats {
    const fn new() -> Self {
        Self {
            default_allocs:    AtomicU64::new(0),
            bind_allocs:       AtomicU64::new(0),
            preferred_allocs:  AtomicU64::new(0),
            interleave_allocs: AtomicU64::new(0),
            bind_failures:     AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for NumaPolicyStats {}
pub static NUMA_POLICY_STATS: NumaPolicyStats = NumaPolicyStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Sélection de nœud avec tracking stats
// ─────────────────────────────────────────────────────────────────────────────

/// Sélectionne le nœud cible selon `policy` et enregistre les stats.
pub fn select_node(policy: &NumaPolicy, cpu_id: u32, interleave_ctr: u64) -> u32 {
    let nid = policy.select_node(cpu_id, interleave_ctr);
    match policy {
        NumaPolicy::Default     => NUMA_POLICY_STATS.default_allocs.fetch_add(1, Ordering::Relaxed),
        NumaPolicy::Bind(_)     => {
            if nid == NUMA_NODE_INVALID {
                NUMA_POLICY_STATS.bind_failures.fetch_add(1, Ordering::Relaxed)
            } else {
                NUMA_POLICY_STATS.bind_allocs.fetch_add(1, Ordering::Relaxed)
            }
        }
        NumaPolicy::Preferred(_)  => NUMA_POLICY_STATS.preferred_allocs.fetch_add(1, Ordering::Relaxed),
        NumaPolicy::Interleave(_) => NUMA_POLICY_STATS.interleave_allocs.fetch_add(1, Ordering::Relaxed),
    };
    nid
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait d'injection pour le scheduler
// ─────────────────────────────────────────────────────────────────────────────

/// Trait que le scheduler doit implémenter pour fournir le nœud NUMA courant.
/// Permet à memory/ de rester en Couche 0.
pub trait NumaCpuProvider {
    fn current_cpu_id(&self) -> u32;
    fn thread_interleave_counter(&self) -> u64;
    fn thread_numa_policy(&self) -> NumaPolicy;
}

/// Implémentation BSP statique (avant scheduler).
pub struct BspNumaProvider;
impl NumaCpuProvider for BspNumaProvider {
    fn current_cpu_id(&self) -> u32 { 0 }
    fn thread_interleave_counter(&self) -> u64 { 0 }
    fn thread_numa_policy(&self) -> NumaPolicy { NumaPolicy::Default }
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    // Politique système déjà Default.
}
