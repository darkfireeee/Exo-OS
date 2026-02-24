// kernel/src/memory/physical/allocator/ai_hints.rs
//
// Hints IA pour l'allocateur — tables statiques (.rodata uniquement).
// RÈGLE IA-KERNEL-01 : ZERO inférence à l'exécution, ZERO modèle dynamique.
// Les hints sont des lookup tables compilées, jamais mises à jour en runtime.
// Couche 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

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
// TABLE DE HINTS NUMA (statique .rodata)
// ─────────────────────────────────────────────────────────────────────────────

/// Table de hints NUMA : [cpu_id][size_class] → nœud NUMA préféré.
///
/// Générée statiquement — jamais modifiée en runtime.
/// MAX_CPUS=256, SizeClass::COUNT=13.
///
/// Stratégie par défaut : nœud local du CPU (CPU→NUMA node mapping trivial).
///   cpu_id / 4 → numa_node (4 cœurs physiques par nœud NUMA dans la config de ref)
///
/// Ces valeurs sont overridées par set_numa_topology() au boot uniquement.
static NUMA_HINT_TABLE: [[u8; SizeClass::COUNT]; 256] = {
    let mut table = [[0u8; SizeClass::COUNT]; 256];
    let mut cpu = 0usize;
    while cpu < 256 {
        let node = (cpu / 4) as u8;
        let node = if node >= NumaNode::MAX_NODES as u8 { 0 } else { node };
        let mut sc = 0usize;
        while sc < SizeClass::COUNT {
            table[cpu][sc] = node;
            sc += 1;
        }
        cpu += 1;
    }
    table
};

/// Table de distance inter-nœuds NUMA (coût relatif de cross-node allocation).
///
/// distance[src][dst] → coût relatif (1 = local, 2 = 1 hop, 3 = 2 hops, etc.).
/// ACPI SLIT standard: 10 = local, 20 = remote 1 hop.
/// Normalisé ici à [1..=4].
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
// TOPOLOGY (donnée de boot, initée une seule fois)
// ─────────────────────────────────────────────────────────────────────────────

/// Topologie NUMA renseignée au boot par ACPI/SRAT.
/// LOCK-FREE: lecture à partir du moment où `TOPO_READY` = true.
static CPU_TO_NUMA: [AtomicU8; 256] = {
    // Initialisation const: chaque CPU → nœud 0 par défaut.
    const INIT: AtomicU8 = AtomicU8::new(0);
    [INIT; 256]
};

static TOPO_READY: AtomicBool  = AtomicBool::new(false);
static HINTS_ENABLED: AtomicBool = AtomicBool::new(true);

/// Statistiques d'utilisation des hints.
#[derive(Default)]
pub struct HintStats {
    pub hits:   AtomicU64,
    pub misses: AtomicU64,
    pub local:  AtomicU64,
    pub remote: AtomicU64,
}

pub static HINT_STATS: HintStats = HintStats {
    hits:   AtomicU64::new(0),
    misses: AtomicU64::new(0),
    local:  AtomicU64::new(0),
    remote: AtomicU64::new(0),
};

// ─────────────────────────────────────────────────────────────────────────────
// API PUBLIQUE
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le nœud NUMA suggéré pour une allocation sur un CPU donné.
///
/// Règle : lit la table statique NUMA_HINT_TABLE, puis applique la topologie
/// bootée. ZÉRO inférence dynamique (RÈGLE IA-KERNEL-01).
///
/// Retourne `None` si les hints sont désactivés ou si le CPU/size sont hors
/// limites.
#[inline]
pub fn hint_numa_node(size_class: u8, current_cpu: u8) -> Option<NumaNode> {
    if !HINTS_ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    let cpu_idx = current_cpu as usize;
    let sc_idx  = size_class as usize;
    if cpu_idx >= 256 || sc_idx >= SizeClass::COUNT {
        HINT_STATS.misses.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    // Lire la table statique
    let base_node = NUMA_HINT_TABLE[cpu_idx][sc_idx];
    // Override par topologie réelle si disponible
    let node = if TOPO_READY.load(Ordering::Acquire) {
        CPU_TO_NUMA[cpu_idx].load(Ordering::Relaxed)
    } else {
        base_node
    };
    if node >= NumaNode::MAX_NODES as u8 {
        HINT_STATS.misses.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    HINT_STATS.hits.fetch_add(1, Ordering::Relaxed);
    let result = NumaNode::new(node);
    if result == NumaNode::LOCAL {
        HINT_STATS.local.fetch_add(1, Ordering::Relaxed);
    } else {
        HINT_STATS.remote.fetch_add(1, Ordering::Relaxed);
    }
    Some(result)
}

/// Active ou désactive les hints IA.
pub fn set_hints_enabled(enabled: bool) {
    HINTS_ENABLED.store(enabled, Ordering::SeqCst);
}

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
/// SAFETY: Doit être appelé avant tout accès multi-CPU (pendant l'init single-CPU).
pub unsafe fn set_numa_topology(cpu_to_node: &[u8]) {
    debug_assert!(!TOPO_READY.load(Ordering::Relaxed),
        "set_numa_topology appelé plusieurs fois");
    for (cpu, &node) in cpu_to_node.iter().enumerate() {
        if cpu >= 256 { break; }
        CPU_TO_NUMA[cpu].store(
            node.min(NumaNode::MAX_NODES as u8 - 1),
            Ordering::Relaxed,
        );
    }
    TOPO_READY.store(true, Ordering::Release);
}

/// Retourne les statistiques d'utilisation des hints.
pub fn stats() -> (u64, u64, u64, u64) {
    (
        HINT_STATS.hits.load(Ordering::Relaxed),
        HINT_STATS.misses.load(Ordering::Relaxed),
        HINT_STATS.local.load(Ordering::Relaxed),
        HINT_STATS.remote.load(Ordering::Relaxed),
    )
}
