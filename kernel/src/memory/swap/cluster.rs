// kernel/src/memory/swap/cluster.rs
//
// Regroupement d'I/O swap en clusters séquentiels.
//
// Motivation : soumettre 8 pages au swap en une seule opération séquentielle
// de 32 KiB est bien plus efficace que 8 I/O de 4 KiB individuelles.
//
// Architecture :
//   • `SwapCluster` — groupe de CLUSTER_SIZE slots consécutifs sur un device.
//   • `ClusterQueue` — file d'attente de clusters prêts à être écrits.
//   • `ClusterAccumulator` — accumule les slots en entrée jusqu'à former
//     un cluster complet, puis le pousse dans la `ClusterQueue`.
//   • `ClusterManager` — point d'entrée global.
//   • `CLUSTER_MANAGER` — instance statique.
//
// Flux typique :
//   1. `CLUSTER_MANAGER.add_page(dev_idx, slot, pfn)` — appelé par reclaim.
//   2. Quand CLUSTER_SIZE pages sont accumulées, un cluster est formé.
//   3. `CLUSTER_MANAGER.flush()` — force l'écriture même si cluster partiel.
//   4. `CLUSTER_MANAGER.next_cluster()` — retourne le prochain cluster à écrire
//      (appelé par le thread swap pour effectuer les I/O réelles).

use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::swap::backend::SwapSlot;

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTES
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de pages par cluster (8 × 4 KiB = 32 KiB par I/O).
pub const CLUSTER_SIZE: usize = 8;

/// Nombre maximum de clusters dans la queue prête à être écrite.
pub const MAX_CLUSTER_QUEUE: usize = 256;

/// Nombre maximum d'accumulateurs per-device (1 accumulateur = 1 cluster en cours).
pub const MAX_SWAP_DEVICES: usize = 8; // doit correspondre à swap/backend.rs

// ─────────────────────────────────────────────────────────────────────────────
// SWAP CLUSTER
// ─────────────────────────────────────────────────────────────────────────────

/// Une entrée dans un cluster — un slot de swap et son PFN source.
#[derive(Copy, Clone, Debug)]
pub struct ClusterEntry {
    pub slot: SwapSlot,
    pub src_pfn: u64,
}

impl ClusterEntry {
    const EMPTY: ClusterEntry = ClusterEntry {
        slot: SwapSlot(0),
        src_pfn: 0,
    };
}

/// Un cluster de `CLUSTER_SIZE` pages à écrire séquentiellement.
#[derive(Copy, Clone)]
pub struct SwapCluster {
    /// Indice du device de swap cible.
    pub dev_idx: u8,
    /// Les entrées du cluster.
    pub entries: [ClusterEntry; CLUSTER_SIZE],
    /// Nombre d'entrées valides dans ce cluster (1..=CLUSTER_SIZE).
    pub count: u8,
}

impl SwapCluster {
    const fn new() -> Self {
        SwapCluster {
            dev_idx: 0,
            entries: [ClusterEntry::EMPTY; CLUSTER_SIZE],
            count: 0,
        }
    }

    /// Retourne les entrées valides.
    #[inline]
    pub fn valid_entries(&self) -> &[ClusterEntry] {
        &self.entries[..self.count as usize]
    }

    /// Taille totale en octets de ce cluster.
    #[inline]
    pub fn byte_size(&self) -> usize {
        self.count as usize * crate::memory::core::constants::PAGE_SIZE
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CLUSTER QUEUE (RING-BUFFER STATIQUE)
// ─────────────────────────────────────────────────────────────────────────────

struct ClusterQueue {
    buf: [SwapCluster; MAX_CLUSTER_QUEUE],
    head: usize, // producteur
    tail: usize, // consommateur
    count: usize,
}

impl ClusterQueue {
    const fn new() -> Self {
        ClusterQueue {
            buf: [SwapCluster::new(); MAX_CLUSTER_QUEUE], // Copy
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    fn push(&mut self, cluster: SwapCluster) -> bool {
        if self.count >= MAX_CLUSTER_QUEUE {
            return false;
        }
        self.buf[self.head] = cluster;
        self.head = (self.head + 1) % MAX_CLUSTER_QUEUE;
        self.count += 1;
        true
    }

    fn pop(&mut self) -> Option<SwapCluster> {
        if self.count == 0 {
            return None;
        }
        let c = self.buf[self.tail];
        self.tail = (self.tail + 1) % MAX_CLUSTER_QUEUE;
        self.count -= 1;
        Some(c)
    }

    #[inline]
    fn len(&self) -> usize {
        self.count
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ACCUMULATEUR PAR DEVICE
// ─────────────────────────────────────────────────────────────────────────────

struct Accumulator {
    cluster: SwapCluster,
    /// True si cet accumulateur est actif pour un device.
    active: bool,
}

impl Accumulator {
    const fn new() -> Self {
        Accumulator {
            cluster: SwapCluster::new(),
            active: false,
        }
    }

    /// Ajoute une entrée à l'accumulateur en cours.
    /// Retourne `Some(cluster)` si le cluster est maintenant complet.
    fn add_entry(&mut self, dev_idx: u8, entry: ClusterEntry) -> Option<SwapCluster> {
        if !self.active {
            self.cluster = SwapCluster::new();
            self.cluster.dev_idx = dev_idx;
            self.active = true;
        }
        let idx = self.cluster.count as usize;
        if idx < CLUSTER_SIZE {
            self.cluster.entries[idx] = entry;
            self.cluster.count += 1;
        }
        if self.cluster.count as usize >= CLUSTER_SIZE {
            let full = self.cluster;
            self.active = false;
            self.cluster.count = 0;
            Some(full)
        } else {
            None
        }
    }

    /// Vide l'accumulateur partiellement rempli (flush forcé).
    /// Retourne `Some(cluster)` s'il y avait des entrées en attente.
    fn flush(&mut self) -> Option<SwapCluster> {
        if !self.active || self.cluster.count == 0 {
            return None;
        }
        let partial = self.cluster;
        self.active = false;
        self.cluster.count = 0;
        Some(partial)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CLUSTER MANAGER
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques du gestionnaire de clusters.
pub struct ClusterStats {
    /// Total de clusters complets formés.
    pub clusters_formed: AtomicU64,
    /// Total de clusters partiels flushés.
    pub partial_flushes: AtomicU64,
    /// Total d'entrées ajoutées.
    pub pages_added: AtomicU64,
    /// Clusters perdus (queue pleine).
    pub queue_overflows: AtomicU64,
    /// Clusters prélevés par le thread d'I/O.
    pub clusters_consumed: AtomicU64,
}

impl ClusterStats {
    const fn new() -> Self {
        ClusterStats {
            clusters_formed: AtomicU64::new(0),
            partial_flushes: AtomicU64::new(0),
            pages_added: AtomicU64::new(0),
            queue_overflows: AtomicU64::new(0),
            clusters_consumed: AtomicU64::new(0),
        }
    }
}

struct ClusterManagerInner {
    accumulators: [Accumulator; MAX_SWAP_DEVICES],
    queue: ClusterQueue,
}

impl ClusterManagerInner {
    const fn new() -> Self {
        const A: Accumulator = Accumulator::new();
        ClusterManagerInner {
            accumulators: [A; MAX_SWAP_DEVICES],
            queue: ClusterQueue::new(),
        }
    }
}

pub struct ClusterManager {
    inner: Mutex<ClusterManagerInner>,
    pub stats: ClusterStats,
}

impl ClusterManager {
    const fn new() -> Self {
        ClusterManager {
            inner: Mutex::new(ClusterManagerInner::new()),
            stats: ClusterStats::new(),
        }
    }

    /// Ajoute une page au cluster en cours de formation pour `dev_idx`.
    /// Si un cluster se complète, il est poussé dans la queue de sortie.
    ///
    /// Retourne `true` si l'entrée a été acceptée (echec si `dev_idx` hors limites).
    pub fn add_page(&self, dev_idx: usize, slot: SwapSlot, src_pfn: u64) -> bool {
        if dev_idx >= MAX_SWAP_DEVICES {
            return false;
        }

        self.stats.pages_added.fetch_add(1, Ordering::Relaxed);

        let entry = ClusterEntry { slot, src_pfn };
        let mut inner = self.inner.lock();

        if let Some(cluster) = inner.accumulators[dev_idx].add_entry(dev_idx as u8, entry) {
            // Cluster complet — pousser dans la queue.
            if !inner.queue.push(cluster) {
                self.stats.queue_overflows.fetch_add(1, Ordering::Relaxed);
            } else {
                self.stats.clusters_formed.fetch_add(1, Ordering::Relaxed);
            }
        }
        true
    }

    /// Flush forcé de tous les accumulateurs partiels.
    /// Typiquement appelé lors d'un arrêt ou d'un reclaim urgent.
    pub fn flush_all(&self) {
        let mut inner = self.inner.lock();
        let r = &mut *inner;
        for acc in r.accumulators.iter_mut() {
            if let Some(partial) = acc.flush() {
                if r.queue.push(partial) {
                    self.stats.partial_flushes.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.stats.queue_overflows.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    /// Flush forcé de l'accumulateur pour un device donné.
    pub fn flush_device(&self, dev_idx: usize) -> bool {
        if dev_idx >= MAX_SWAP_DEVICES {
            return false;
        }
        let mut inner = self.inner.lock();
        if let Some(partial) = inner.accumulators[dev_idx].flush() {
            if inner.queue.push(partial) {
                self.stats.partial_flushes.fetch_add(1, Ordering::Relaxed);
                return true;
            } else {
                self.stats.queue_overflows.fetch_add(1, Ordering::Relaxed);
            }
        }
        false
    }

    /// Retourne le prochain cluster prêt à être écrit sur le device de swap.
    /// Appelé par le thread d'I/O swap.
    pub fn next_cluster(&self) -> Option<SwapCluster> {
        let mut inner = self.inner.lock();
        let c = inner.queue.pop();
        if c.is_some() {
            self.stats.clusters_consumed.fetch_add(1, Ordering::Relaxed);
        }
        c
    }

    /// Nombre de clusters en attente d'écriture.
    #[inline]
    pub fn pending_count(&self) -> usize {
        self.inner.lock().queue.len()
    }

    /// Nombre de pages en attente dans les accumulateurs (pas encore dans la queue).
    pub fn buffered_pages(&self) -> usize {
        let inner = self.inner.lock();
        inner
            .accumulators
            .iter()
            .map(|a| {
                if a.active {
                    a.cluster.count as usize
                } else {
                    0
                }
            })
            .sum()
    }
}

/// Instance globale du gestionnaire de clusters swap.
pub static CLUSTER_MANAGER: ClusterManager = ClusterManager::new();
