// kernel/src/memory/physical/numa/node.rs
//
// NUMA nodes — descriptors, global table, and per-node counters.
//
// Each NUMA node represents a memory bank local to a set of CPUs.
// This layer maintains:
//   • The list of detected nodes (via ACPI SRAT at init).
//   • Physical address ranges belonging to each node.
//   • Free / used page counters per node.
//
// LAYER 0 — no scheduler/process/ipc/fs dependencies.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::memory::core::constants::PAGE_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of NUMA nodes.
pub const MAX_NUMA_NODES: usize = 8;
/// Maximum number of physical ranges per node.
const MAX_RANGES_PER_NODE: usize = 16;
/// Invalid / sentinel ID.
pub const NUMA_NODE_INVALID: u32 = u32::MAX;

// ─────────────────────────────────────────────────────────────────────────────
// Per-node statistics
// ─────────────────────────────────────────────────────────────────────────────

/// Live counters per NUMA node.
#[repr(C, align(64))]
pub struct NumaNodeStats {
    /// Total pages in this node.
    pub total_pages: AtomicU64,
    /// Currently free pages.
    pub free_pages: AtomicU64,
    /// Used pages (total - free).
    pub used_pages: AtomicU64,
    /// Successful local allocations.
    pub local_allocs: AtomicU64,
    /// Remote allocations (fallback).
    pub remote_allocs: AtomicU64,
    /// Pages migrated into this node.
    pub migrated_in: AtomicU64,
    /// Pages migrated out of this node.
    pub migrated_out: AtomicU64,
    _pad: [u8; 8],
}

impl NumaNodeStats {
    const fn new() -> Self {
        Self {
            total_pages: AtomicU64::new(0),
            free_pages: AtomicU64::new(0),
            used_pages: AtomicU64::new(0),
            local_allocs: AtomicU64::new(0),
            remote_allocs: AtomicU64::new(0),
            migrated_in: AtomicU64::new(0),
            migrated_out: AtomicU64::new(0),
            _pad: [0; 8],
        }
    }

    #[inline]
    pub fn record_alloc_local(&self) {
        self.local_allocs.fetch_add(1, Ordering::Relaxed);
        self.free_pages.fetch_sub(1, Ordering::Relaxed);
        self.used_pages.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_alloc_remote(&self) {
        self.remote_allocs.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_free(&self) {
        self.free_pages.fetch_add(1, Ordering::Relaxed);
        self.used_pages.fetch_sub(1, Ordering::Relaxed);
    }
}

unsafe impl Sync for NumaNodeStats {}

// ─────────────────────────────────────────────────────────────────────────────
// Physical range belonging to a node
// ─────────────────────────────────────────────────────────────────────────────

/// A physical address range belonging to a NUMA node.
#[derive(Debug, Clone, Copy)]
pub struct NumaPhysRange {
    pub start: u64, // inclusive
    pub end: u64,   // exclusive
}

impl NumaPhysRange {
    pub const fn invalid() -> Self {
        Self { start: 0, end: 0 }
    }
    pub fn is_valid(&self) -> bool {
        self.end > self.start
    }
    pub fn size_bytes(&self) -> u64 {
        self.end - self.start
    }
    pub fn size_pages(&self) -> u64 {
        self.size_bytes() / PAGE_SIZE as u64
    }
    pub fn contains_phys(&self, phys: u64) -> bool {
        phys >= self.start && phys < self.end
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NUMA node descriptor
// ─────────────────────────────────────────────────────────────────────────────

/// Full descriptor of a NUMA node.
#[repr(C, align(64))]
pub struct NumaNode {
    /// Unique node identifier (0..MAX_NUMA_NODES-1).
    pub id: u32,
    /// Mask of CPUs local to this node (bit i = CPU i).
    pub cpu_mask: AtomicU64,
    /// Number of valid physical ranges in `ranges`.
    pub range_count: u32,
    /// Active (detected at ACPI SRAT init).
    pub active: AtomicBool,
    _pad1: [u8; 3],
    /// Live statistics.
    pub stats: NumaNodeStats,
    /// Physical ranges of this node.
    pub ranges: [NumaPhysRange; MAX_RANGES_PER_NODE],
    _pad2: [u8; 16],
}

impl NumaNode {
    const fn new(id: u32) -> Self {
        Self {
            id,
            cpu_mask: AtomicU64::new(0),
            range_count: 0,
            active: AtomicBool::new(false),
            _pad1: [0; 3],
            stats: NumaNodeStats::new(),
            ranges: [NumaPhysRange { start: 0, end: 0 }; MAX_RANGES_PER_NODE],
            _pad2: [0; 16],
        }
    }

    /// Adds a physical range. Returns `false` if table is full.
    pub fn add_range(&mut self, start: u64, end: u64) -> bool {
        if self.range_count as usize >= MAX_RANGES_PER_NODE {
            return false;
        }
        self.ranges[self.range_count as usize] = NumaPhysRange { start, end };
        self.range_count += 1;
        let new_pages = (end - start) / PAGE_SIZE as u64;
        self.stats
            .total_pages
            .fetch_add(new_pages, Ordering::Relaxed);
        self.stats
            .free_pages
            .fetch_add(new_pages, Ordering::Relaxed);
        true
    }

    /// Returns `true` if `phys` belongs to this node.
    pub fn owns_phys(&self, phys: u64) -> bool {
        for i in 0..self.range_count as usize {
            if self.ranges[i].contains_phys(phys) {
                return true;
            }
        }
        false
    }

    /// Total free pages on this node.
    #[inline]
    pub fn free_pages(&self) -> u64 {
        self.stats.free_pages.load(Ordering::Relaxed)
    }

    /// Total pages on this node.
    #[inline]
    pub fn total_pages(&self) -> u64 {
        self.stats.total_pages.load(Ordering::Relaxed)
    }
}

unsafe impl Sync for NumaNode {}

// ─────────────────────────────────────────────────────────────────────────────
// Global node table
// ─────────────────────────────────────────────────────────────────────────────

/// Central table of all detected NUMA nodes.
pub struct NumaNodeTable {
    nodes: [NumaNode; MAX_NUMA_NODES],
    node_count: AtomicU32,
}

impl NumaNodeTable {
    const fn new() -> Self {
        Self {
            nodes: [
                NumaNode::new(0),
                NumaNode::new(1),
                NumaNode::new(2),
                NumaNode::new(3),
                NumaNode::new(4),
                NumaNode::new(5),
                NumaNode::new(6),
                NumaNode::new(7),
            ],
            node_count: AtomicU32::new(0),
        }
    }

    /// Registers a NUMA node.
    /// `cpu_mask`: bitmask of local CPUs.
    /// Returns node id or `NUMA_NODE_INVALID` if table is full.
    pub fn register_node(&self, cpu_mask: u64) -> u32 {
        let id = self.node_count.fetch_add(1, Ordering::AcqRel);
        if id as usize >= MAX_NUMA_NODES {
            self.node_count.fetch_sub(1, Ordering::Relaxed);
            return NUMA_NODE_INVALID;
        }
        // SAFETY: accessed via unique id (atomic counter), no race.
        let node = unsafe { &mut *(core::ptr::addr_of!(self.nodes[id as usize]) as *mut NumaNode) };
        node.cpu_mask.store(cpu_mask, Ordering::Release);
        node.active.store(true, Ordering::Release);
        id
    }

    /// Adds a physical range to node `id`.
    pub fn add_range(&self, id: u32, start: u64, end: u64) -> bool {
        if id as usize >= MAX_NUMA_NODES {
            return false;
        }
        // SAFETY: id < MAX_NUMA_NODES verified above, addr_of! avoids &T->*mut T.
        // Access is single-threaded during SRAT initialization.
        let node = unsafe { &mut *(core::ptr::addr_of!(self.nodes[id as usize]) as *mut NumaNode) };
        node.add_range(start, end)
    }

    /// Returns node owning `phys` or `NUMA_NODE_INVALID`.
    pub fn node_for_phys(&self, phys: u64) -> u32 {
        let count = self.node_count.load(Ordering::Acquire) as usize;
        for i in 0..count {
            if self.nodes[i].active.load(Ordering::Relaxed) && self.nodes[i].owns_phys(phys) {
                return i as u32;
            }
        }
        NUMA_NODE_INVALID
    }

    /// Returns local node for `cpu_id` (first node whose mask includes cpu).
    pub fn node_for_cpu(&self, cpu_id: u32) -> u32 {
        let count = self.node_count.load(Ordering::Acquire) as usize;
        let bit = 1u64 << (cpu_id & 63);
        for i in 0..count {
            if self.nodes[i].active.load(Ordering::Relaxed)
                && self.nodes[i].cpu_mask.load(Ordering::Relaxed) & bit != 0
            {
                return i as u32;
            }
        }
        0 // Fallback node 0
    }

    /// Access to a node by id.
    #[inline]
    pub fn get(&self, id: u32) -> Option<&NumaNode> {
        if id as usize >= MAX_NUMA_NODES {
            return None;
        }
        let n = &self.nodes[id as usize];
        if n.active.load(Ordering::Relaxed) {
            Some(n)
        } else {
            None
        }
    }

    /// Number of active nodes.
    #[inline]
    pub fn count(&self) -> u32 {
        self.node_count.load(Ordering::Acquire)
    }
}

unsafe impl Sync for NumaNodeTable {}

pub static NUMA_NODES: NumaNodeTable = NumaNodeTable::new();

// ─────────────────────────────────────────────────────────────────────────────
// NUMA global statistics
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct NumaGlobalStats {
    pub total_nodes: AtomicU32,
    pub fallback_allocs: AtomicU64,
    pub migration_ops: AtomicU64,
}

impl NumaGlobalStats {
    const fn new() -> Self {
        Self {
            total_nodes: AtomicU32::new(0),
            fallback_allocs: AtomicU64::new(0),
            migration_ops: AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for NumaGlobalStats {}
pub static NUMA_GLOBAL_STATS: NumaGlobalStats = NumaGlobalStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Initialization
// ─────────────────────────────────────────────────────────────────────────────

/// Default NUMA init (UMA): a single node with all CPUs + total range.
///
/// In a real NUMA system, this function is replaced by the ACPI SRAT parser
/// which calls `NUMA_NODES.register_node()` + `add_range()` for each
/// discovered node.
///
/// # Safety
/// Callable only in CPL 0 during kernel initialization.
pub unsafe fn init() {
    // Node 0: all CPUs, range 0..4GiB by default.
    let nid = NUMA_NODES.register_node(u64::MAX);
    NUMA_NODES.add_range(nid, 0x0000_0000, 0x1_0000_0000);
    NUMA_GLOBAL_STATS.total_nodes.store(1, Ordering::Relaxed);
}
