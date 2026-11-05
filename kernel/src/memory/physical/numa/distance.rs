// kernel/src/memory/physical/numa/distance.rs
//
// NUMA distance table — access cost between nodes.
//
// The table is derived from the ACPI SLIT (System Locality Information Table).
// In the absence of SLIT, the table is initialized with default values:
//   • distance(i, i) = 10 (local)
//   • distance(i, j) = 20 (remote, 1 hop horizon)
//
// AI-KERNEL-01 RULE: the table is stored in .rodata-like via a static;
// no runtime generation of allocation policy.
//
// Layer 0 — no scheduler/process/ipc/fs dependency.

use core::sync::atomic::AtomicBool;

use super::node::{MAX_NUMA_NODES, NUMA_NODES};

// ─────────────────────────────────────────────────────────────────────────────
// Distance constants
// ─────────────────────────────────────────────────────────────────────────────

/// Local distance (same node) per standard NUMA ACPI.
pub const NUMA_DISTANCE_LOCAL: u8 = 10;
/// Remote distance (1 hop) by default.
pub const NUMA_DISTANCE_REMOTE: u8 = 20;
/// Maximum realistic distance (2 hops on a ring topology).
pub const NUMA_DISTANCE_FAR: u8 = 40;
/// Sentinel value: unreachable node.
pub const NUMA_DISTANCE_UNREACHABLE: u8 = u8::MAX;

// ─────────────────────────────────────────────────────────────────────────────
// Distance table
// ─────────────────────────────────────────────────────────────────────────────

/// Symmetric MAX_NUMA_NODES × MAX_NUMA_NODES table of uint8 distances.
pub struct NumaDistanceTable {
    /// Values: `dist[from][to]`.
    dist: [[u8; MAX_NUMA_NODES]; MAX_NUMA_NODES],
    #[allow(dead_code)]
    initialized: AtomicBool,
}

impl NumaDistanceTable {
    /// Constructs table with default values.
    const fn default_init() -> Self {
        // Initialization: local=10, remote=20.
        let mut dist = [[NUMA_DISTANCE_REMOTE; MAX_NUMA_NODES]; MAX_NUMA_NODES];
        let mut i = 0;
        while i < MAX_NUMA_NODES {
            dist[i][i] = NUMA_DISTANCE_LOCAL;
            i += 1;
        }
        Self {
            dist,
            initialized: AtomicBool::new(true),
        }
    }

    /// Returns the distance between `from` and `to`.
    #[inline]
    pub fn get(&self, from: u32, to: u32) -> u8 {
        if from as usize >= MAX_NUMA_NODES || to as usize >= MAX_NUMA_NODES {
            return NUMA_DISTANCE_UNREACHABLE;
        }
        self.dist[from as usize][to as usize]
    }

    /// Updates distance (from ACPI SLIT parser).
    ///
    /// # Safety
    /// Called during single-threaded initialization.
    pub unsafe fn set(&mut self, from: u32, to: u32, distance: u8) {
        if (from as usize) < MAX_NUMA_NODES && (to as usize) < MAX_NUMA_NODES {
            self.dist[from as usize][to as usize] = distance;
            self.dist[to as usize][from as usize] = distance; // symmetry
        }
    }

    /// Returns the identifier of the closest node to `origin`
    /// among active nodes having at least `min_free_pages` free pages.
    pub fn closest_node_with_memory(&self, origin: u32, min_free_pages: u64) -> u32 {
        let node_count = NUMA_NODES.count();
        let mut best_dist = NUMA_DISTANCE_UNREACHABLE;
        let mut best_node = super::node::NUMA_NODE_INVALID;

        for nid in 0..node_count {
            if let Some(node) = NUMA_NODES.get(nid) {
                if node.free_pages() >= min_free_pages {
                    let d = self.get(origin, nid);
                    if d < best_dist {
                        best_dist = d;
                        best_node = nid;
                    }
                }
            }
        }
        best_node
    }

    /// Returns sorted list of nodes by increasing distance from `origin`.
    /// Returns an array of (node_id, distance) and its valid length.
    pub fn sorted_nodes_from(&self, origin: u32) -> ([u32; MAX_NUMA_NODES], usize) {
        let node_count = NUMA_NODES.count() as usize;
        let mut ids: [u32; MAX_NUMA_NODES] = [super::node::NUMA_NODE_INVALID; MAX_NUMA_NODES];
        let mut dists: [u8; MAX_NUMA_NODES] = [NUMA_DISTANCE_UNREACHABLE; MAX_NUMA_NODES];
        let mut len = 0;

        for nid in 0..node_count {
            if NUMA_NODES.get(nid as u32).is_some() {
                ids[len] = nid as u32;
                dists[len] = self.get(origin, nid as u32);
                len += 1;
            }
        }

        // Bubble sort (len <= 8 -> O(n²) acceptable).
        for i in 0..len {
            for j in i + 1..len {
                if dists[j] < dists[i] {
                    ids.swap(i, j);
                    dists.swap(i, j);
                }
            }
        }

        (ids, len)
    }
}

unsafe impl Sync for NumaDistanceTable {}

/// Global static distance table.
pub static NUMA_DISTANCE: NumaDistanceTable = NumaDistanceTable::default_init();

// ─────────────────────────────────────────────────────────────────────────────
// Public helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Returns the distance between two NUMA nodes.
#[inline]
pub fn numa_distance(from: u32, to: u32) -> u8 {
    NUMA_DISTANCE.get(from, to)
}

/// Returns `true` if `from` and `to` are the same node (local distance).
#[inline]
pub fn numa_same_node(from: u32, to: u32) -> bool {
    from == to
}

/// Returns closest node with available memory.
#[inline]
pub fn closest_node(origin: u32, min_free: u64) -> u32 {
    NUMA_DISTANCE.closest_node_with_memory(origin, min_free)
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialization
// ─────────────────────────────────────────────────────────────────────────────

/// Init distance table — already initialized with default_init().
/// ACPI SLIT parser can call `NUMA_DISTANCE.set()` to update.
pub fn init() {
    // Table already initialized constexpr.
    // If ACPI SLIT present: call NUMA_DISTANCE.set() for each entry.
}
