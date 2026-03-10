//! cache_stats.rs — Métriques atomiques du cache ExoFS (no_std).
//!
//! `CacheStats` : compteurs atomiques lock-free pour les opérations de cache.
//! `CacheStatsSnapshot` : copie cohérente pour inspection.
//! Règles : ONDISK-03 (pas d'AtomicU64 dans repr(C)), ARITH-02.


use core::sync::atomic::{AtomicU64, Ordering};

/// Instance globale des statistiques de cache.
pub static CACHE_STATS: CacheStats = CacheStats::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// CacheStats
// ─────────────────────────────────────────────────────────────────────────────

/// Compteurs atomiques des opérations de cache.
///
/// Tous les compteurs sont monotones (croissants) sauf `total_bytes` et
/// `dirty_bytes` qui peuvent décroître lors d'évictions / flushes.
pub struct CacheStats {
    /// Nombre total de hits (blob trouvé en cache).
    pub hits:          AtomicU64,
    /// Nombre total de misses (blob absent du cache).
    pub misses:        AtomicU64,
    /// Nombre total d'évictions.
    pub evictions:     AtomicU64,
    /// Nombre total d'insertions.
    pub insertions:    AtomicU64,
    /// Nombre total d'invalidations manuelles.
    pub invalidations: AtomicU64,
    /// Nombre d'opérations de warming (prélecture).
    pub warmings:      AtomicU64,
    /// Octets actuellement en cache (approximatif).
    pub total_bytes:   AtomicU64,
    /// Octets dirty en attente de writeback.
    pub dirty_bytes:   AtomicU64,
    /// Nombre de flushes writeback déclenchés.
    pub flushes:       AtomicU64,
    /// Nombre d'opérations de shrink.
    pub shrinks:       AtomicU64,
}

impl CacheStats {
    /// Constructeur `const` pour `static`.
    pub const fn new_const() -> Self {
        Self {
            hits:          AtomicU64::new(0),
            misses:        AtomicU64::new(0),
            evictions:     AtomicU64::new(0),
            insertions:    AtomicU64::new(0),
            invalidations: AtomicU64::new(0),
            warmings:      AtomicU64::new(0),
            total_bytes:   AtomicU64::new(0),
            dirty_bytes:   AtomicU64::new(0),
            flushes:       AtomicU64::new(0),
            shrinks:       AtomicU64::new(0),
        }
    }

    // ── Enregistrement ────────────────────────────────────────────────────────

    pub fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_eviction(&self, bytes: u64) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
        let cur = self.total_bytes.load(Ordering::Relaxed);
        self.total_bytes.store(cur.saturating_sub(bytes), Ordering::Relaxed);
    }

    pub fn record_insert(&self, bytes: u64) {
        self.insertions.fetch_add(1, Ordering::Relaxed);
        self.total_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn record_invalidation(&self, bytes: u64) {
        self.invalidations.fetch_add(1, Ordering::Relaxed);
        let cur = self.total_bytes.load(Ordering::Relaxed);
        self.total_bytes.store(cur.saturating_sub(bytes), Ordering::Relaxed);
    }

    pub fn record_warming(&self) {
        self.warmings.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_dirty_add(&self, bytes: u64) {
        self.dirty_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn record_dirty_flush(&self, bytes: u64) {
        self.flushes.fetch_add(1, Ordering::Relaxed);
        let cur = self.dirty_bytes.load(Ordering::Relaxed);
        self.dirty_bytes.store(cur.saturating_sub(bytes), Ordering::Relaxed);
    }

    pub fn record_shrink(&self) {
        self.shrinks.fetch_add(1, Ordering::Relaxed);
    }

    // ── Lecture ───────────────────────────────────────────────────────────────

    pub fn hits(&self)          -> u64 { self.hits.load(Ordering::Relaxed) }
    pub fn misses(&self)        -> u64 { self.misses.load(Ordering::Relaxed) }
    pub fn evictions(&self)     -> u64 { self.evictions.load(Ordering::Relaxed) }
    pub fn insertions(&self)    -> u64 { self.insertions.load(Ordering::Relaxed) }
    pub fn invalidations(&self) -> u64 { self.invalidations.load(Ordering::Relaxed) }
    pub fn warmings(&self)      -> u64 { self.warmings.load(Ordering::Relaxed) }
    pub fn total_bytes(&self)   -> u64 { self.total_bytes.load(Ordering::Relaxed) }
    pub fn dirty_bytes(&self)   -> u64 { self.dirty_bytes.load(Ordering::Relaxed) }
    pub fn flushes(&self)       -> u64 { self.flushes.load(Ordering::Relaxed) }
    pub fn shrinks(&self)       -> u64 { self.shrinks.load(Ordering::Relaxed) }

    /// Ratio de hit en pourcentage (0 si aucun accès).
    pub fn hit_ratio_pct(&self) -> u64 {
        let h = self.hits.load(Ordering::Relaxed);
        let m = self.misses.load(Ordering::Relaxed);
        let total = h.wrapping_add(m);
        if total == 0 { return 0; }
        h * 100 / total
    }

    /// `true` si le ratio de hit est supérieur à `min_pct`.
    pub fn is_effective(&self, min_pct: u64) -> bool {
        self.hit_ratio_pct() >= min_pct
    }

    /// Remet tous les compteurs à zéro.
    pub fn reset(&self) {
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.evictions.store(0, Ordering::Relaxed);
        self.insertions.store(0, Ordering::Relaxed);
        self.invalidations.store(0, Ordering::Relaxed);
        self.warmings.store(0, Ordering::Relaxed);
        self.total_bytes.store(0, Ordering::Relaxed);
        self.dirty_bytes.store(0, Ordering::Relaxed);
        self.flushes.store(0, Ordering::Relaxed);
        self.shrinks.store(0, Ordering::Relaxed);
    }

    /// Retourne un snapshot cohérent des compteurs.
    pub fn snapshot(&self) -> CacheStatsSnapshot {
        CacheStatsSnapshot {
            hits:          self.hits.load(Ordering::Relaxed),
            misses:        self.misses.load(Ordering::Relaxed),
            evictions:     self.evictions.load(Ordering::Relaxed),
            insertions:    self.insertions.load(Ordering::Relaxed),
            invalidations: self.invalidations.load(Ordering::Relaxed),
            warmings:      self.warmings.load(Ordering::Relaxed),
            total_bytes:   self.total_bytes.load(Ordering::Relaxed),
            dirty_bytes:   self.dirty_bytes.load(Ordering::Relaxed),
            flushes:       self.flushes.load(Ordering::Relaxed),
            shrinks:       self.shrinks.load(Ordering::Relaxed),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CacheStatsSnapshot
// ─────────────────────────────────────────────────────────────────────────────

/// Copie ponctuelle des métriques de cache.
#[derive(Clone, Copy, Debug, Default)]
pub struct CacheStatsSnapshot {
    pub hits:          u64,
    pub misses:        u64,
    pub evictions:     u64,
    pub insertions:    u64,
    pub invalidations: u64,
    pub warmings:      u64,
    pub total_bytes:   u64,
    pub dirty_bytes:   u64,
    pub flushes:       u64,
    pub shrinks:       u64,
}

impl CacheStatsSnapshot {
    pub fn total_accesses(&self) -> u64 {
        self.hits.wrapping_add(self.misses)
    }

    pub fn hit_ratio_pct(&self) -> u64 {
        let total = self.total_accesses();
        if total == 0 { return 0; }
        self.hits * 100 / total
    }

    /// `true` si aucune activité enregistrée.
    pub fn is_idle(&self) -> bool {
        self.total_accesses() == 0
    }

    /// Différence entre deux snapshots.
    pub fn delta(&self, prev: &CacheStatsSnapshot) -> CacheStatsDelta {
        CacheStatsDelta {
            d_hits:      self.hits.wrapping_sub(prev.hits),
            d_misses:    self.misses.wrapping_sub(prev.misses),
            d_evictions: self.evictions.wrapping_sub(prev.evictions),
            d_insertions: self.insertions.wrapping_sub(prev.insertions),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CacheStatsDelta
// ─────────────────────────────────────────────────────────────────────────────

/// Évolution des métriques entre deux snapshots.
#[derive(Clone, Copy, Debug, Default)]
pub struct CacheStatsDelta {
    pub d_hits:       u64,
    pub d_misses:     u64,
    pub d_evictions:  u64,
    pub d_insertions: u64,
}

impl CacheStatsDelta {
    /// `true` si les performances se sont dégradées (plus de misses que de hits).
    pub fn is_degraded(&self) -> bool { self.d_misses > self.d_hits }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn test_initial_zeros() {
        let s = CacheStats::new_const();
        assert_eq!(s.hits(), 0);
        assert_eq!(s.total_bytes(), 0);
    }

    #[test] fn test_record_hit_miss() {
        let s = CacheStats::new_const();
        s.record_hit(); s.record_hit(); s.record_miss();
        assert_eq!(s.hits(), 2);
        assert_eq!(s.misses(), 1);
    }

    #[test] fn test_hit_ratio_50pct() {
        let s = CacheStats::new_const();
        s.record_hit(); s.record_miss();
        assert_eq!(s.hit_ratio_pct(), 50);
    }

    #[test] fn test_hit_ratio_zero_initially() {
        let s = CacheStats::new_const();
        assert_eq!(s.hit_ratio_pct(), 0);
    }

    #[test] fn test_record_insert_updates_bytes() {
        let s = CacheStats::new_const();
        s.record_insert(1024);
        assert_eq!(s.total_bytes(), 1024);
    }

    #[test] fn test_record_eviction_decreases_bytes() {
        let s = CacheStats::new_const();
        s.record_insert(2048);
        s.record_eviction(1024);
        assert_eq!(s.total_bytes(), 1024);
    }

    #[test] fn test_eviction_no_underflow() {
        let s = CacheStats::new_const();
        s.record_eviction(9999);
        assert_eq!(s.total_bytes(), 0);
    }

    #[test] fn test_reset_clears_all() {
        let s = CacheStats::new_const();
        s.record_hit(); s.record_insert(512); s.record_eviction(256);
        s.reset();
        assert_eq!(s.hits(), 0);
        assert_eq!(s.total_bytes(), 0);
    }

    #[test] fn test_snapshot_coherent() {
        let s = CacheStats::new_const();
        s.record_hit(); s.record_miss(); s.record_insert(128);
        let snap = s.snapshot();
        assert_eq!(snap.hits, 1);
        assert_eq!(snap.misses, 1);
        assert_eq!(snap.total_bytes, 128);
    }

    #[test] fn test_snapshot_delta() {
        let s1 = CacheStatsSnapshot { hits: 10, misses: 5, ..Default::default() };
        let s2 = CacheStatsSnapshot { hits: 15, misses: 7, ..Default::default() };
        let d = s2.delta(&s1);
        assert_eq!(d.d_hits, 5);
        assert_eq!(d.d_misses, 2);
    }

    #[test] fn test_is_effective() {
        let s = CacheStats::new_const();
        for _ in 0..80 { s.record_hit(); }
        for _ in 0..20 { s.record_miss(); }
        assert!(s.is_effective(75));
        assert!(!s.is_effective(90));
    }

    #[test] fn test_dirty_tracking() {
        let s = CacheStats::new_const();
        s.record_dirty_add(512);
        assert_eq!(s.dirty_bytes(), 512);
        s.record_dirty_flush(512);
        assert_eq!(s.dirty_bytes(), 0);
    }
}

// ── Extensions CacheStats ──────────────────────────────────────────────────

impl CacheStats {
    /// Copie les compteurs les plus importants dans un tableau [hits, misses, evictions, bytes].
    pub fn to_array(&self) -> [u64; 4] {
        [
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
            self.evictions.load(Ordering::Relaxed),
            self.total_bytes.load(Ordering::Relaxed),
        ]
    }

    /// `true` si le cache n'a jamais servi aucune requête.
    pub fn is_cold(&self) -> bool {
        self.hits.load(Ordering::Relaxed) == 0
            && self.misses.load(Ordering::Relaxed) == 0
    }

    /// Met à jour `total_bytes` directement (pour synchronisation externe).
    pub fn set_total_bytes(&self, bytes: u64) {
        self.total_bytes.store(bytes, Ordering::Relaxed);
    }
}

impl CacheStatsSnapshot {
    /// `true` si le ratio de hit dépasse `min_pct`.
    pub fn is_effective(&self, min_pct: u64) -> bool {
        self.hit_ratio_pct() >= min_pct
    }

    /// Octets par accès (0 si aucun accès).
    pub fn bytes_per_access(&self) -> u64 {
        let total = self.total_accesses();
        if total == 0 { return 0; }
        self.total_bytes / total
    }
}

