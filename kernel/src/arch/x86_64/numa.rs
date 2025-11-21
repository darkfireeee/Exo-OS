//! # NUMA (Non-Uniform Memory Access) Support
//!
//! This module provides NUMA topology detection and management for x86_64.
//! NUMA systems have multiple memory nodes with different access latencies
//! depending on which CPU accesses which node.

use crate::memory::PhysicalAddress;

/// NUMA node ID
pub type NodeId = usize;

/// Get NUMA node for a given physical address
pub fn get_numa_node(_addr: PhysicalAddress) -> NodeId {
    // TODO: Implement actual NUMA detection via ACPI SRAT
    // For now, return node 0 (single node system)
    0
}

/// Get total number of NUMA nodes
pub fn node_count() -> usize {
    // TODO: Detect actual node count from ACPI SRAT
    1
}

/// Get NUMA node for current CPU
pub fn current_node() -> NodeId {
    // TODO: Map CPU to NUMA node
    0
}

/// Get distance between two NUMA nodes (lower = faster)
pub fn node_distance(_from: NodeId, _to: NodeId) -> u8 {
    // TODO: Implement SLIT table parsing
    10 // Default local access distance
}

/// Check if NUMA is enabled on this system
pub fn is_numa_enabled() -> bool {
    // TODO: Check ACPI SRAT presence
    false
}
