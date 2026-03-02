// kernel/src/scheduler/smp/topology.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Topologie SMP/NUMA — détection et accès aux distances inter-CPU
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, Ordering};
use crate::scheduler::core::task::CpuId;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes --- limites statiques (tables .bss, no_alloc)
// ─────────────────────────────────────────────────────────────────────────────

pub const MAX_CPUS:  usize = 256;
pub const MAX_NODES: usize = 16;
/// Sentinel : CPU non présent.
pub const CPU_ABSENT: u32 = u32::MAX;

// ─────────────────────────────────────────────────────────────────────────────
// Données topologiques
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de CPU logiques détectés.
static NR_CPUS: AtomicU32 = AtomicU32::new(1);
/// Nombre de nœuds NUMA détectés.
static NR_NODES: AtomicU32 = AtomicU32::new(1);

/// Table CPU → nœud NUMA. Index = cpu_id, valeur = node_id.
static mut CPU_TO_NODE: [u8; MAX_CPUS] = [0u8; MAX_CPUS];

/// Table CPU → CPU physique (HT sibling). `CPU_ABSENT` si pas de HT.
static mut CPU_SIBLING: [u32; MAX_CPUS] = [CPU_ABSENT; MAX_CPUS];

/// Matrice de distance NUMA NR_NODES × NR_NODES.
/// distance[node_a][node_b] : coût de migration inter-nœud (1 = local, >1 = distant).
static mut NUMA_DISTANCE: [[u8; MAX_NODES]; MAX_NODES] = [[1u8; MAX_NODES]; MAX_NODES];

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation (appelée depuis scheduler::init par le BSP)
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise la topologie SMP.
///
/// # Safety
/// Doit être appelé avant tout autre accès aux tables de topologie,
/// et avant l'activation des APs.
pub unsafe fn init(nr_cpus: usize, nr_nodes: usize) {
    let nr_cpus  = nr_cpus.min(MAX_CPUS);
    let nr_nodes = nr_nodes.min(MAX_NODES);
    NR_CPUS.store(nr_cpus as u32, Ordering::Release);
    NR_NODES.store(nr_nodes as u32, Ordering::Release);

    // Distance locale = 10 (standard SLIT), distante = 20.
    for a in 0..nr_nodes {
        for b in 0..nr_nodes {
            NUMA_DISTANCE[a][b] = if a == b { 10 } else { 20 };
        }
    }
}

/// Enregistre le nœud NUMA d'un CPU.
///
/// # Safety
/// Doit être appelé pendant l'init, avant que le CPU soit mis en ligne.
pub unsafe fn set_cpu_node(cpu: CpuId, node: u8) {
    let idx = cpu.0 as usize;
    if idx < MAX_CPUS { CPU_TO_NODE[idx] = node; }
}

/// Enregistre le sibling (HyperThread) d'un CPU.
///
/// # Safety
/// Doit être appelé pendant l'init.
pub unsafe fn set_cpu_sibling(cpu: CpuId, sibling: CpuId) {
    let idx = cpu.0 as usize;
    if idx < MAX_CPUS { CPU_SIBLING[idx] = sibling.0; }
}

/// Définit la distance entre deux nœuds NUMA (symétrique).
///
/// # Safety
/// Doit être appelé pendant l'init.
pub unsafe fn set_numa_distance(a: usize, b: usize, dist: u8) {
    if a < MAX_NODES && b < MAX_NODES {
        NUMA_DISTANCE[a][b] = dist;
        NUMA_DISTANCE[b][a] = dist;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Accesseurs (read-only après init)
// ─────────────────────────────────────────────────────────────────────────────

pub fn nr_cpus()  -> usize { NR_CPUS.load(Ordering::Relaxed)  as usize }
pub fn nr_nodes() -> usize { NR_NODES.load(Ordering::Relaxed) as usize }

/// Retourne le nœud NUMA du CPU `cpu`.
pub fn cpu_node(cpu: CpuId) -> u8 {
    let idx = cpu.0 as usize;
    // SAFETY: idx < MAX_CPUS garanti par le if; CPU_TO_NODE est un tableau statique
    // initialisé par topology_init() avant tout appel à cpu_node().
    if idx < MAX_CPUS { unsafe { CPU_TO_NODE[idx] } }
    else { 0 }
}

/// Retourne le sibling HT du CPU `cpu`, ou `CPU_ABSENT`.
pub fn cpu_sibling(cpu: CpuId) -> u32 {
    let idx = cpu.0 as usize;
    // SAFETY: idx < MAX_CPUS garanti par le if; CPU_SIBLING est initialisé par
    // topology_init() en lecture seule après le boot.
    if idx < MAX_CPUS { unsafe { CPU_SIBLING[idx] } }
    else { CPU_ABSENT }
}

/// Retourne la distance NUMA entre deux nœuds.
pub fn numa_distance(a: usize, b: usize) -> u8 {
    // SAFETY: a < MAX_NODES && b < MAX_NODES garantit que les indices sont dans
    // les bornes du tableau 2D statique NUMA_DISTANCE.
    if a < MAX_NODES && b < MAX_NODES { unsafe { NUMA_DISTANCE[a][b] } }
    else { u8::MAX }
}

/// Retourne `true` si le CPU `cpu` est sur le même nœud NUMA que `reference`.
pub fn same_node(cpu: CpuId, reference: CpuId) -> bool {
    cpu_node(cpu) == cpu_node(reference)
}
