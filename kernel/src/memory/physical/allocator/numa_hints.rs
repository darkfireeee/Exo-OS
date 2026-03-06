// kernel/src/memory/physical/allocator/numa_hints.rs
//
// Types et topologie NUMA — données de boot, aucune heuristique IA.
// Couche 0 — aucune dépendance externe.
//
// Ce module remplace l'ancien ai_hints.rs. Il conserve uniquement :
//   • NumaNode / SizeClass : types NUMA fondamentaux
//   • NUMA_DISTANCE_TABLE   : distances inter-nœuds (ACPI SLIT)
//   • CPU_TO_NUMA + TOPO_READY : mapping CPU→nœud fourni par ACPI/SRAT au boot
//   • numa_distance / cpu_numa_node / set_numa_topology : API publique
//
// Ce qui a été retiré par rapport à ai_hints.rs :
//   • NUMA_HINT_TABLE (table de lookup per-cpu/size-class → heuristique AI)
//   • hint_numa_node() (requête IA)
//   • HINTS_ENABLED / HintStats / HINT_STATS / set_hints_enabled() / stats()

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// TYPES NUMA
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant d'un nœud NUMA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct NumaNode(u8);

impl NumaNode {
    pub const MAX_NODES: usize = 8;

    #[inline] pub const fn new(id: u8) -> Self { NumaNode(id) }
    #[inline] pub const fn id(self)    -> u8   { self.0 }
    #[inline] pub const fn as_usize(self) -> usize { self.0 as usize }
    pub const LOCAL: NumaNode = NumaNode(0);
}

// ─────────────────────────────────────────────────────────────────────────────
// CLASSE DE TAILLE
// ─────────────────────────────────────────────────────────────────────────────

/// Classe de taille d'allocation (index 0..=N_SIZE_CLASSES-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SizeClass {
    /// 4 KiB  (ordre 0)
    Page0  = 0,
    /// 8 KiB  (ordre 1)
    Page1  = 1,
    /// 16 KiB (ordre 2)
    Page2  = 2,
    /// 32 KiB (ordre 3)
    Page3  = 3,
    /// 64 KiB (ordre 4)
    Page4  = 4,
    /// 128 KiB (ordre 5)
    Page5  = 5,
    /// 256 KiB (ordre 6)
    Page6  = 6,
    /// 512 KiB (ordre 7)
    Page7  = 7,
    /// 1 MiB  (ordre 8)
    Page8  = 8,
    /// 2 MiB  (ordre 9)
    Page9  = 9,
    /// 4 MiB  (ordre 10)
    Page10 = 10,
    /// 8 MiB  (ordre 11)
    Page11 = 11,
    /// 16 MiB (ordre 12)
    Page12 = 12,
}

impl SizeClass {
    pub const COUNT: usize = 13;

    #[inline]
    pub fn from_order(order: u8) -> Option<SizeClass> {
        match order {
            0  => Some(SizeClass::Page0),
            1  => Some(SizeClass::Page1),
            2  => Some(SizeClass::Page2),
            3  => Some(SizeClass::Page3),
            4  => Some(SizeClass::Page4),
            5  => Some(SizeClass::Page5),
            6  => Some(SizeClass::Page6),
            7  => Some(SizeClass::Page7),
            8  => Some(SizeClass::Page8),
            9  => Some(SizeClass::Page9),
            10 => Some(SizeClass::Page10),
            11 => Some(SizeClass::Page11),
            12 => Some(SizeClass::Page12),
            _  => None,
        }
    }

    #[inline] pub fn as_order(self) -> u8 { self as u8 }
}

// ─────────────────────────────────────────────────────────────────────────────
// TABLE DE DISTANCE NUMA
// ─────────────────────────────────────────────────────────────────────────────

/// distance[src][dst] → coût relatif (1=local, 2=1 hop, 3=2 hops, 4=très éloigné).
/// Basée sur les valeurs ACPI SLIT normalisées à [1..=4].
static NUMA_DISTANCE_TABLE: [[u8; NumaNode::MAX_NODES]; NumaNode::MAX_NODES] = [
    // Node 0    1    2    3    4    5    6    7
    [1,   2,   3,   3,   4,   4,   4,   4],  // From 0
    [2,   1,   2,   3,   4,   4,   3,   4],  // From 1
    [3,   2,   1,   2,   4,   3,   4,   4],  // From 2
    [3,   3,   2,   1,   4,   4,   2,   3],  // From 3
    [4,   4,   4,   4,   1,   2,   3,   3],  // From 4
    [4,   4,   3,   4,   2,   1,   2,   3],  // From 5
    [4,   3,   4,   2,   3,   2,   1,   2],  // From 6
    [4,   4,   4,   3,   3,   3,   2,   1],  // From 7
];

// ─────────────────────────────────────────────────────────────────────────────
// TOPOLOGIE (donnée de boot, initiée une seule fois)
// ─────────────────────────────────────────────────────────────────────────────

/// Mapping CPU → nœud NUMA renseigné au boot par ACPI/SRAT.
/// LOCK-FREE : lecture sûre dès que `TOPO_READY` = true.
static CPU_TO_NUMA: [AtomicU8; 256] = {
    const INIT: AtomicU8 = AtomicU8::new(0);
    [INIT; 256]
};

static TOPO_READY: AtomicBool = AtomicBool::new(false);

// ─────────────────────────────────────────────────────────────────────────────
// API PUBLIQUE
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne la distance NUMA entre deux nœuds (1=local, ..., 4=très éloigné).
#[inline]
pub fn numa_distance(from: NumaNode, to: NumaNode) -> u8 {
    let f = from.as_usize().min(NumaNode::MAX_NODES - 1);
    let t = to.as_usize().min(NumaNode::MAX_NODES - 1);
    NUMA_DISTANCE_TABLE[f][t]
}

/// Retourne le nœud NUMA actuellement affecté à ce CPU.
#[inline]
pub fn cpu_numa_node(cpu: u8) -> NumaNode {
    let node = CPU_TO_NUMA[cpu as usize].load(Ordering::Relaxed);
    NumaNode::new(node)
}

/// Enregistre la topologie NUMA réelle (appelé UNE SEULE FOIS par le parser ACPI/SRAT).
///
/// # Safety
/// Doit être appelé avant tout accès multi-CPU (pendant l'init single-CPU).
pub unsafe fn set_numa_topology(cpu_to_node: &[u8]) {
    debug_assert!(
        !TOPO_READY.load(Ordering::Relaxed),
        "set_numa_topology appelé plusieurs fois"
    );
    for (cpu, &node) in cpu_to_node.iter().enumerate() {
        if cpu >= 256 { break; }
        CPU_TO_NUMA[cpu].store(
            node.min(NumaNode::MAX_NODES as u8 - 1),
            Ordering::Relaxed,
        );
    }
    TOPO_READY.store(true, Ordering::Release);
}
