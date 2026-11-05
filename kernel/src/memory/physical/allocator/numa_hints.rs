// kernel/src/memory/physical/allocator/numa_hints.rs
//
// NUMA types and topology — boot data, no AI heuristics.
// Layer 0 — no external dependencies.
//
// This module replaces the old ai_hints.rs. It retains only:
//   • NumaNode / SizeClass : fundamental NUMA types
//   • NUMA_DISTANCE_TABLE   : inter-node distances (ACPI SLIT)
//   • CPU_TO_NUMA + TOPO_READY : CPU->node mapping provided by ACPI/SRAT at boot
//   • numa_distance / cpu_numa_node / set_numa_topology : public API
//
// What was removed compared to ai_hints.rs:
//   • NUMA_HINT_TABLE (per-cpu/size-class lookup table -> AI heuristic)
//   • hint_numa_node() (AI request)
//   • HINTS_ENABLED / HintStats / HINT_STATS / set_hints_enabled() / stats()

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// NUMA TYPES
// ─────────────────────────────────────────────────────────────────────────────

/// NUMA node identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct NumaNode(u8);

impl NumaNode {
    pub const MAX_NODES: usize = 8;

    #[inline]
    pub const fn new(id: u8) -> Self {
        NumaNode(id)
    }
    #[inline]
    pub const fn id(self) -> u8 {
        self.0
    }
    #[inline]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
    pub const LOCAL: NumaNode = NumaNode(0);
}

// ─────────────────────────────────────────────────────────────────────────────
// SIZE CLASS
// ─────────────────────────────────────────────────────────────────────────────

/// Allocation size class (index 0..=N_SIZE_CLASSES-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SizeClass {
    /// 4 KiB  (order 0)
    Page0 = 0,
    /// 8 KiB  (order 1)
    Page1 = 1,
    /// 16 KiB (order 2)
    Page2 = 2,
    /// 32 KiB (order 3)
    Page3 = 3,
    /// 64 KiB (order 4)
    Page4 = 4,
    /// 128 KiB (order 5)
    Page5 = 5,
    /// 256 KiB (order 6)
    Page6 = 6,
    /// 512 KiB (order 7)
    Page7 = 7,
    /// 1 MiB  (order 8)
    Page8 = 8,
    /// 2 MiB  (order 9)
    Page9 = 9,
    /// 4 MiB  (order 10)
    Page10 = 10,
    /// 8 MiB  (order 11)
    Page11 = 11,
    /// 16 MiB (order 12)
    Page12 = 12,
}

impl SizeClass {
    pub const COUNT: usize = 13;

    #[inline]
    pub fn from_order(order: u8) -> Option<SizeClass> {
        match order {
            0 => Some(SizeClass::Page0),
            1 => Some(SizeClass::Page1),
            2 => Some(SizeClass::Page2),
            3 => Some(SizeClass::Page3),
            4 => Some(SizeClass::Page4),
            5 => Some(SizeClass::Page5),
            6 => Some(SizeClass::Page6),
            7 => Some(SizeClass::Page7),
            8 => Some(SizeClass::Page8),
            9 => Some(SizeClass::Page9),
            10 => Some(SizeClass::Page10),
            11 => Some(SizeClass::Page11),
            12 => Some(SizeClass::Page12),
            _ => None,
        }
    }

    #[inline]
    pub fn as_order(self) -> u8 {
        self as u8
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// NUMA DISTANCE TABLE
// ─────────────────────────────────────────────────────────────────────────────

/// distance[src][dst] -> relative cost (1=local, 2=1 hop, 3=2 hops, 4=very far).
/// Based on ACPI SLIT values normalized to [1..=4].
static NUMA_DISTANCE_TABLE: [[u8; NumaNode::MAX_NODES]; NumaNode::MAX_NODES] = [
    // Node 0    1    2    3    4    5    6    7
    [1, 2, 3, 3, 4, 4, 4, 4], // From 0
    [2, 1, 2, 3, 4, 4, 3, 4], // From 1
    [3, 2, 1, 2, 4, 3, 4, 4], // From 2
    [3, 3, 2, 1, 4, 4, 2, 3], // From 3
    [4, 4, 4, 4, 1, 2, 3, 3], // From 4
    [4, 4, 3, 4, 2, 1, 2, 3], // From 5
    [4, 3, 4, 2, 3, 2, 1, 2], // From 6
    [4, 4, 4, 3, 3, 3, 2, 1], // From 7
];

// ─────────────────────────────────────────────────────────────────────────────
// TOPOLOGY (boot data, initialized once)
// ─────────────────────────────────────────────────────────────────────────────

/// CPU -> NUMA node mapping populated at boot by ACPI/SRAT.
/// LOCK-FREE: safe read as soon as `TOPO_READY` = true.
static CPU_TO_NUMA: [AtomicU8; 256] = [const { AtomicU8::new(0) }; 256];

static TOPO_READY: AtomicBool = AtomicBool::new(false);

// ─────────────────────────────────────────────────────────────────────────────
// PUBLIC API
// ─────────────────────────────────────────────────────────────────────────────

/// Returns NUMA distance between two nodes (1=local, ..., 4=very far).
#[inline]
pub fn numa_distance(from: NumaNode, to: NumaNode) -> u8 {
    let f = from.as_usize().min(NumaNode::MAX_NODES - 1);
    let t = to.as_usize().min(NumaNode::MAX_NODES - 1);
    NUMA_DISTANCE_TABLE[f][t]
}

/// Returns NUMA node currently assigned to this CPU.
#[inline]
pub fn cpu_numa_node(cpu: u8) -> NumaNode {
    let node = CPU_TO_NUMA[cpu as usize].load(Ordering::Relaxed);
    NumaNode::new(node)
}

/// Registers actual NUMA topology (called EXACTLY ONCE by ACPI/SRAT parser).
///
/// # Safety
/// Must be called before any multi-CPU access (during single-CPU init).
pub unsafe fn set_numa_topology(cpu_to_node: &[u8]) {
    debug_assert!(
        !TOPO_READY.load(Ordering::Relaxed),
        "set_numa_topology appelé plusieurs fois"
    );
    for (cpu, &node) in cpu_to_node.iter().enumerate() {
        if cpu >= 256 {
            break;
        }
        CPU_TO_NUMA[cpu].store(node.min(NumaNode::MAX_NODES as u8 - 1), Ordering::Relaxed);
    }
    TOPO_READY.store(true, Ordering::Release);
}
