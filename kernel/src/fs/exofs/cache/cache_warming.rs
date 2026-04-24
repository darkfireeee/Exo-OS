//! cache_warming.rs — Pré-chargement du cache ExoFS au démarrage (no_std).
//!
//! `CacheWarmer` : file de priorité pour le pré-chargement de blobs.
//! `WarmingStrategy` : politique de prélecture.
//! Règles : RECUR-01, OOM-02, ARITH-02.

extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// WarmingStrategy
// ─────────────────────────────────────────────────────────────────────────────

/// Stratégie de pré-chargement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarmingStrategy {
    /// Charge les blobs dans l'ordre séquentiel de la file.
    Sequential,
    /// Charge en priorité les blobs les plus fréquemment accédés.
    ByFrequency,
    /// Charge en priorité les blobs les plus récemment accédés.
    ByRecency,
    /// Sélectif : seulement les blobs avec score > seuil.
    Selective { min_score: u32 },
}

// ─────────────────────────────────────────────────────────────────────────────
// WarmEntry
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée dans la file de warming.
#[derive(Clone, Debug)]
pub struct WarmEntry {
    /// Identifiant du blob.
    pub blob_id: BlobId,
    /// Score combiné (fréquence * 1000 + recéence_relative).
    pub score: u32,
    /// Taille estimée en octets.
    pub size_hint: u64,
    /// Ticks du dernier accès.
    pub last_ticks: u64,
    /// Nombre d'accès historiques.
    pub freq: u32,
}

impl WarmEntry {
    pub fn new(blob_id: BlobId, freq: u32, last_ticks: u64, size_hint: u64) -> Self {
        let score = freq.saturating_mul(1000).saturating_add(
            // normalise la récence (ticks récents = score plus haut)
            if last_ticks > 0 {
                (last_ticks % 1000) as u32
            } else {
                0
            },
        );
        Self {
            blob_id,
            score,
            size_hint,
            last_ticks,
            freq,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CacheWarmer
// ─────────────────────────────────────────────────────────────────────────────

/// File de pré-chargement de blobs avec stratégie ordonnable.
pub struct CacheWarmer {
    queue: Vec<WarmEntry>,
    strategy: WarmingStrategy,
    warmed: u64,
    skipped: u64,
    /// Taille maximale de la file.
    max_queue: usize,
}

impl CacheWarmer {
    pub fn new(strategy: WarmingStrategy, max_queue: usize) -> Self {
        Self {
            queue: Vec::new(),
            strategy,
            warmed: 0,
            skipped: 0,
            max_queue: max_queue.max(1),
        }
    }

    // ── File ─────────────────────────────────────────────────────────────────

    /// Ajoute un blob à la file de warming.
    pub fn enqueue(&mut self, entry: WarmEntry) -> ExofsResult<()> {
        if self.queue.len() >= self.max_queue {
            return Err(ExofsError::NoSpace);
        }
        self.queue
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.queue.push(entry);
        Ok(())
    }

    /// Trie la file selon la stratégie.
    pub fn sort_queue(&mut self) {
        match self.strategy {
            WarmingStrategy::Sequential => { /* ordre naturel, ne rien faire */ }
            WarmingStrategy::ByFrequency => {
                self.queue.sort_unstable_by(|a, b| b.freq.cmp(&a.freq));
            }
            WarmingStrategy::ByRecency => {
                self.queue
                    .sort_unstable_by(|a, b| b.last_ticks.cmp(&a.last_ticks));
            }
            WarmingStrategy::Selective { min_score } => {
                self.queue.retain(|e| e.score >= min_score);
                self.queue.sort_unstable_by(|a, b| b.score.cmp(&a.score));
            }
        }
    }

    /// Retire les `n` prochains blobs à charger.
    pub fn next_batch(&mut self, n: usize) -> Vec<WarmEntry> {
        let take = n.min(self.queue.len());
        // Itératif : on draine les n premiers éléments.
        let mut batch = Vec::with_capacity(take);
        let remaining = self.queue.split_off(take);
        batch.extend(self.queue.drain(..));
        self.queue = remaining;
        self.warmed = self.warmed.wrapping_add(batch.len() as u64);
        batch
    }

    // ── Statistiques ──────────────────────────────────────────────────────────

    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }
    pub fn warmed(&self) -> u64 {
        self.warmed
    }
    pub fn skipped(&self) -> u64 {
        self.skipped
    }
    pub fn strategy(&self) -> WarmingStrategy {
        self.strategy
    }
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Vide la file.
    pub fn clear(&mut self) {
        self.queue.clear();
    }

    /// Taille totale estimée des blobs en queue.
    pub fn queued_bytes(&self) -> u64 {
        self.queue
            .iter()
            .map(|e| e.size_hint)
            .fold(0u64, |acc, v| acc.saturating_add(v))
    }

    /// Supprime les blobs déjà présents dans le cache (prédicat d'appartenance).
    pub fn dedup_with<F>(&mut self, already_cached: F)
    where
        F: Fn(&BlobId) -> bool,
    {
        let before = self.queue.len();
        self.queue.retain(|e| !already_cached(&e.blob_id));
        let removed = before.saturating_sub(self.queue.len());
        self.skipped = self.skipped.wrapping_add(removed as u64);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(b: u8) -> BlobId {
        BlobId([b; 32])
    }
    fn entry(b: u8, freq: u32, ticks: u64) -> WarmEntry {
        WarmEntry::new(blob(b), freq, ticks, 512)
    }

    #[test]
    fn test_enqueue_and_len() {
        let mut w = CacheWarmer::new(WarmingStrategy::Sequential, 10);
        w.enqueue(entry(1, 5, 100)).unwrap();
        w.enqueue(entry(2, 3, 200)).unwrap();
        assert_eq!(w.queue_len(), 2);
    }

    #[test]
    fn test_enqueue_overflow() {
        let mut w = CacheWarmer::new(WarmingStrategy::Sequential, 2);
        w.enqueue(entry(1, 1, 1)).unwrap();
        w.enqueue(entry(2, 1, 1)).unwrap();
        assert!(w.enqueue(entry(3, 1, 1)).is_err());
    }

    #[test]
    fn test_sort_by_frequency() {
        let mut w = CacheWarmer::new(WarmingStrategy::ByFrequency, 10);
        w.enqueue(entry(1, 1, 0)).unwrap();
        w.enqueue(entry(2, 9, 0)).unwrap();
        w.sort_queue();
        let batch = w.next_batch(1);
        assert_eq!(batch[0].blob_id, blob(2));
    }

    #[test]
    fn test_sort_by_recency() {
        let mut w = CacheWarmer::new(WarmingStrategy::ByRecency, 10);
        w.enqueue(entry(1, 1, 50)).unwrap();
        w.enqueue(entry(2, 1, 200)).unwrap();
        w.sort_queue();
        let batch = w.next_batch(1);
        assert_eq!(batch[0].blob_id, blob(2));
    }

    #[test]
    fn test_selective_filters() {
        let mut w = CacheWarmer::new(WarmingStrategy::Selective { min_score: 5000 }, 10);
        w.enqueue(entry(1, 1, 0)).unwrap(); // score = 1000
        w.enqueue(entry(2, 6, 0)).unwrap(); // score = 6000
        w.sort_queue();
        assert_eq!(w.queue_len(), 1);
        assert_eq!(w.queue[0].blob_id, blob(2));
    }

    #[test]
    fn test_next_batch_reduces_queue() {
        let mut w = CacheWarmer::new(WarmingStrategy::Sequential, 10);
        for i in 0..5u8 {
            w.enqueue(entry(i, 1, 0)).unwrap();
        }
        let b = w.next_batch(3);
        assert_eq!(b.len(), 3);
        assert_eq!(w.queue_len(), 2);
    }

    #[test]
    fn test_warmed_counter() {
        let mut w = CacheWarmer::new(WarmingStrategy::Sequential, 10);
        w.enqueue(entry(1, 1, 0)).unwrap();
        w.enqueue(entry(2, 1, 0)).unwrap();
        w.next_batch(2);
        assert_eq!(w.warmed(), 2);
    }

    #[test]
    fn test_queued_bytes() {
        let mut w = CacheWarmer::new(WarmingStrategy::Sequential, 10);
        w.enqueue(WarmEntry::new(blob(1), 1, 0, 1024)).unwrap();
        assert_eq!(w.queued_bytes(), 1024);
    }

    #[test]
    fn test_dedup_with() {
        let mut w = CacheWarmer::new(WarmingStrategy::Sequential, 10);
        w.enqueue(entry(1, 1, 0)).unwrap();
        w.enqueue(entry(2, 1, 0)).unwrap();
        w.dedup_with(|b| *b == blob(1));
        assert_eq!(w.queue_len(), 1);
        assert_eq!(w.skipped(), 1);
    }

    #[test]
    fn test_clear() {
        let mut w = CacheWarmer::new(WarmingStrategy::Sequential, 10);
        w.enqueue(entry(1, 1, 0)).unwrap();
        w.clear();
        assert!(w.is_empty());
    }
}

// ── Extensions CacheWarmer ───────────────────────────────────────────────────

impl CacheWarmer {
    /// Insère un lot d'entrées sans échouer sur dépassement — retourne le nb inséré.
    pub fn enqueue_batch_best_effort(&mut self, entries: &[WarmEntry]) -> usize {
        let mut count = 0usize;
        for e in entries {
            if self.enqueue(e.clone()).is_ok() {
                count = count.wrapping_add(1);
            }
        }
        count
    }

    /// Inspecte le premier élément sans le consommer.
    pub fn peek(&self) -> Option<&WarmEntry> {
        self.queue.first()
    }

    /// Vide et retourne tous les éléments.
    pub fn drain_all(&mut self) -> Vec<WarmEntry> {
        let all: Vec<WarmEntry> = self.queue.drain(..).collect();
        self.warmed = self.warmed.wrapping_add(all.len() as u64);
        all
    }

    /// Retourne les blobs à score élevé sans les enlever de la file.
    pub fn high_priority(&self, min_score: u32) -> Vec<&WarmEntry> {
        self.queue.iter().filter(|e| e.score >= min_score).collect()
    }

    /// Replace les entrées dont le score est en-dessous de `min_score`.
    pub fn prune_below(&mut self, min_score: u32) -> usize {
        let before = self.queue.len();
        self.queue.retain(|e| e.score >= min_score);
        let removed = before.saturating_sub(self.queue.len());
        self.skipped = self.skipped.wrapping_add(removed as u64);
        removed
    }

    /// Nombre de blobs en attente avec size_hint > `threshold`.
    pub fn count_large(&self, threshold: u64) -> usize {
        self.queue
            .iter()
            .filter(|e| e.size_hint > threshold)
            .count()
    }
}
