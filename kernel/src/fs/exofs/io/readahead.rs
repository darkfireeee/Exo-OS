//! readahead.rs — Fenêtre de lecture avancée adaptative (no_std).
//!
//! Ce module fournit :
//!  - `ReadaheadPolicy`  : Disabled / Fixed / Adaptive.
//!  - `ReadaheadWindow`  : fenêtre démarrée en bloc + longueur + score.
//!  - `ReadaheadEngine`  : moteur avec hint_access + adjustment.
//!  - `ReadaheadStats`   : statistiques.
//!  - `BlockAccessLog`   : journal circulaire d'accès pour la détection séq.
//!
//! RECUR-01 : boucles while — aucune récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add/mul.

extern crate alloc;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─── ReadaheadPolicy ─────────────────────────────────────────────────────────

/// Politique de read-ahead.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum ReadaheadPolicy {
    Disabled = 0,
    Fixed = 1,
    Adaptive = 2,
}

impl ReadaheadPolicy {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Disabled,
            1 => Self::Fixed,
            _ => Self::Adaptive,
        }
    }
    pub fn is_active(self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

// ─── ReadaheadWindow ─────────────────────────────────────────────────────────

/// Fenêtre de read-ahead.
#[derive(Clone, Debug)]
pub struct ReadaheadWindow {
    pub start_block: u64,
    pub length: u32,     // nombre de blocs pré-chargés
    pub hit_count: u32,  // accès dans la fenêtre
    pub miss_count: u32, // miss dans la fenêtre
    pub active: bool,
}

impl ReadaheadWindow {
    pub fn new(start_block: u64, length: u32) -> Self {
        Self {
            start_block,
            length,
            hit_count: 0,
            miss_count: 0,
            active: true,
        }
    }

    pub fn end_block(&self) -> u64 {
        self.start_block.saturating_add(self.length as u64)
    }

    pub fn contains(&self, block: u64) -> bool {
        self.active && block >= self.start_block && block < self.end_block()
    }

    pub fn record_hit(&mut self) {
        self.hit_count = self.hit_count.saturating_add(1);
    }
    pub fn record_miss(&mut self) {
        self.miss_count = self.miss_count.saturating_add(1);
    }

    pub fn hit_ratio_pct10(&self) -> u32 {
        let total = self.hit_count.saturating_add(self.miss_count);
        if total == 0 {
            return 0;
        }
        self.hit_count
            .saturating_mul(1000)
            .checked_div(total)
            .unwrap_or(0)
    }

    /// Avance la fenêtre au-delà du bloc courant.
    pub fn advance(&mut self, current_block: u64, distance: u32) {
        self.start_block = current_block.saturating_add(1);
        self.length = distance;
        self.hit_count = 0;
        self.miss_count = 0;
    }
}

// ─── BlockAccessLog (ring circulaire) ────────────────────────────────────────

const ACCESS_LOG_SIZE: usize = 32;

/// Journal circulaire des N derniers accès (pour détecter les accès séquentiels).
pub struct BlockAccessLog {
    log: [u64; ACCESS_LOG_SIZE],
    head: usize,
    count: usize,
}

impl BlockAccessLog {
    pub const fn new() -> Self {
        Self {
            log: [0u64; ACCESS_LOG_SIZE],
            head: 0,
            count: 0,
        }
    }

    pub fn push(&mut self, block: u64) {
        self.log[self.head] = block;
        self.head = self.head.wrapping_add(1) % ACCESS_LOG_SIZE;
        self.count = self.count.saturating_add(1).min(ACCESS_LOG_SIZE);
    }

    /// Détecte si les derniers accès sont séquentiels (RECUR-01 : while).
    pub fn is_sequential(&self, window: u32) -> bool {
        if self.count < 2 {
            return false;
        }
        let n = (window as usize).min(self.count).min(ACCESS_LOG_SIZE);
        let mut i = 1usize;
        while i < n {
            let cur = self.head.wrapping_sub(i).wrapping_add(ACCESS_LOG_SIZE) % ACCESS_LOG_SIZE;
            let prev =
                self.head.wrapping_sub(i + 1).wrapping_add(ACCESS_LOG_SIZE) % ACCESS_LOG_SIZE;
            let diff = self.log[cur].wrapping_sub(self.log[prev]);
            if diff != 1 {
                return false;
            }
            i = i.wrapping_add(1);
        }
        true
    }

    pub fn last(&self) -> Option<u64> {
        if self.count == 0 {
            return None;
        }
        let idx = self.head.wrapping_sub(1).wrapping_add(ACCESS_LOG_SIZE) % ACCESS_LOG_SIZE;
        Some(self.log[idx])
    }
}

// ─── ReadaheadStats ───────────────────────────────────────────────────────────

/// Statistiques du read-ahead.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReadaheadStats {
    pub windows_created: u64,
    pub blocks_prefetched: u64,
    pub hits: u64,
    pub misses: u64,
    pub window_advances: u64,
    pub policy_upgrades: u64, // Disabled→Fixed→Adaptive
}

impl ReadaheadStats {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn is_clean(&self) -> bool {
        true
    } // le readahead ne génère pas d'erreurs dures

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

// ─── ReadaheadEngine ─────────────────────────────────────────────────────────

/// Moteur de read-ahead avec fenêtre adaptative.
///
/// RECUR-01 : toutes les boucles while.
pub struct ReadaheadEngine {
    policy: ReadaheadPolicy,
    window: ReadaheadWindow,
    log: BlockAccessLog,
    stats: ReadaheadStats,
    fixed_distance: u32,
    adaptive_distance: u32,
    seq_threshold: u32, // accès séquentiels consécutifs pour passer en Adaptive
}

impl ReadaheadEngine {
    pub fn new(policy: ReadaheadPolicy) -> Self {
        Self {
            policy,
            window: ReadaheadWindow::new(0, 8),
            log: BlockAccessLog::new(),
            stats: ReadaheadStats::new(),
            fixed_distance: 8,
            adaptive_distance: 16,
            seq_threshold: 3,
        }
    }

    pub fn policy(&self) -> ReadaheadPolicy {
        self.policy
    }

    /// Signale un accès à un bloc et met à jour la fenêtre.
    pub fn hint_access(&mut self, block: u64) {
        self.log.push(block);

        if !self.policy.is_active() {
            return;
        }

        if self.window.contains(block) {
            self.window.record_hit();
            self.stats.hits = self.stats.hits.saturating_add(1);
        } else {
            self.window.record_miss();
            self.stats.misses = self.stats.misses.saturating_add(1);
        }

        // Passage Adaptive si accès séquentiels
        if self.policy == ReadaheadPolicy::Fixed && self.log.is_sequential(self.seq_threshold) {
            self.policy = ReadaheadPolicy::Adaptive;
            self.stats.policy_upgrades = self.stats.policy_upgrades.saturating_add(1);
        }
    }

    /// Retourne les prochains blocs à pré-charger (RECUR-01 : while).
    pub fn next_blocks_to_prefetch(&self) -> Vec<u64> {
        if !self.policy.is_active() {
            return Vec::new();
        }
        let distance = match self.policy {
            ReadaheadPolicy::Adaptive => self.adaptive_distance,
            _ => self.fixed_distance,
        };
        let base = self.log.last().unwrap_or(0).saturating_add(1);
        let mut out = Vec::new();
        let _ = out.try_reserve(distance as usize);
        let mut i = 0u32;
        while i < distance {
            out.push(base.saturating_add(i as u64));
            i = i.wrapping_add(1);
        }
        out
    }

    /// Avance la fenêtre selon le résultat des derniers accès.
    pub fn adjust_window(&mut self, block: u64) {
        let distance = match self.policy {
            ReadaheadPolicy::Adaptive => self.adaptive_distance,
            _ => self.fixed_distance,
        };
        self.window.advance(block, distance);
        self.stats.windows_created = self.stats.windows_created.saturating_add(1);
        self.stats.blocks_prefetched = self.stats.blocks_prefetched.saturating_add(distance as u64);
        self.stats.window_advances = self.stats.window_advances.saturating_add(1);
    }

    pub fn stats(&self) -> &ReadaheadStats {
        &self.stats
    }
    pub fn reset_stats(&mut self) {
        self.stats.reset();
    }
    pub fn window(&self) -> &ReadaheadWindow {
        &self.window
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_from_u8() {
        assert_eq!(ReadaheadPolicy::from_u8(0), ReadaheadPolicy::Disabled);
        assert_eq!(ReadaheadPolicy::from_u8(1), ReadaheadPolicy::Fixed);
        assert_eq!(ReadaheadPolicy::from_u8(2), ReadaheadPolicy::Adaptive);
    }

    #[test]
    fn test_window_contains() {
        let w = ReadaheadWindow::new(10, 5);
        assert!(w.contains(10));
        assert!(w.contains(14));
        assert!(!w.contains(15));
        assert!(!w.contains(9));
    }

    #[test]
    fn test_window_hit_ratio() {
        let mut w = ReadaheadWindow::new(0, 10);
        w.record_hit();
        w.record_hit();
        w.record_miss();
        assert_eq!(w.hit_ratio_pct10(), 666);
    }

    #[test]
    fn test_window_advance() {
        let mut w = ReadaheadWindow::new(0, 8);
        w.advance(16, 12);
        assert_eq!(w.start_block, 17);
        assert_eq!(w.length, 12);
        assert_eq!(w.hit_count, 0);
    }

    #[test]
    fn test_access_log_sequential() {
        let mut log = BlockAccessLog::new();
        log.push(1);
        log.push(2);
        log.push(3);
        log.push(4);
        assert!(log.is_sequential(3));
    }

    #[test]
    fn test_access_log_not_sequential() {
        let mut log = BlockAccessLog::new();
        log.push(1);
        log.push(5);
        log.push(6);
        assert!(!log.is_sequential(3));
    }

    #[test]
    fn test_engine_hint_access_hit() {
        let mut eng = ReadaheadEngine::new(ReadaheadPolicy::Fixed);
        eng.adjust_window(0);
        eng.hint_access(5);
        assert_eq!(eng.stats().hits, 1);
    }

    #[test]
    fn test_engine_hint_access_miss() {
        let mut eng = ReadaheadEngine::new(ReadaheadPolicy::Fixed);
        eng.adjust_window(0);
        eng.hint_access(100);
        assert_eq!(eng.stats().misses, 1);
    }

    #[test]
    fn test_next_blocks_disabled() {
        let eng = ReadaheadEngine::new(ReadaheadPolicy::Disabled);
        assert!(eng.next_blocks_to_prefetch().is_empty());
    }

    #[test]
    fn test_next_blocks_fixed() {
        let mut eng = ReadaheadEngine::new(ReadaheadPolicy::Fixed);
        eng.hint_access(4);
        let blocks = eng.next_blocks_to_prefetch();
        assert_eq!(blocks.len(), 8);
        assert_eq!(blocks[0], 5);
    }

    #[test]
    fn test_policy_upgrade_to_adaptive() {
        let mut eng = ReadaheadEngine::new(ReadaheadPolicy::Fixed);
        eng.hint_access(1);
        eng.hint_access(2);
        eng.hint_access(3);
        eng.hint_access(4);
        assert_eq!(eng.policy(), ReadaheadPolicy::Adaptive);
    }

    #[test]
    fn test_stats_reset() {
        let mut eng = ReadaheadEngine::new(ReadaheadPolicy::Fixed);
        eng.hint_access(1);
        eng.reset_stats();
        assert_eq!(eng.stats().hits, 0);
    }

    #[test]
    fn test_hit_ratio_zero_div() {
        let stats = ReadaheadStats::new();
        assert_eq!(stats.hit_ratio_pct10(), 0);
    }
}

// ─── ReadaheadScheduler ───────────────────────────────────────────────────────

/// Planificateur multi-flux de read-ahead (plusieurs fenêtres en parallèle).
pub struct ReadaheadScheduler {
    engines: Vec<ReadaheadEngine>,
    max_streams: usize,
}

impl ReadaheadScheduler {
    /// Crée un planificateur avec `max_streams` flux (OOM-02).
    pub fn new(max_streams: usize, policy: ReadaheadPolicy) -> ExofsResult<Self> {
        if max_streams == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        let mut engines = Vec::new();
        engines
            .try_reserve(max_streams)
            .map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < max_streams {
            engines.push(ReadaheadEngine::new(policy));
            i = i.wrapping_add(1);
        }
        Ok(Self {
            engines,
            max_streams,
        })
    }

    /// Donne un accès au flux `stream_id` (RECUR-01 : pas de récursion).
    pub fn hint_access(&mut self, stream_id: usize, block: u64) -> ExofsResult<()> {
        if stream_id >= self.max_streams {
            return Err(ExofsError::InvalidArgument);
        }
        self.engines[stream_id].hint_access(block);
        Ok(())
    }

    /// Retourne les blocs à pré-charger pour le flux `stream_id`.
    pub fn next_blocks(&self, stream_id: usize) -> ExofsResult<Vec<u64>> {
        if stream_id >= self.max_streams {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(self.engines[stream_id].next_blocks_to_prefetch())
    }

    /// Ajuste la fenêtre du flux `stream_id`.
    pub fn adjust_window(&mut self, stream_id: usize, block: u64) -> ExofsResult<()> {
        if stream_id >= self.max_streams {
            return Err(ExofsError::InvalidArgument);
        }
        self.engines[stream_id].adjust_window(block);
        Ok(())
    }

    /// Statistiques agrégées de tous les flux (RECUR-01 : while).
    pub fn aggregate_stats(&self) -> ReadaheadStats {
        let mut s = ReadaheadStats::new();
        let mut i = 0usize;
        while i < self.engines.len() {
            let e = self.engines[i].stats();
            s.windows_created = s.windows_created.saturating_add(e.windows_created);
            s.blocks_prefetched = s.blocks_prefetched.saturating_add(e.blocks_prefetched);
            s.hits = s.hits.saturating_add(e.hits);
            s.misses = s.misses.saturating_add(e.misses);
            s.window_advances = s.window_advances.saturating_add(e.window_advances);
            s.policy_upgrades = s.policy_upgrades.saturating_add(e.policy_upgrades);
            i = i.wrapping_add(1);
        }
        s
    }

    pub fn stream_count(&self) -> usize {
        self.engines.len()
    }

    /// Réinitialise toutes les stats (RECUR-01 : while).
    pub fn reset_all_stats(&mut self) {
        let mut i = 0usize;
        while i < self.engines.len() {
            self.engines[i].reset_stats();
            i = i.wrapping_add(1);
        }
    }
}

#[cfg(test)]
mod tests_scheduler {
    use super::*;

    #[test]
    fn test_scheduler_new() {
        let s = ReadaheadScheduler::new(4, ReadaheadPolicy::Fixed).expect("ok");
        assert_eq!(s.stream_count(), 4);
    }

    #[test]
    fn test_scheduler_hint_invalid_stream() {
        let mut s = ReadaheadScheduler::new(2, ReadaheadPolicy::Fixed).expect("ok");
        assert!(s.hint_access(5, 10).is_err());
    }

    #[test]
    fn test_scheduler_next_blocks() {
        let mut s = ReadaheadScheduler::new(2, ReadaheadPolicy::Fixed).expect("ok");
        s.hint_access(0, 4).expect("ok");
        let blocks = s.next_blocks(0).expect("ok");
        assert!(!blocks.is_empty());
    }

    #[test]
    fn test_scheduler_aggregate_stats() {
        let mut s = ReadaheadScheduler::new(2, ReadaheadPolicy::Fixed).expect("ok");
        s.adjust_window(0, 0).expect("ok");
        s.hint_access(0, 1).expect("ok");
        s.hint_access(0, 2).expect("ok");
        let agg = s.aggregate_stats();
        // au moins une fenêtre créée
        assert_eq!(agg.windows_created, 1);
    }

    #[test]
    fn test_scheduler_reset_stats() {
        let mut s = ReadaheadScheduler::new(2, ReadaheadPolicy::Fixed).expect("ok");
        s.adjust_window(0, 0).expect("ok");
        s.reset_all_stats();
        let agg = s.aggregate_stats();
        assert_eq!(agg.windows_created, 0);
    }
}
