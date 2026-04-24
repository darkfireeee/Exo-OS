//! prefetch.rs — Moteur de pré-chargement de blobs ExoFS (no_std).
//!
//! Ce module fournit :
//!  - `PrefetchStrategy` : séquentiel / aléatoire / stride / adaptatif.
//!  - `PrefetchEntry`    : entrée dans la file de pré-chargement.
//!  - `PrefetchConfig`   : configuration du moteur.
//!  - `PrefetchQueue`    : file simple (Vec, LRU-eviction).
//!  - `Prefetcher`       : moteur principal.
//!  - `PrefetchStats`    : statistiques.
//!
//! RECUR-01 : boucles while — aucune récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add/mul.

extern crate alloc;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─── PrefetchStrategy ─────────────────────────────────────────────────────────

/// Stratégie de pré-chargement.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum PrefetchStrategy {
    Sequential = 0,
    Random = 1,
    Stride = 2,
    AdaptiveSeq = 3,
    Disabled = 4,
}

impl PrefetchStrategy {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Sequential,
            1 => Self::Random,
            2 => Self::Stride,
            3 => Self::AdaptiveSeq,
            _ => Self::Disabled,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sequential => "sequential",
            Self::Random => "random",
            Self::Stride => "stride",
            Self::AdaptiveSeq => "adaptive_seq",
            Self::Disabled => "disabled",
        }
    }
    pub fn is_active(self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

// ─── PrefetchEntry ────────────────────────────────────────────────────────────

/// Entrée dans la file de pré-chargement.
#[derive(Clone, Debug)]
pub struct PrefetchEntry {
    pub blob_id: [u8; 32],
    pub priority: u8,
    pub triggered_at: u64, // timestamp TTL (simulé en ticks)
    pub strategy: PrefetchStrategy,
    pub fetched: bool,
}

impl PrefetchEntry {
    pub fn new(blob_id: [u8; 32], strategy: PrefetchStrategy, ts: u64) -> Self {
        Self {
            blob_id,
            priority: 128,
            triggered_at: ts,
            strategy,
            fetched: false,
        }
    }

    pub fn urgent(blob_id: [u8; 32], ts: u64) -> Self {
        Self {
            blob_id,
            priority: 255,
            triggered_at: ts,
            strategy: PrefetchStrategy::Sequential,
            fetched: false,
        }
    }

    pub fn with_priority(mut self, p: u8) -> Self {
        self.priority = p;
        self
    }
    pub fn mark_fetched(&mut self) {
        self.fetched = true;
    }
}

// ─── PrefetchConfig ───────────────────────────────────────────────────────────

/// Configuration du moteur de pré-chargement.
#[derive(Clone, Copy, Debug)]
pub struct PrefetchConfig {
    pub max_queue: u32,
    pub prefetch_distance: u32,  // nombre de blobs à pré-charger en avance
    pub adaptive_threshold: u32, // hits consécutifs avant de passer en AdaptiveSeq
    pub ttl_ticks: u64,          // durée de vie max d'une entrée
    pub default_strategy: PrefetchStrategy,
}

impl PrefetchConfig {
    pub fn default() -> Self {
        Self {
            max_queue: 64,
            prefetch_distance: 4,
            adaptive_threshold: 3,
            ttl_ticks: 10_000,
            default_strategy: PrefetchStrategy::Sequential,
        }
    }

    pub fn aggressive() -> Self {
        Self {
            max_queue: 256,
            prefetch_distance: 16,
            adaptive_threshold: 1,
            ttl_ticks: 50_000,
            default_strategy: PrefetchStrategy::AdaptiveSeq,
        }
    }

    pub fn validate(&self) -> ExofsResult<()> {
        if self.max_queue == 0 || self.prefetch_distance == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }
}

// ─── PrefetchQueue ────────────────────────────────────────────────────────────

/// File de pré-chargement (Vec + éviction LRU simplifiée, RECUR-01 : while).
pub struct PrefetchQueue {
    entries: Vec<PrefetchEntry>,
    max_size: u32,
}

impl PrefetchQueue {
    pub fn new(max_size: u32) -> Self {
        Self {
            entries: Vec::new(),
            max_size,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn is_full(&self) -> bool {
        self.entries.len() as u32 >= self.max_size
    }

    /// Ajoute une entrée (si déjà présente → mise à jour, sinon éviction LRU si pleine).
    ///
    /// OOM-02 : try_reserve avant push.
    pub fn enqueue(&mut self, entry: PrefetchEntry) -> ExofsResult<()> {
        // Chercher un doublon (RECUR-01 : while)
        let mut i = 0usize;
        while i < self.entries.len() {
            if self.entries[i].blob_id == entry.blob_id {
                self.entries[i].priority = self.entries[i].priority.max(entry.priority);
                self.entries[i].triggered_at = entry.triggered_at;
                return Ok(());
            }
            i = i.wrapping_add(1);
        }
        // Éviction LRU si pleine (enlever priorité la plus basse)
        if self.is_full() {
            let mut min_idx = 0usize;
            let mut min_prio = 255u8;
            let mut j = 0usize;
            while j < self.entries.len() {
                if self.entries[j].priority < min_prio {
                    min_prio = self.entries[j].priority;
                    min_idx = j;
                }
                j = j.wrapping_add(1);
            }
            self.entries.swap_remove(min_idx);
        }
        self.entries
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.entries.push(entry);
        Ok(())
    }

    /// Dépile l'entrée de plus haute priorité (RECUR-01 : while).
    pub fn dequeue(&mut self) -> Option<PrefetchEntry> {
        if self.entries.is_empty() {
            return None;
        }
        let mut max_idx = 0usize;
        let mut max_prio = 0u8;
        let mut i = 0usize;
        while i < self.entries.len() {
            if self.entries[i].priority > max_prio {
                max_prio = self.entries[i].priority;
                max_idx = i;
            }
            i = i.wrapping_add(1);
        }
        Some(self.entries.swap_remove(max_idx))
    }

    /// Supprime les entrées expirées selon le TTL (RECUR-01 : while).
    pub fn evict_expired(&mut self, current_tick: u64, ttl: u64) -> u32 {
        let mut removed = 0u32;
        let mut i = 0usize;
        while i < self.entries.len() {
            let age = current_tick.saturating_sub(self.entries[i].triggered_at);
            if age > ttl {
                self.entries.swap_remove(i);
                removed = removed.saturating_add(1);
                // Ne pas incrémenter i (swap_remove déplace le dernier ici)
            } else {
                i = i.wrapping_add(1);
            }
        }
        removed
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

// ─── PrefetchStats ────────────────────────────────────────────────────────────

/// Statistiques du moteur de pré-chargement.
#[derive(Clone, Copy, Debug, Default)]
pub struct PrefetchStats {
    pub triggered: u64,
    pub fetched_ok: u64,
    pub fetched_err: u64,
    pub evictions: u64,
    pub hits: u64,   // accès servis depuis le pré-chargement
    pub misses: u64, // accès non pré-chargés
}

impl PrefetchStats {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn is_clean(&self) -> bool {
        self.fetched_err == 0
    }
    pub fn hit_ratio_pct10(&self) -> u32 {
        let total = self.hits.saturating_add(self.misses);
        if total == 0 {
            return 0;
        }
        self.hits
            .saturating_mul(1000)
            .checked_div(total)
            .unwrap_or(0) as u32
    }
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

// ─── Prefetcher ───────────────────────────────────────────────────────────────

/// Moteur de pré-chargement principal.
pub struct Prefetcher {
    config: PrefetchConfig,
    queue: PrefetchQueue,
    stats: PrefetchStats,
    tick: u64,
}

impl Prefetcher {
    pub fn new(config: PrefetchConfig) -> ExofsResult<Self> {
        config.validate()?;
        let q_size = config.max_queue;
        Ok(Self {
            config,
            queue: PrefetchQueue::new(q_size),
            stats: PrefetchStats::new(),
            tick: 0,
        })
    }

    /// Déclenche un pré-chargement pour `blob_id`.
    pub fn trigger(&mut self, blob_id: [u8; 32], strategy: PrefetchStrategy) -> ExofsResult<()> {
        if !strategy.is_active() {
            return Ok(());
        }
        let entry = PrefetchEntry::new(blob_id, strategy, self.tick);
        self.queue.enqueue(entry)?;
        self.stats.triggered = self.stats.triggered.saturating_add(1);
        Ok(())
    }

    /// Retourne le prochain candidat à pré-charger.
    pub fn next_candidate(&mut self) -> Option<PrefetchEntry> {
        self.queue.dequeue()
    }

    /// Signale un succès de pré-chargement.
    pub fn report_ok(&mut self) {
        self.stats.fetched_ok = self.stats.fetched_ok.saturating_add(1);
    }

    /// Signale un échec de pré-chargement.
    pub fn report_err(&mut self) {
        self.stats.fetched_err = self.stats.fetched_err.saturating_add(1);
    }

    /// Signale un hit (blob pré-chargé utilisé).
    pub fn report_hit(&mut self) {
        self.stats.hits = self.stats.hits.saturating_add(1);
    }

    /// Signale un miss (blob non pré-chargé).
    pub fn report_miss(&mut self) {
        self.stats.misses = self.stats.misses.saturating_add(1);
    }

    /// Avance le tick interne et évicte les entrées expirées.
    pub fn advance_tick(&mut self, ticks: u64) {
        self.tick = self.tick.saturating_add(ticks);
        let evicted = self.queue.evict_expired(self.tick, self.config.ttl_ticks);
        self.stats.evictions = self.stats.evictions.saturating_add(evicted as u64);
    }

    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }
    pub fn stats(&self) -> &PrefetchStats {
        &self.stats
    }
    pub fn reset_stats(&mut self) {
        self.stats.reset();
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    fn make_id(n: u8) -> [u8; 32] {
        let mut id = [0u8; 32];
        id[0] = n;
        id
    }

    #[test]
    fn test_prefetch_strategy() {
        assert!(PrefetchStrategy::Sequential.is_active());
        assert!(!PrefetchStrategy::Disabled.is_active());
        assert_eq!(PrefetchStrategy::from_u8(0), PrefetchStrategy::Sequential);
    }

    #[test]
    fn test_prefetch_queue_enqueue_dequeue() {
        let mut q = PrefetchQueue::new(8);
        q.enqueue(PrefetchEntry::new(
            make_id(1),
            PrefetchStrategy::Sequential,
            0,
        ))
        .expect("ok");
        q.enqueue(PrefetchEntry::new(make_id(2), PrefetchStrategy::Random, 0))
            .expect("ok");
        assert_eq!(q.len(), 2);
        let e = q.dequeue().expect("some");
        assert_eq!(q.len(), 1);
        assert!(e.blob_id[0] == 1 || e.blob_id[0] == 2);
    }

    #[test]
    fn test_prefetch_queue_dedup() {
        let mut q = PrefetchQueue::new(8);
        let id = make_id(5);
        q.enqueue(PrefetchEntry::new(id, PrefetchStrategy::Sequential, 0))
            .expect("ok");
        q.enqueue(PrefetchEntry::new(id, PrefetchStrategy::Sequential, 1))
            .expect("ok");
        assert_eq!(q.len(), 1); // déduplication
    }

    #[test]
    fn test_prefetch_queue_priority_eviction() {
        let mut q = PrefetchQueue::new(2);
        q.enqueue(
            PrefetchEntry::new(make_id(1), PrefetchStrategy::Sequential, 0).with_priority(10),
        )
        .expect("ok");
        q.enqueue(
            PrefetchEntry::new(make_id(2), PrefetchStrategy::Sequential, 0).with_priority(200),
        )
        .expect("ok");
        q.enqueue(
            PrefetchEntry::new(make_id(3), PrefetchStrategy::Sequential, 0).with_priority(100),
        )
        .expect("ok");
        // la priorité 10 doit avoir été évincée
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn test_prefetch_queue_evict_expired() {
        let mut q = PrefetchQueue::new(8);
        q.enqueue(PrefetchEntry::new(
            make_id(1),
            PrefetchStrategy::Sequential,
            0,
        ))
        .expect("ok");
        q.enqueue(PrefetchEntry::new(
            make_id(2),
            PrefetchStrategy::Sequential,
            0,
        ))
        .expect("ok");
        let evicted = q.evict_expired(20_000, 5_000);
        assert_eq!(evicted, 2);
        assert!(q.is_empty());
    }

    #[test]
    fn test_prefetcher_trigger() {
        let mut p = Prefetcher::new(PrefetchConfig::default()).expect("ok");
        p.trigger(make_id(1), PrefetchStrategy::Sequential)
            .expect("ok");
        assert_eq!(p.stats().triggered, 1);
        assert_eq!(p.queue_len(), 1);
    }

    #[test]
    fn test_prefetcher_next_candidate() {
        let mut p = Prefetcher::new(PrefetchConfig::default()).expect("ok");
        p.trigger(make_id(1), PrefetchStrategy::Sequential)
            .expect("ok");
        let cand = p.next_candidate().expect("some");
        assert_eq!(cand.blob_id[0], 1);
    }

    #[test]
    fn test_prefetcher_stats() {
        let mut p = Prefetcher::new(PrefetchConfig::default()).expect("ok");
        p.trigger(make_id(1), PrefetchStrategy::Sequential)
            .expect("ok");
        p.report_ok();
        p.report_hit();
        p.report_miss();
        assert_eq!(p.stats().fetched_ok, 1);
        assert_eq!(p.stats().hits, 1);
        assert_eq!(p.stats().misses, 1);
    }

    #[test]
    fn test_hit_ratio() {
        let mut p = Prefetcher::new(PrefetchConfig::default()).expect("ok");
        p.report_hit();
        p.report_hit();
        p.report_miss();
        assert_eq!(p.stats().hit_ratio_pct10(), 666);
    }

    #[test]
    fn test_prefetcher_disabled_strategy() {
        let mut p = Prefetcher::new(PrefetchConfig::default()).expect("ok");
        p.trigger(make_id(1), PrefetchStrategy::Disabled)
            .expect("ok");
        assert_eq!(p.queue_len(), 0); // rien ajouté
    }

    #[test]
    fn test_advance_tick_eviction() {
        let cfg = PrefetchConfig {
            ttl_ticks: 100,
            ..PrefetchConfig::default()
        };
        let mut p = Prefetcher::new(cfg).expect("ok");
        p.trigger(make_id(1), PrefetchStrategy::Sequential)
            .expect("ok");
        p.advance_tick(200);
        assert_eq!(p.queue_len(), 0);
        assert_eq!(p.stats().evictions, 1);
    }

    #[test]
    fn test_config_validate() {
        let mut cfg = PrefetchConfig::default();
        assert!(cfg.validate().is_ok());
        cfg.max_queue = 0;
        assert!(cfg.validate().is_err());
    }
}

// ─── Heuristique stride ───────────────────────────────────────────────────────

/// Calcule les N prochains blob_ids selon une distance stride (ARITH-02).
///
/// `base_id[31]` est utilisé comme compteur de stride.
pub fn stride_candidates(base_id: [u8; 32], stride: u8, count: u8) -> [[u8; 32]; 4] {
    let mut out = [[0u8; 32]; 4];
    let max = (count as usize).min(4);
    let mut i = 0usize;
    while i < max {
        let mut id = base_id;
        id[31] = base_id[31].wrapping_add(stride.wrapping_mul(i as u8 + 1));
        out[i] = id;
        i = i.wrapping_add(1);
    }
    out
}
