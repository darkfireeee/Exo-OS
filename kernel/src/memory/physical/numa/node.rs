// kernel/src/memory/physical/numa/node.rs
//
// Nœuds NUMA — descripteurs, table globale et compteurs par nœud.
//
// Chaque nœud NUMA représente une banque mémoire locale à un ensemble de CPUs.
// Cette couche maintient :
//   • La liste des nœuds détectés (via ACPI SRAT à l'init).
//   • Les plages d'adresses physiques appartenant à chaque nœud.
//   • Les compteurs de pages libres / utilisées par nœud.
//
// COUCHE 0 — aucune dépendance scheduler/process/ipc/fs.


use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU32, Ordering};

use crate::memory::core::constants::PAGE_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximal de nœuds NUMA.
pub const MAX_NUMA_NODES: usize = 8;
/// Nombre maximal de plages physiques par nœud.
const MAX_RANGES_PER_NODE: usize = 16;
/// ID invalide / sentinelle.
pub const NUMA_NODE_INVALID: u32 = u32::MAX;

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques par nœud
// ─────────────────────────────────────────────────────────────────────────────

/// Compteurs live par nœud NUMA.
#[repr(C, align(64))]
pub struct NumaNodeStats {
    /// Pages totales dans ce nœud.
    pub total_pages:  AtomicU64,
    /// Pages libres actuellement.
    pub free_pages:   AtomicU64,
    /// Pages utilisées (total - free).
    pub used_pages:   AtomicU64,
    /// Allocations locales réussies.
    pub local_allocs: AtomicU64,
    /// Allocations distantes (fallback).
    pub remote_allocs: AtomicU64,
    /// Pages migrées vers ce nœud.
    pub migrated_in:  AtomicU64,
    /// Pages migrées hors de ce nœud.
    pub migrated_out: AtomicU64,
    _pad: [u8; 8],
}

impl NumaNodeStats {
    const fn new() -> Self {
        Self {
            total_pages:  AtomicU64::new(0),
            free_pages:   AtomicU64::new(0),
            used_pages:   AtomicU64::new(0),
            local_allocs: AtomicU64::new(0),
            remote_allocs: AtomicU64::new(0),
            migrated_in:  AtomicU64::new(0),
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
// Plage physique appartenant à un nœud
// ─────────────────────────────────────────────────────────────────────────────

/// Une plage d'adresses physiques appartenant à un nœud NUMA.
#[derive(Debug, Clone, Copy)]
pub struct NumaPhysRange {
    pub start: u64,  // inclus
    pub end:   u64,  // exclus
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
// Descripteur de nœud NUMA
// ─────────────────────────────────────────────────────────────────────────────

/// Descripteur complet d'un nœud NUMA.
#[repr(C, align(64))]
pub struct NumaNode {
    /// Identifiant unique du nœud (0..MAX_NUMA_NODES-1).
    pub id:     u32,
    /// Masque de CPUs locaux à ce nœud (bit i = CPU i).
    pub cpu_mask: AtomicU64,
    /// Nombre de plages physiques valides dans `ranges`.
    pub range_count: u32,
    /// Actif (détecté à l'init ACPI SRAT).
    pub active: AtomicBool,
    _pad1: [u8; 3],
    /// Statistiques live.
    pub stats: NumaNodeStats,
    /// Plages physiques de ce nœud.
    pub ranges: [NumaPhysRange; MAX_RANGES_PER_NODE],
    _pad2: [u8; 16],
}

impl NumaNode {
    const fn new(id: u32) -> Self {
        Self {
            id,
            cpu_mask:    AtomicU64::new(0),
            range_count: 0,
            active:      AtomicBool::new(false),
            _pad1:       [0; 3],
            stats:       NumaNodeStats::new(),
            ranges:      [NumaPhysRange { start: 0, end: 0 }; MAX_RANGES_PER_NODE],
            _pad2:       [0; 16],
        }
    }

    /// Ajoute une plage physique.  Retourne `false` si table pleine.
    pub fn add_range(&mut self, start: u64, end: u64) -> bool {
        if self.range_count as usize >= MAX_RANGES_PER_NODE {
            return false;
        }
        self.ranges[self.range_count as usize] = NumaPhysRange { start, end };
        self.range_count += 1;
        let new_pages = (end - start) / PAGE_SIZE as u64;
        self.stats.total_pages.fetch_add(new_pages, Ordering::Relaxed);
        self.stats.free_pages.fetch_add(new_pages, Ordering::Relaxed);
        true
    }

    /// Retourne `true` si `phys` appartient à ce nœud.
    pub fn owns_phys(&self, phys: u64) -> bool {
        for i in 0..self.range_count as usize {
            if self.ranges[i].contains_phys(phys) {
                return true;
            }
        }
        false
    }

    /// Total de pages libres sur ce nœud.
    #[inline]
    pub fn free_pages(&self) -> u64 {
        self.stats.free_pages.load(Ordering::Relaxed)
    }

    /// Total de pages dans ce nœud.
    #[inline]
    pub fn total_pages(&self) -> u64 {
        self.stats.total_pages.load(Ordering::Relaxed)
    }
}

unsafe impl Sync for NumaNode {}

// ─────────────────────────────────────────────────────────────────────────────
// Table globale des nœuds
// ─────────────────────────────────────────────────────────────────────────────

/// Table centrale de tous les nœuds NUMA détectés.
pub struct NumaNodeTable {
    nodes:      [NumaNode; MAX_NUMA_NODES],
    node_count: AtomicU32,
}

impl NumaNodeTable {
    const fn new() -> Self {
        Self {
            nodes: [
                NumaNode::new(0), NumaNode::new(1), NumaNode::new(2), NumaNode::new(3),
                NumaNode::new(4), NumaNode::new(5), NumaNode::new(6), NumaNode::new(7),
            ],
            node_count: AtomicU32::new(0),
        }
    }

    /// Enregistre un nœud NUMA.
    /// `cpu_mask` : bitmask des CPUs locaux.
    /// Returns l'id du nœud ou `NUMA_NODE_INVALID` si table pleine.
    pub fn register_node(&self, cpu_mask: u64) -> u32 {
        let id = self.node_count.fetch_add(1, Ordering::AcqRel);
        if id as usize >= MAX_NUMA_NODES {
            self.node_count.fetch_sub(1, Ordering::Relaxed);
            return NUMA_NODE_INVALID;
        }
        // SAFETY : accès par id unique (counter atomique), pas de race.
        let node = unsafe { &mut *(core::ptr::addr_of!(self.nodes[id as usize]) as *mut NumaNode) };
        node.cpu_mask.store(cpu_mask, Ordering::Release);
        node.active.store(true, Ordering::Release);
        id
    }

    /// Ajoute une plage physique au nœud `id`.
    pub fn add_range(&self, id: u32, start: u64, end: u64) -> bool {
        if id as usize >= MAX_NUMA_NODES {
            return false;
        }
        // SAFETY : id < MAX_NUMA_NODES vérifié ci-dessus, addr_of! évite &T→*mut T.
        // L'accès est mono-thread pendant l'initialisation SRAT.
        let node = unsafe { &mut *(core::ptr::addr_of!(self.nodes[id as usize]) as *mut NumaNode) };
        node.add_range(start, end)
    }

    /// Retourne le nœud owning `phys` ou `NUMA_NODE_INVALID`.
    pub fn node_for_phys(&self, phys: u64) -> u32 {
        let count = self.node_count.load(Ordering::Acquire) as usize;
        for i in 0..count {
            if self.nodes[i].active.load(Ordering::Relaxed) && self.nodes[i].owns_phys(phys) {
                return i as u32;
            }
        }
        NUMA_NODE_INVALID
    }

    /// Retourne le nœud local pour `cpu_id` (premier nœud dont le masque inclut cpu).
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
        0 // Fallback nœud 0
    }

    /// Accès à un nœud par id.
    #[inline]
    pub fn get(&self, id: u32) -> Option<&NumaNode> {
        if id as usize >= MAX_NUMA_NODES { return None; }
        let n = &self.nodes[id as usize];
        if n.active.load(Ordering::Relaxed) { Some(n) } else { None }
    }

    /// Nombre de nœuds actifs.
    #[inline]
    pub fn count(&self) -> u32 {
        self.node_count.load(Ordering::Acquire)
    }
}

unsafe impl Sync for NumaNodeTable {}

pub static NUMA_NODES: NumaNodeTable = NumaNodeTable::new();

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques globales NUMA
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct NumaGlobalStats {
    pub total_nodes:   AtomicU32,
    pub fallback_allocs: AtomicU64,
    pub migration_ops:   AtomicU64,
}

impl NumaGlobalStats {
    const fn new() -> Self {
        Self {
            total_nodes:   AtomicU32::new(0),
            fallback_allocs: AtomicU64::new(0),
            migration_ops:   AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for NumaGlobalStats {}
pub static NUMA_GLOBAL_STATS: NumaGlobalStats = NumaGlobalStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation
// ─────────────────────────────────────────────────────────────────────────────

/// Init NUMA par défaut (UMA) : un unique nœud avec tous les CPUs + plage totale.
///
/// Dans un système NUMA réel, cette fonction est remplacée par le parseur ACPI
/// SRAT qui appelle `NUMA_NODES.register_node()` + `add_range()` pour chaque
/// nœud découvert.
///
/// # Safety : CPL 0.
pub unsafe fn init() {
    // Nœud 0 : tous les CPUs, plage 0..4GiB par défaut.
    let nid = NUMA_NODES.register_node(u64::MAX);
    NUMA_NODES.add_range(nid, 0x0000_0000, 0x1_0000_0000);
    NUMA_GLOBAL_STATS.total_nodes.store(1, Ordering::Relaxed);
}
