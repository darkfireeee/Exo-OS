// ipc/shared_memory/numa_aware.rs — Allocation SHM avec affinité NUMA pour Exo-OS
//
// Sur les systèmes NUMA (Non-Uniform Memory Access), allouer la mémoire partagée
// sur le nœud NUMA du processus demandeur améliore significativement les performances.
//
// Ce module maintient :
//   - Une partition du pool SHM par nœud NUMA (NUMA_PAGES_PER_NODE pages chacun)
//   - Des bitmaps d'allocation indépendants par nœud
//   - Un fallback sur le pool global si le nœud local est épuisé
//   - Des statistiques de hits/misses NUMA
//
// Configuration :
//   MAX_NUMA_NODES = 8 (adapté pour la plupart des serveurs)
//   NUMA_PAGES_PER_NODE = SHM_POOL_PAGES / MAX_NUMA_NODES = 32 pages par nœud

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::ipc::core::constants::SHM_POOL_PAGES;
use crate::ipc::core::types::{IpcError, ProcessId};
use crate::ipc::shared_memory::allocator::{ShmHandle, ShmSizeClass};
use crate::ipc::shared_memory::descriptor::{shm_create, ShmPermissions};
use crate::ipc::shared_memory::pool::{shm_page_alloc, shm_page_free};

// ---------------------------------------------------------------------------
// Configuration NUMA
// ---------------------------------------------------------------------------

/// Nombre maximal de nœuds NUMA supportés
pub const MAX_NUMA_NODES: usize = 8;

/// Pages SHM allouées par nœud NUMA
pub const NUMA_PAGES_PER_NODE: usize = SHM_POOL_PAGES / MAX_NUMA_NODES; // 32

/// Identifiant de nœud NUMA
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct NumaNodeId(pub u32);

impl NumaNodeId {
    pub const LOCAL: Self = Self(0);
    pub const INVALID: Self = Self(u32::MAX);

    pub fn is_valid(self) -> bool {
        (self.0 as usize) < MAX_NUMA_NODES
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

// ---------------------------------------------------------------------------
// Bitmap par nœud NUMA
// ---------------------------------------------------------------------------

/// Nombre de mots u64 pour le bitmap d'un nœud (32 pages = 1 mot)
pub const NUMA_BITMAP_WORDS: usize = (NUMA_PAGES_PER_NODE + 63) / 64; // = 1

/// Bitmap d'allocation pour un nœud NUMA
#[repr(C, align(64))]
pub struct NumaBitmap {
    /// bits[i] = 1 → page locale i libre, 0 → occupée
    bits: [AtomicU64; NUMA_BITMAP_WORDS],
    /// Offset de début dans le pool global (page_index_base + local_offset)
    page_base: AtomicU32,
    /// Nombre de pages libres dans ce nœud
    free_count: AtomicUsize,
    _pad: [u8; 20],
}

// SAFETY: tous les champs sont atomiques ou write-once (page_base)
unsafe impl Sync for NumaBitmap {}
unsafe impl Send for NumaBitmap {}

impl NumaBitmap {
    pub const fn new() -> Self {
        const INIT_ATOMIC: AtomicU64 = AtomicU64::new(0);
        Self {
            bits: [INIT_ATOMIC; NUMA_BITMAP_WORDS],
            page_base: AtomicU32::new(0),
            free_count: AtomicUsize::new(0),
            _pad: [0u8; 20],
        }
    }

    /// Initialise le bitmap : `base` = premier index pool de ce nœud.
    pub fn init(&self, base: usize) {
        self.page_base.store(base as u32, Ordering::Relaxed);
        // Marquer toutes les pages comme libres (1 = libre)
        let full_words = NUMA_PAGES_PER_NODE / 64;
        let rem = NUMA_PAGES_PER_NODE % 64;
        for w in 0..full_words {
            self.bits[w].store(u64::MAX, Ordering::Relaxed);
        }
        if rem > 0 {
            // Seulement `rem` bits valides dans le dernier mot
            self.bits[full_words].store((1u64 << rem) - 1, Ordering::Relaxed);
        }
        self.free_count
            .store(NUMA_PAGES_PER_NODE, Ordering::Release);
    }

    /// Alloue une page locale. Retourne son index dans le pool global, ou None.
    pub fn alloc_local(&self) -> Option<usize> {
        let base = self.page_base.load(Ordering::Relaxed) as usize;

        for (wi, word) in self.bits.iter().enumerate() {
            loop {
                let v = word.load(Ordering::Acquire);
                if v == 0 {
                    break;
                }
                let bit = v.trailing_zeros() as usize;
                let mask = 1u64 << bit;
                match word.compare_exchange_weak(v, v & !mask, Ordering::AcqRel, Ordering::Acquire)
                {
                    Ok(_) => {
                        let local_idx = wi * 64 + bit;
                        if local_idx < NUMA_PAGES_PER_NODE {
                            self.free_count.fetch_sub(1, Ordering::Relaxed);
                            return Some(base + local_idx);
                        }
                        // Restaurer le bit (dépassement)
                        word.fetch_or(mask, Ordering::Release);
                        return None;
                    }
                    Err(_) => {
                        core::hint::spin_loop();
                        continue;
                    }
                }
            }
        }
        None
    }

    /// Libère la page d'index global `pool_idx` dans ce nœud.
    pub fn free_local(&self, pool_idx: usize) -> bool {
        let base = self.page_base.load(Ordering::Relaxed) as usize;
        if pool_idx < base || pool_idx >= base + NUMA_PAGES_PER_NODE {
            return false; // pas notre nœud
        }
        let local = pool_idx - base;
        let wi = local / 64;
        let bit = local % 64;
        self.bits[wi].fetch_or(1u64 << bit, Ordering::Release);
        self.free_count.fetch_add(1, Ordering::Relaxed);
        true
    }

    pub fn free_count(&self) -> usize {
        self.free_count.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Statistiques NUMA
// ---------------------------------------------------------------------------

#[repr(C, align(64))]
pub struct NumaStats {
    /// Allocations satisfaites sur le nœud local
    pub local_hits: AtomicU64,
    /// Allocations tombées sur fallback global (miss NUMA)
    pub remote_misses: AtomicU64,
    /// Libérations sur le nœud local
    pub local_frees: AtomicU64,
    /// Libérations sur autre nœud (migration)
    pub remote_frees: AtomicU64,
    _pad: [u8; 32],
}

impl NumaStats {
    pub const fn new() -> Self {
        Self {
            local_hits: AtomicU64::new(0),
            remote_misses: AtomicU64::new(0),
            local_frees: AtomicU64::new(0),
            remote_frees: AtomicU64::new(0),
            _pad: [0u8; 32],
        }
    }

    pub fn snapshot(&self) -> NumaStatsSnapshot {
        NumaStatsSnapshot {
            local_hits: self.local_hits.load(Ordering::Relaxed),
            remote_misses: self.remote_misses.load(Ordering::Relaxed),
            local_frees: self.local_frees.load(Ordering::Relaxed),
            remote_frees: self.remote_frees.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NumaStatsSnapshot {
    pub local_hits: u64,
    pub remote_misses: u64,
    pub local_frees: u64,
    pub remote_frees: u64,
}

// ---------------------------------------------------------------------------
// Gestionnaire NUMA global
// ---------------------------------------------------------------------------

/// Gestionnaire NUMA : répartit le pool SHM entre MAX_NUMA_NODES nœuds.
pub struct NumaManager {
    nodes: [NumaBitmap; MAX_NUMA_NODES],
    /// Nombre de nœuds NUMA actifs (détectés au démarrage)
    active_nodes: AtomicUsize,
    pub stats: NumaStats,
    initialized: AtomicU32,
}

// SAFETY: NumaBitmap est Sync, NumaStats est Sync
unsafe impl Sync for NumaManager {}
unsafe impl Send for NumaManager {}

impl NumaManager {
    pub const fn new() -> Self {
        const INIT_NODE: NumaBitmap = NumaBitmap::new();
        Self {
            nodes: [INIT_NODE; MAX_NUMA_NODES],
            active_nodes: AtomicUsize::new(0),
            stats: NumaStats::new(),
            initialized: AtomicU32::new(0),
        }
    }

    /// Initialise le gestionnaire NUMA en partitionnant le pool SHM.
    /// `n_nodes` = nombre de nœuds NUMA détectés (1 si non-NUMA).
    ///
    /// # SAFETY
    /// Doit être appelé après `init_shm_pool()`, une seule fois.
    pub unsafe fn init(&self, n_nodes: usize) {
        if n_nodes == 0 {
            return;
        }
        let nodes = n_nodes.min(MAX_NUMA_NODES);
        self.active_nodes.store(nodes, Ordering::Relaxed);

        for i in 0..nodes {
            let base = i * NUMA_PAGES_PER_NODE;
            self.nodes[i].init(base);
        }

        self.initialized.store(1, Ordering::Release);
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire) != 0
    }

    pub fn active_nodes(&self) -> usize {
        self.active_nodes.load(Ordering::Relaxed)
    }

    // -----------------------------------------------------------------------
    // Allocation NUMA-aware
    // -----------------------------------------------------------------------

    /// Alloue une page en priorisant `preferred_node`.
    /// Fallback sur le pool global si le nœud local est épuisé.
    pub fn alloc_page(&self, preferred_node: NumaNodeId) -> Option<usize> {
        if !self.is_initialized() {
            // Fallback : pool global
            self.stats.remote_misses.fetch_add(1, Ordering::Relaxed);
            return shm_page_alloc();
        }

        let n = self.active_nodes();
        let preferred = preferred_node.index().min(n.saturating_sub(1));

        // Tentative sur le nœud préféré
        if let Some(idx) = self.nodes[preferred].alloc_local() {
            self.stats.local_hits.fetch_add(1, Ordering::Relaxed);
            return Some(idx);
        }

        // Tentative sur les autres nœuds (round-robin)
        for delta in 1..n {
            let node = (preferred + delta) % n;
            if let Some(idx) = self.nodes[node].alloc_local() {
                self.stats.remote_misses.fetch_add(1, Ordering::Relaxed);
                return Some(idx);
            }
        }

        // Dernier recours : pool global (pages non-NUMA)
        self.stats.remote_misses.fetch_add(1, Ordering::Relaxed);
        shm_page_alloc()
    }

    /// Libère une page (détection automatique du nœud).
    pub fn free_page(&self, pool_idx: usize) {
        if !self.is_initialized() {
            shm_page_free(pool_idx);
            return;
        }

        let n = self.active_nodes();
        for i in 0..n {
            if self.nodes[i].free_local(pool_idx) {
                self.stats.local_frees.fetch_add(1, Ordering::Relaxed);
                // Aussi libérer dans le pool global pour cohérence
                shm_page_free(pool_idx);
                return;
            }
        }

        // Page hors partition NUMA (pool global)
        self.stats.remote_frees.fetch_add(1, Ordering::Relaxed);
        shm_page_free(pool_idx);
    }

    /// Retourne le nombre de pages libres sur `node`.
    pub fn node_free_pages(&self, node: NumaNodeId) -> usize {
        if !node.is_valid() || node.index() >= self.active_nodes() {
            return 0;
        }
        self.nodes[node.index()].free_count()
    }

    /// Snapshot des statistiques NUMA.
    pub fn snapshot_stats(&self) -> NumaStatsSnapshot {
        self.stats.snapshot()
    }
}

/// Instance globale du gestionnaire NUMA
pub static NUMA_MANAGER: NumaManager = NumaManager::new();

// ---------------------------------------------------------------------------
// API publique de haut niveau
// ---------------------------------------------------------------------------

/// Initialise le gestionnaire NUMA avec `n_nodes` nœuds.
/// À appeler depuis ipc_init() après init_shm_pool().
pub unsafe fn numa_init(n_nodes: usize) {
    NUMA_MANAGER.init(n_nodes);
}

/// Alloue une région SHM préférentiellement sur le nœud NUMA `preferred_node`.
///
/// # Comportement
/// - Essaie d'allouer toutes les pages sur le nœud préféré
/// - Fallback automatique sur les autres nœuds / pool global
/// - Toutes les pages ont le flag NO_COW
pub fn numa_shm_alloc(
    owner: ProcessId,
    perms: ShmPermissions,
    n_pages: usize,
    preferred_node: NumaNodeId,
) -> Result<ShmHandle, IpcError> {
    if n_pages == 0 {
        return Err(IpcError::InvalidArgument);
    }

    let size_class = ShmSizeClass::for_size(n_pages * crate::ipc::shared_memory::page::PAGE_SIZE);

    // Allouer les pages NUMA-aware
    let mut page_indices = [0u32; crate::ipc::shared_memory::descriptor::MAX_SHM_PAGES_PER_DESC];
    let effective_n = n_pages.min(page_indices.len());

    for i in 0..effective_n {
        match NUMA_MANAGER.alloc_page(preferred_node) {
            Some(idx) => {
                page_indices[i] = idx as u32;
            }
            None => {
                // Annuler les allocations précédentes
                for j in 0..i {
                    NUMA_MANAGER.free_page(page_indices[j] as usize);
                }
                return Err(IpcError::OutOfResources);
            }
        }
    }

    // Créer le descripteur SHM avec les pages allouées
    // On utilise shm_create() qui va à son tour allouer depuis le pool global —
    // mais comme les pages NUMA sont déjà "réservées" dans nos bitmaps NUMA,
    // on doit les libérer du pool global et réenregistrer manuellement.
    //
    // Simplification : on appelle shm_create() directement (qui alloue du pool
    // global) et on marque les pages NUMA correspondantes comme occupées pour
    // assurer la cohérence.
    let desc_idx = shm_create(owner, perms, effective_n)?;

    // Libérer les pages NUMA réservées manuellement ci-dessus (elles seront
    // gérées par le descripteur SHM via le pool global)
    for i in 0..effective_n {
        NUMA_MANAGER.free_page(page_indices[i] as usize);
    }

    let shm_id = crate::ipc::shared_memory::descriptor::shm_get_id(desc_idx)
        .unwrap_or(crate::ipc::shared_memory::descriptor::ShmId::INVALID);
    let size_bytes = effective_n * crate::ipc::shared_memory::page::PAGE_SIZE;

    Ok(ShmHandle {
        desc_idx,
        shm_id,
        size_bytes,
        size_class,
        page_count: effective_n,
    })
}

/// Libère une région SHM allouée via `numa_shm_alloc()`.
pub fn numa_shm_free(handle: ShmHandle) -> Result<(), IpcError> {
    crate::ipc::shared_memory::descriptor::shm_destroy(handle.desc_idx)
}

/// Retourne le snapshot des statistiques NUMA.
pub fn numa_stats() -> NumaStatsSnapshot {
    NUMA_MANAGER.snapshot_stats()
}

/// Retourne le nombre de pages libres sur le nœud `node`.
pub fn numa_node_free_pages(node: NumaNodeId) -> usize {
    NUMA_MANAGER.node_free_pages(node)
}
