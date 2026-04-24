// kernel/src/memory/dma/channels/affinity.rs
//
// Affinité CPU/NUMA des canaux DMA.
//
// Permet d'associer chaque canal DMA à :
//   - un CPU préféré (pour affinité d'interruption et per-CPU pools),
//   - un nœud NUMA (pour privilégier la RAM locale),
//   - un masque CPU (pour les IRQ multi-CPU).
//
// Le gestionnaire de canaux consulte ces affinités lors de l'allocation
// d'un canal pour une requête sur un CPU donné, afin de minimiser la
// latence de la transaction (données locales + IRQ traitée sur le bon CPU).
//
// COUCHE 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::memory::dma::channels::manager::MAX_DMA_CHANNELS;
use crate::memory::dma::core::types::DmaChannelId;

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTES
// ─────────────────────────────────────────────────────────────────────────────

/// Valeur sentinelle : aucune affinité CPU définie.
pub const CPU_AFFINITY_NONE: u8 = u8::MAX;

/// Valeur sentinelle : aucun nœud NUMA préféré.
pub const NUMA_NODE_NONE: u8 = u8::MAX;

/// Nombre maximum de CPUs trackés dans le masque (bits).
pub const MAX_AFFINITY_CPUS: usize = 64;

// ─────────────────────────────────────────────────────────────────────────────
// ENTRÉE D'AFFINITÉ
// ─────────────────────────────────────────────────────────────────────────────

/// Affinité CPU/NUMA d'un canal DMA.
#[repr(C, align(16))]
struct AffinityEntry {
    /// CPU préféré pour IRQ et traitement des completions (255 = aucun).
    preferred_cpu: AtomicU8,
    /// Nœud NUMA préféré pour les allocations mémoire (255 = aucun).
    numa_node: AtomicU8,
    /// Masque des CPUs autorisés à traiter les IRQ de ce canal (64 bits max).
    /// Bit i = 1 → CPU i peut traiter les interruptions de ce canal.
    /// 0 = non configuré (tous les CPUs autorisés).
    cpu_mask: AtomicU64,
    /// Nombre de fois que ce canal a été sélectionné depuis son CPU préféré.
    local_hits: AtomicU32,
    /// Nombre de fois que ce canal a été sélectionné depuis un CPU non-préféré.
    remote_hits: AtomicU32,
}

impl AffinityEntry {
    const fn new() -> Self {
        AffinityEntry {
            preferred_cpu: AtomicU8::new(CPU_AFFINITY_NONE),
            numa_node: AtomicU8::new(NUMA_NODE_NONE),
            cpu_mask: AtomicU64::new(0),
            local_hits: AtomicU32::new(0),
            remote_hits: AtomicU32::new(0),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TABLE D'AFFINITÉS
// ─────────────────────────────────────────────────────────────────────────────

/// Table d'affinités CPU/NUMA pour tous les canaux DMA enregistrés.
///
/// Indexée par `DmaChannelId.0`. Thread-safe : chaque champ est atomique.
pub struct DmaAffinityTable {
    entries: [AffinityEntry; MAX_DMA_CHANNELS],
    /// Statistiques globales.
    pub stats_local_total: AtomicU64,
    pub stats_remote_total: AtomicU64,
}

// SAFETY: DmaAffinityTable utilise uniquement des atomiques.
unsafe impl Sync for DmaAffinityTable {}
unsafe impl Send for DmaAffinityTable {}

impl DmaAffinityTable {
    const fn new() -> Self {
        // SAFETY: AffinityEntry est const-initialisable.
        const ENTRY: AffinityEntry = AffinityEntry::new();
        DmaAffinityTable {
            entries: [ENTRY; MAX_DMA_CHANNELS],
            stats_local_total: AtomicU64::new(0),
            stats_remote_total: AtomicU64::new(0),
        }
    }

    // ── Configuration ────────────────────────────────────────────────────────

    /// Définit le CPU préféré d'un canal.
    ///
    /// `cpu` = `CPU_AFFINITY_NONE` pour supprimer la préférence.
    pub fn set_preferred_cpu(&self, channel: DmaChannelId, cpu: u8) {
        if (channel.0 as usize) < MAX_DMA_CHANNELS {
            self.entries[channel.0 as usize]
                .preferred_cpu
                .store(cpu, Ordering::Relaxed);
        }
    }

    /// Définit le nœud NUMA préféré d'un canal.
    pub fn set_numa_node(&self, channel: DmaChannelId, node: u8) {
        if (channel.0 as usize) < MAX_DMA_CHANNELS {
            self.entries[channel.0 as usize]
                .numa_node
                .store(node, Ordering::Relaxed);
        }
    }

    /// Définit le masque CPU pour les IRQ de ce canal.
    ///
    /// `mask` = 0 → tous les CPUs sont autorisés (aucune contrainte).
    pub fn set_cpu_mask(&self, channel: DmaChannelId, mask: u64) {
        if (channel.0 as usize) < MAX_DMA_CHANNELS {
            self.entries[channel.0 as usize]
                .cpu_mask
                .store(mask, Ordering::Relaxed);
        }
    }

    // ── Lecture ──────────────────────────────────────────────────────────────

    /// Retourne le CPU préféré d'un canal (ou `CPU_AFFINITY_NONE`).
    pub fn preferred_cpu(&self, channel: DmaChannelId) -> u8 {
        if (channel.0 as usize) >= MAX_DMA_CHANNELS {
            return CPU_AFFINITY_NONE;
        }
        self.entries[channel.0 as usize]
            .preferred_cpu
            .load(Ordering::Relaxed)
    }

    /// Retourne le nœud NUMA préféré d'un canal (ou `NUMA_NODE_NONE`).
    pub fn numa_node(&self, channel: DmaChannelId) -> u8 {
        if (channel.0 as usize) >= MAX_DMA_CHANNELS {
            return NUMA_NODE_NONE;
        }
        self.entries[channel.0 as usize]
            .numa_node
            .load(Ordering::Relaxed)
    }

    /// Retourne le masque CPU d'un canal (0 = aucune contrainte).
    pub fn cpu_mask(&self, channel: DmaChannelId) -> u64 {
        if (channel.0 as usize) >= MAX_DMA_CHANNELS {
            return 0;
        }
        self.entries[channel.0 as usize]
            .cpu_mask
            .load(Ordering::Relaxed)
    }

    // ── Sélection avec accounting ────────────────────────────────────────────

    /// Indique qu'un canal a été utilisé depuis `cpu_id` (pour statistiques).
    ///
    /// Met à jour les compteurs `local_hits` / `remote_hits` selon que
    /// `cpu_id` correspond au CPU préféré du canal ou non.
    pub fn record_usage(&self, channel: DmaChannelId, cpu_id: u8) {
        if (channel.0 as usize) >= MAX_DMA_CHANNELS {
            return;
        }
        let entry = &self.entries[channel.0 as usize];
        let pref = entry.preferred_cpu.load(Ordering::Relaxed);
        if pref == CPU_AFFINITY_NONE || pref == cpu_id {
            entry.local_hits.fetch_add(1, Ordering::Relaxed);
            self.stats_local_total.fetch_add(1, Ordering::Relaxed);
        } else {
            entry.remote_hits.fetch_add(1, Ordering::Relaxed);
            self.stats_remote_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    // ── Sélection du meilleur canal pour un CPU donné ────────────────────────

    /// Retourne l'indice du canal le plus proche de `cpu_id` parmi les canaux
    /// dont les IDs sont listés dans `candidates`.
    ///
    /// Critères de sélection (ordre décroissant de priorité) :
    ///   1. Canal dont `preferred_cpu == cpu_id` (match exact).
    ///   2. Canal dont `cpu_mask` contient `cpu_id` (affinité partielle).
    ///   3. Canal sans affinité définie (fallback).
    ///
    /// Retourne `None` si `candidates` est vide.
    pub fn best_for_cpu<'a>(
        &self,
        candidates: &'a [DmaChannelId],
        cpu_id: u8,
    ) -> Option<DmaChannelId> {
        let mut best_exact: Option<DmaChannelId> = None;
        let mut best_partial: Option<DmaChannelId> = None;
        let mut best_any: Option<DmaChannelId> = None;

        for &ch in candidates {
            if (ch.0 as usize) >= MAX_DMA_CHANNELS {
                continue;
            }
            let entry = &self.entries[ch.0 as usize];
            let pref = entry.preferred_cpu.load(Ordering::Relaxed);
            let mask = entry.cpu_mask.load(Ordering::Relaxed);

            if pref != CPU_AFFINITY_NONE && pref == cpu_id {
                best_exact = Some(ch);
                break; // Match parfait, pas besoin de continuer.
            }
            if mask != 0 && mask & (1u64 << (cpu_id as u64)) != 0 && best_partial.is_none() {
                best_partial = Some(ch);
            }
            if best_any.is_none() {
                best_any = Some(ch);
            }
        }

        best_exact.or(best_partial).or(best_any)
    }

    /// Taux de localité global : proportion des accès depuis le CPU préféré.
    pub fn locality_rate(&self) -> u32 {
        let local = self.stats_local_total.load(Ordering::Relaxed);
        let remote = self.stats_remote_total.load(Ordering::Relaxed);
        let total = local + remote;
        if total == 0 {
            return 100;
        }
        ((local * 100) / total) as u32
    }
}

/// Table d'affinités DMA globale.
pub static DMA_AFFINITY: DmaAffinityTable = DmaAffinityTable::new();
