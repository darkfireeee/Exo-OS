// kernel/src/memory/physical/allocator/numa_aware.rs
//
// NUMA-aware allocator — buddy wrapping with local-first policy.
// Implements policies: Local-First, Interleave, Bind.
// Layer 0 — no external dependencies except `spin`.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

use crate::memory::core::{AllocError, AllocFlags, Frame};
use crate::memory::physical::allocator::numa_hints::{numa_distance, NumaNode};

// ─────────────────────────────────────────────────────────────────────────────
// NUMA ALLOCATION POLICIES
// ─────────────────────────────────────────────────────────────────────────────

/// NUMA allocation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum NumaPolicy {
    /// Allocate on local node of current CPU (default).
    #[default]
    LocalFirst = 0,
    /// Interleave round-robin across all available nodes.
    Interleave = 1,
    /// Force allocation on a specific node.
    Bind = 2,
    /// Prefer local node, fallback if unavailable.
    Preferred = 3,
}

// ─────────────────────────────────────────────────────────────────────────────
// NUMA ALLOCATION CONTEXT
// ─────────────────────────────────────────────────────────────────────────────

/// NUMA context for an allocation: policy + target node.
#[derive(Debug, Clone, Copy)]
pub struct NumaAllocContext {
    pub policy: NumaPolicy,
    /// Target node for Bind/Preferred. Ignored for LocalFirst/Interleave.
    pub bind_node: Option<NumaNode>,
    /// Allow inter-node fallback if target node is exhausted.
    pub allow_fallback: bool,
}

impl Default for NumaAllocContext {
    fn default() -> Self {
        NumaAllocContext {
            policy: NumaPolicy::LocalFirst,
            bind_node: None,
            allow_fallback: true,
        }
    }
}

impl NumaAllocContext {
    pub const fn local_first() -> Self {
        NumaAllocContext {
            policy: NumaPolicy::LocalFirst,
            bind_node: None,
            allow_fallback: true,
        }
    }

    pub fn bind(node: NumaNode) -> Self {
        NumaAllocContext {
            policy: NumaPolicy::Bind,
            bind_node: Some(node),
            allow_fallback: false,
        }
    }

    pub fn preferred(node: NumaNode) -> Self {
        NumaAllocContext {
            policy: NumaPolicy::Preferred,
            bind_node: Some(node),
            allow_fallback: true,
        }
    }

    pub const fn interleave() -> Self {
        NumaAllocContext {
            policy: NumaPolicy::Interleave,
            bind_node: None,
            allow_fallback: true,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NUMA STATISTICS
// ─────────────────────────────────────────────────────────────────────────────

pub struct NumaStats {
    pub local_allocs: [AtomicU64; NumaNode::MAX_NODES],
    pub remote_allocs: [AtomicU64; NumaNode::MAX_NODES],
    pub fallback_count: AtomicU64,
    pub bind_failures: AtomicU64,
    pub interleave_idx: AtomicU8,
}

impl Default for NumaStats {
    fn default() -> Self {
        Self::new()
    }
}

impl NumaStats {
    pub const fn new() -> Self {
        NumaStats {
            local_allocs: [const { AtomicU64::new(0) }; NumaNode::MAX_NODES],
            remote_allocs: [const { AtomicU64::new(0) }; NumaNode::MAX_NODES],
            fallback_count: AtomicU64::new(0),
            bind_failures: AtomicU64::new(0),
            interleave_idx: AtomicU8::new(0),
        }
    }
}

pub static NUMA_STATS: NumaStats = NumaStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// NUMA-AWARE ALLOCATOR
// ─────────────────────────────────────────────────────────────────────────────

/// Abstract trait for a physical page allocator.
/// Allows NumaAllocator to delegate without direct dependency on buddy.
pub trait PageAllocator: Sync {
    fn alloc_on_node(
        &self,
        order: u8,
        node: NumaNode,
        flags: AllocFlags,
    ) -> Result<Frame, AllocError>;
    fn free_pages(&self, frame: Frame, order: u8);
    fn node_free_pages(&self, node: NumaNode) -> usize;
    fn num_nodes(&self) -> usize;
}

/// NUMA-aware allocator: orchestrates buddy calls according to policy.
pub struct NumaAllocator {
    inner: Mutex<NumaAllocatorInner>,
    enabled: AtomicBool,
}

struct NumaAllocatorInner {
    /// Active nodes (u8 bitmask — max 8 nodes covered).
    active_nodes: u8,
    n_nodes: usize,
}

// SAFETY: NumaAllocator is thread-safe via its internal Mutex.
unsafe impl Sync for NumaAllocator {}

impl Default for NumaAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl NumaAllocator {
    pub const fn new() -> Self {
        NumaAllocator {
            inner: Mutex::new(NumaAllocatorInner {
                active_nodes: 0x01, // node 0 active by default
                n_nodes: 1,
            }),
            enabled: AtomicBool::new(false),
        }
    }

    /// Initializes with active NUMA nodes (bitmask).
    pub fn init(&self, active_nodes_mask: u8) {
        let n = active_nodes_mask.count_ones() as usize;
        let mut inner = self.inner.lock();
        inner.active_nodes = active_nodes_mask;
        inner.n_nodes = n.min(NumaNode::MAX_NODES);
        drop(inner);
        self.enabled.store(true, Ordering::Release);
    }

    /// Allocates `2^order` physical pages according to NUMA context.
    pub fn alloc<A: PageAllocator>(
        &self,
        allocator: &A,
        order: u8,
        ctx: NumaAllocContext,
        flags: AllocFlags,
        _current_cpu: u8,
    ) -> Result<Frame, AllocError> {
        if !self.enabled.load(Ordering::Acquire) {
            // Simple non-NUMA fallback
            return allocator.alloc_on_node(order, NumaNode::LOCAL, flags);
        }

        let inner = self.inner.lock();
        let n_nodes = inner.n_nodes;
        let active_mask = inner.active_nodes;
        drop(inner);

        match ctx.policy {
            NumaPolicy::LocalFirst => {
                // Local allocation — current CPU node by default
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
                    self.alloc_with_fallback(
                        allocator,
                        order,
                        node,
                        flags,
                        active_mask,
                        n_nodes,
                        true,
                    )
                }
            }
            NumaPolicy::Preferred => {
                let node = ctx.bind_node.unwrap_or(NumaNode::LOCAL);
                self.alloc_with_fallback(allocator, order, node, flags, active_mask, n_nodes, true)
            }
            NumaPolicy::Interleave => {
                let idx = NUMA_STATS.interleave_idx.fetch_add(1, Ordering::Relaxed);
                let node = NumaNode::new(idx % n_nodes as u8);
                self.alloc_with_fallback(allocator, order, node, flags, active_mask, n_nodes, true)
            }
        }
    }

    /// Allocates on `preferred`, then attempts other nodes by increasing distance.
    #[allow(clippy::too_many_arguments)]
    fn alloc_with_fallback<A: PageAllocator>(
        &self,
        allocator: &A,
        order: u8,
        preferred: NumaNode,
        flags: AllocFlags,
        active_mask: u8,
        _n_nodes: usize,
        do_fallback: bool,
    ) -> Result<Frame, AllocError> {
        // Attempt 1: preferred node
        if let Ok(frame) = allocator.alloc_on_node(order, preferred, flags) {
            NUMA_STATS.local_allocs[preferred.as_usize()].fetch_add(1, Ordering::Relaxed);
            return Ok(frame);
        }
        if !do_fallback {
            return Err(AllocError::OutOfMemory);
        }
        NUMA_STATS.fallback_count.fetch_add(1, Ordering::Relaxed);

        // Attempt 2: other nodes by increasing distance
        // Build an array sorted by distance
        let mut ordered: [NumaNode; NumaNode::MAX_NODES] = [NumaNode::new(0); NumaNode::MAX_NODES];
        let mut distances: [u8; NumaNode::MAX_NODES] = [u8::MAX; NumaNode::MAX_NODES];
        let mut count = 0usize;

        for node_id in 0..NumaNode::MAX_NODES as u8 {
            if (active_mask >> node_id) & 1 == 0 {
                continue;
            }
            if NumaNode::new(node_id) == preferred {
                continue;
            }
            ordered[count] = NumaNode::new(node_id);
            distances[count] = numa_distance(preferred, NumaNode::new(node_id));
            count += 1;
        }

        // Bubble sort for count <= 8
        for i in 0..count {
            for j in 0..count - 1 - i {
                if distances[j] > distances[j + 1] {
                    distances.swap(j, j + 1);
                    ordered.swap(j, j + 1);
                }
            }
        }

        for &node in &ordered[..count] {
            if let Ok(frame) = allocator.alloc_on_node(order, node, flags) {
                NUMA_STATS.remote_allocs[node.as_usize()].fetch_add(1, Ordering::Relaxed);
                return Ok(frame);
            }
        }

        Err(AllocError::OutOfMemory)
    }
}

/// Maximum number of NUMA policy slots per CPU.
const MAX_NUMA_POLICY_CPUS: usize = 256;

/// Current NUMA policy per CPU.
///
/// A single global policy leaked one CPU's preference to
/// other CPUs. Threads can still explicitly override via
/// `NumaAllocContext`, but the implicit context is now local to the CPU.
static CURRENT_POLICY_PER_CPU: [AtomicU8; MAX_NUMA_POLICY_CPUS] =
    [const { AtomicU8::new(NumaPolicy::LocalFirst as u8) }; MAX_NUMA_POLICY_CPUS];

#[inline]
fn current_policy_cpu_slot() -> usize {
    #[cfg(target_os = "none")]
    {
        let cpu = crate::arch::x86_64::smp::percpu::current_cpu_id() as usize;
        cpu.min(MAX_NUMA_POLICY_CPUS - 1)
    }

    #[cfg(not(target_os = "none"))]
    {
        0
    }
}

pub fn set_current_policy(policy: NumaPolicy) {
    CURRENT_POLICY_PER_CPU[current_policy_cpu_slot()].store(policy as u8, Ordering::Release);
}

pub fn get_current_policy() -> NumaPolicy {
    match CURRENT_POLICY_PER_CPU[current_policy_cpu_slot()].load(Ordering::Acquire) {
        0 => NumaPolicy::LocalFirst,
        1 => NumaPolicy::Interleave,
        2 => NumaPolicy::Bind,
        3 => NumaPolicy::Preferred,
        _ => NumaPolicy::LocalFirst,
    }
}

/// Global NUMA-aware allocator.
pub static NUMA_ALLOCATOR: NumaAllocator = NumaAllocator::new();
