// kernel/src/memory/physical/allocator/numa_aware.rs
//
// Allocateur NUMA-aware — wrapping du buddy avec politique locale-first.
// Implémente les politiques : Local-First, Interleave, Bind.
// Couche 0 — aucune dépendance externe sauf `spin`.

use core::sync::atomic::{AtomicU8, AtomicU64, AtomicBool, Ordering};
use spin::Mutex;

use crate::memory::core::{AllocError, AllocFlags, Frame};
use crate::memory::physical::allocator::numa_hints::{NumaNode, numa_distance};

// ─────────────────────────────────────────────────────────────────────────────
// POLITIQUES D'ALLOCATION NUMA
// ─────────────────────────────────────────────────────────────────────────────

/// Politique d'allocation NUMA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NumaPolicy {
    /// Allouer sur le nœud local du CPU courant (défaut).
    LocalFirst = 0,
    /// Interleave round-robin entre tous les nœuds disponibles.
    Interleave = 1,
    /// Forcer une allocation sur un nœud spécifique.
    Bind       = 2,
    /// Préférer le nœud local, tomber en fallback si indisponible.
    Preferred  = 3,
}

impl Default for NumaPolicy {
    fn default() -> Self { NumaPolicy::LocalFirst }
}

// ─────────────────────────────────────────────────────────────────────────────
// CONTEXTE D'ALLOCATION NUMA
// ─────────────────────────────────────────────────────────────────────────────

/// Contexte NUMA pour une allocation : politique + nœud cible.
#[derive(Debug, Clone, Copy)]
pub struct NumaAllocContext {
    pub policy:    NumaPolicy,
    /// Nœud cible pour Bind/Preferred. Ignoré pour LocalFirst/Interleave.
    pub bind_node: Option<NumaNode>,
    /// Autoriser le fallback inter-nœuds si le nœud cible est épuisé.
    pub allow_fallback: bool,
}

impl Default for NumaAllocContext {
    fn default() -> Self {
        NumaAllocContext {
            policy:         NumaPolicy::LocalFirst,
            bind_node:      None,
            allow_fallback: true,
        }
    }
}

impl NumaAllocContext {
    pub const fn local_first() -> Self {
        NumaAllocContext {
            policy:         NumaPolicy::LocalFirst,
            bind_node:      None,
            allow_fallback: true,
        }
    }

    pub fn bind(node: NumaNode) -> Self {
        NumaAllocContext {
            policy:         NumaPolicy::Bind,
            bind_node:      Some(node),
            allow_fallback: false,
        }
    }

    pub fn preferred(node: NumaNode) -> Self {
        NumaAllocContext {
            policy:         NumaPolicy::Preferred,
            bind_node:      Some(node),
            allow_fallback: true,
        }
    }

    pub const fn interleave() -> Self {
        NumaAllocContext {
            policy:         NumaPolicy::Interleave,
            bind_node:      None,
            allow_fallback: true,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES NUMA
// ─────────────────────────────────────────────────────────────────────────────

pub struct NumaStats {
    pub local_allocs:   [AtomicU64; NumaNode::MAX_NODES],
    pub remote_allocs:  [AtomicU64; NumaNode::MAX_NODES],
    pub fallback_count: AtomicU64,
    pub bind_failures:  AtomicU64,
    pub interleave_idx: AtomicU8,
}

impl NumaStats {
    pub const fn new() -> Self {
        NumaStats {
            local_allocs:   {
                const ZERO: AtomicU64 = AtomicU64::new(0);
                [ZERO; NumaNode::MAX_NODES]
            },
            remote_allocs:  {
                const ZERO: AtomicU64 = AtomicU64::new(0);
                [ZERO; NumaNode::MAX_NODES]
            },
            fallback_count: AtomicU64::new(0),
            bind_failures:  AtomicU64::new(0),
            interleave_idx: AtomicU8::new(0),
        }
    }
}

pub static NUMA_STATS: NumaStats = NumaStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// ALLOCATEUR NUMA-AWARE
// ─────────────────────────────────────────────────────────────────────────────

/// Trait abstrait pour un allocateur de pages physiques.
/// Permet à NumaAllocator de déléguer sans dépendance directe sur buddy.
pub trait PageAllocator: Sync {
    fn alloc_on_node(&self, order: u8, node: NumaNode, flags: AllocFlags)
        -> Result<Frame, AllocError>;
    fn free_pages(&self, frame: Frame, order: u8);
    fn node_free_pages(&self, node: NumaNode) -> usize;
    fn num_nodes(&self) -> usize;
}

/// Allocateur NUMA-aware : orchestre les appels au buddy selon la politique.
pub struct NumaAllocator {
    inner:   Mutex<NumaAllocatorInner>,
    enabled: AtomicBool,
}

struct NumaAllocatorInner {
    /// Nœuds actifs (bitmask u8 — max 8 nœuds couverts).
    active_nodes: u8,
    n_nodes:      usize,
}

// SAFETY: NumaAllocator est thread-safe via son Mutex interne.
unsafe impl Sync for NumaAllocator {}

impl NumaAllocator {
    pub const fn new() -> Self {
        NumaAllocator {
            inner:   Mutex::new(NumaAllocatorInner {
                active_nodes: 0x01, // nœud 0 actif par défaut
                n_nodes:      1,
            }),
            enabled: AtomicBool::new(false),
        }
    }

    /// Initialise avec les nœuds NUMA actifs (bitmask).
    pub fn init(&self, active_nodes_mask: u8) {
        let n = active_nodes_mask.count_ones() as usize;
        let mut inner = self.inner.lock();
        inner.active_nodes = active_nodes_mask;
        inner.n_nodes      = n.min(NumaNode::MAX_NODES);
        drop(inner);
        self.enabled.store(true, Ordering::Release);
    }

    /// Alloue `2^order` pages physiques selon le contexte NUMA.
    pub fn alloc<A: PageAllocator>(
        &self,
        allocator: &A,
        order:     u8,
        ctx:       NumaAllocContext,
        flags:     AllocFlags,
        _current_cpu: u8,
    ) -> Result<Frame, AllocError> {
        if !self.enabled.load(Ordering::Acquire) {
            // Fallback non-NUMA simple
            return allocator.alloc_on_node(order, NumaNode::LOCAL, flags);
        }

        let inner = self.inner.lock();
        let n_nodes     = inner.n_nodes;
        let active_mask = inner.active_nodes;
        drop(inner);

        match ctx.policy {
            NumaPolicy::LocalFirst => {
                // Allocation locale — nœud du CPU courant par défaut
                let node = NumaNode::LOCAL;
                self.alloc_with_fallback(allocator, order, node, flags, active_mask, n_nodes, true)
            }
            NumaPolicy::Bind => {
                let node = ctx.bind_node.unwrap_or(NumaNode::LOCAL);
                if !ctx.allow_fallback {
                    match allocator.alloc_on_node(order, node, flags) {
                        Ok(f) => {
                            NUMA_STATS.local_allocs[node.as_usize()]
                                .fetch_add(1, Ordering::Relaxed);
                            Ok(f)
                        }
                        Err(e) => {
                            NUMA_STATS.bind_failures.fetch_add(1, Ordering::Relaxed);
                            Err(e)
                        }
                    }
                } else {
                    self.alloc_with_fallback(allocator, order, node, flags, active_mask, n_nodes, true)
                }
            }
            NumaPolicy::Preferred => {
                let node = ctx.bind_node.unwrap_or(NumaNode::LOCAL);
                self.alloc_with_fallback(allocator, order, node, flags, active_mask, n_nodes, true)
            }
            NumaPolicy::Interleave => {
                let idx  = NUMA_STATS.interleave_idx.fetch_add(1, Ordering::Relaxed);
                let node = NumaNode::new(idx % n_nodes as u8);
                self.alloc_with_fallback(allocator, order, node, flags, active_mask, n_nodes, true)
            }
        }
    }

    /// Alloue sur `preferred`, puis tente les autres nœuds par distance croissante.
    fn alloc_with_fallback<A: PageAllocator>(
        &self,
        allocator:    &A,
        order:        u8,
        preferred:    NumaNode,
        flags:        AllocFlags,
        active_mask:  u8,
        _n_nodes:      usize,
        do_fallback:  bool,
    ) -> Result<Frame, AllocError> {
        // Essai 1 : nœud préféré
        if let Ok(frame) = allocator.alloc_on_node(order, preferred, flags) {
            NUMA_STATS.local_allocs[preferred.as_usize()].fetch_add(1, Ordering::Relaxed);
            return Ok(frame);
        }
        if !do_fallback {
            return Err(AllocError::OutOfMemory);
        }
        NUMA_STATS.fallback_count.fetch_add(1, Ordering::Relaxed);

        // Essai 2 : autres nœuds par distance croissante
        // Construire un tableau trié par distance
        let mut ordered: [NumaNode; NumaNode::MAX_NODES] = [NumaNode::new(0); NumaNode::MAX_NODES];
        let mut distances: [u8; NumaNode::MAX_NODES]     = [u8::MAX; NumaNode::MAX_NODES];
        let mut count = 0usize;

        for node_id in 0..NumaNode::MAX_NODES as u8 {
            if (active_mask >> node_id) & 1 == 0 { continue; }
            if NumaNode::new(node_id) == preferred { continue; }
            ordered[count]   = NumaNode::new(node_id);
            distances[count] = numa_distance(preferred, NumaNode::new(node_id));
            count += 1;
        }

        // Tri à bulles pour count <= 8
        for i in 0..count {
            for j in 0..count - 1 - i {
                if distances[j] > distances[j + 1] {
                    distances.swap(j, j + 1);
                    ordered.swap(j, j + 1);
                }
            }
        }

        for i in 0..count {
            let node = ordered[i];
            if let Ok(frame) = allocator.alloc_on_node(order, node, flags) {
                NUMA_STATS.remote_allocs[node.as_usize()].fetch_add(1, Ordering::Relaxed);
                return Ok(frame);
            }
        }

        Err(AllocError::OutOfMemory)
    }
}

/// Modifie la politique NUMA du thread courant.
/// (Stub — en production, utiliser une table par CPU.)
static CURRENT_POLICY: AtomicU8 = AtomicU8::new(NumaPolicy::LocalFirst as u8);

pub fn set_current_policy(policy: NumaPolicy) {
    CURRENT_POLICY.store(policy as u8, Ordering::SeqCst);
}

pub fn get_current_policy() -> NumaPolicy {
    match CURRENT_POLICY.load(Ordering::Relaxed) {
        0 => NumaPolicy::LocalFirst,
        1 => NumaPolicy::Interleave,
        2 => NumaPolicy::Bind,
        3 => NumaPolicy::Preferred,
        _ => NumaPolicy::LocalFirst,
    }
}

/// Allocateur NUMA-aware global.
pub static NUMA_ALLOCATOR: NumaAllocator = NumaAllocator::new();
