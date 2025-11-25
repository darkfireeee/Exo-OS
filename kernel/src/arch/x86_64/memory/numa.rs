//! NUMA (Non-Uniform Memory Access) support

/// NUMA node
#[derive(Debug, Clone, Copy)]
pub struct NumaNode {
    pub id: u8,
    pub base_addr: u64,
    pub size: u64,
}

use alloc::vec::Vec;
use spin::Once;

static NUMA_NODES: Once<Vec<NumaNode>> = Once::new();
static NUMA_MAP: Once<Vec<(u64, u64, u8)>> = Once::new(); // (start, end, node_id)

/// Initialize NUMA detection
pub fn init() {
    // Parse ACPI SRAT table (stub - simplified)
    let nodes = detect_nodes_internal();
    NUMA_NODES.call_once(|| nodes);
    
    // Build address map
    let map = build_address_map();
    NUMA_MAP.call_once(|| map);
    
    log::info!("x86_64 NUMA: detected {} nodes", 
        NUMA_NODES.get().map(|n| n.len()).unwrap_or(0));
}

fn detect_nodes_internal() -> Vec<NumaNode> {
    // Simplified: create single node for UMA systems
    // Real implementation would parse ACPI SRAT
    alloc::vec![
        NumaNode {
            id: 0,
            base_addr: 0x100000,
            size: 512 * 1024 * 1024,
        }
    ]
}

fn build_address_map() -> Vec<(u64, u64, u8)> {
    // Map address ranges to NUMA nodes
    alloc::vec![
        (0x100000, 512 * 1024 * 1024, 0),
    ]
}

/// Detect NUMA nodes
pub fn detect_nodes() -> Option<&'static [NumaNode]> {
    NUMA_NODES.get().map(|v| v.as_slice())
}

/// Get NUMA node for address
pub fn get_node(addr: u64) -> Option<u8> {
    NUMA_MAP.get()?.iter()
        .find(|(start, end, _)| addr >= *start && addr < *end)
        .map(|(_, _, node_id)| *node_id)
}

/// Get local NUMA node (for current CPU)
pub fn local_node() -> u8 {
    // Read APIC ID and map to node (stub)
    let apic_id = read_apic_id();
    
    // Simplified: CPU 0-7 = node 0, CPU 8-15 = node 1, etc.
    (apic_id / 8) as u8
}

fn read_apic_id() -> u32 {
    // Simplified: return 0 (would read from APIC)
    0
}
