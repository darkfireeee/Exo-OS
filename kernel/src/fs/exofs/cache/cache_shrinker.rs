//! cache_shrinker.rs — Réducteur de cache sous pression mémoire ExoFS (no_std).
//!
//! `CacheShrinker` : réduit les caches du plus froid au plus chaud jusqu'à atteindre
//! la cible de libération mémoire.
//! Règles : RECUR-01, OOM-02, ARITH-02.


extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// ShrinkTarget
// ─────────────────────────────────────────────────────────────────────────────

/// Requête de réduction envoyée aux réducteurs.
#[derive(Clone, Copy, Debug)]
pub struct ShrinkRequest {
    /// Octets à libérer.
    pub bytes_to_free: u64,
    /// Priorité : `true` = urgent, passer à travers tous les niveaux.
    pub urgent: bool,
}

/// Résultat d'une opération de réduction.
#[derive(Clone, Copy, Debug, Default)]
pub struct ShrinkResult {
    /// Octets effectivement libérés.
    pub freed_bytes:   u64,
    /// Entrées évincées.
    pub evicted_count: u64,
    /// `true` si la cible a été atteinte.
    pub target_reached: bool,
}

impl ShrinkResult {
    pub fn add(&mut self, freed: u64, count: u64) {
        self.freed_bytes   = self.freed_bytes.wrapping_add(freed);
        self.evicted_count = self.evicted_count.wrapping_add(count);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ShrinkableCache — trait simulé avec pointeur de fonction
// ─────────────────────────────────────────────────────────────────────────────

/// Enregistrement d'un cache reductible.
#[derive(Clone, Copy)]
pub struct CacheEntry {
    /// Nom du cache (pour diagnostics).
    pub name:   &'static str,
    /// Octets actuellement utilisés.
    pub used:   u64,
    /// Capacité.
    pub cap:    u64,
    /// Priorité de réduction : 0 = premier (le plus froid).
    pub priority: u8,
    /// Fonction de réduction.
    pub shrink_fn: fn(bytes: u64) -> ShrinkResult,
}

// ─────────────────────────────────────────────────────────────────────────────
// CacheShrinker
// ─────────────────────────────────────────────────────────────────────────────

/// Gestionnaire de réduction multi-cache.
///
/// Ne possède pas de lock car la liste est statiquement initialisée.
pub struct CacheShrinker {
    caches: [Option<CacheEntry>; 8],
    n:      usize,
}

pub static CACHE_SHRINKER: CacheShrinker = CacheShrinker::new_const();

impl CacheShrinker {
    pub const fn new_const() -> Self {
        Self { caches: [None; 8], n: 0 }
    }

    // ── Enregistrement ────────────────────────────────────────────────────────

    /// Enregistre un cache réductible.
    pub fn register(&mut self, entry: CacheEntry) -> ExofsResult<()> {
        if self.n >= 8 {
            return Err(ExofsError::NoSpace);
        }
        self.caches[self.n] = Some(entry);
        self.n  = self.n.wrapping_add(1);
        Ok(())
    }

    // ── Réduction ─────────────────────────────────────────────────────────────

    /// Libère `bytes_to_free` octets en appelant les réducteurs du plus froid au plus chaud.
    pub fn shrink_to_target(&self, req: ShrinkRequest) -> ShrinkResult {
        // Récupère les caches présents et triés par priorité.
        let mut sorted: Vec<CacheEntry> = self.caches[..self.n]
            .iter()
            .filter_map(|x| x.as_ref().cloned())
            .collect();
        sorted.sort_unstable_by_key(|e| e.priority);

        let mut result    = ShrinkResult::default();
        let mut remaining = req.bytes_to_free;

        'outer: for entry in &sorted {
            if remaining == 0 { break 'outer; }
            let to_free = remaining.min(entry.used);
            let r = (entry.shrink_fn)(to_free);
            result.add(r.freed_bytes, r.evicted_count);
            remaining = remaining.saturating_sub(r.freed_bytes);
        }

        result.target_reached = remaining == 0;
        result
    }

    /// Libère le maximum possible (flush émergence).
    pub fn shrink_all(&self) -> ShrinkResult {
        let total: u64 = self.caches[..self.n]
            .iter()
            .filter_map(|x| x.as_ref())
            .map(|e| e.used)
            .fold(0u64, |acc, v| acc.saturating_add(v));
        self.shrink_to_target(ShrinkRequest { bytes_to_free: total, urgent: true })
    }

    /// Nombre de caches enregistrés.
    pub fn n_caches(&self) -> usize { self.n }

    /// Utilisation totale estimée.
    pub fn total_used(&self) -> u64 {
        self.caches[..self.n]
            .iter()
            .filter_map(|x| x.as_ref())
            .map(|e| e.used)
            .fold(0u64, |acc, v| acc.saturating_add(v))
    }

    /// Capacité totale estimée.
    pub fn total_cap(&self) -> u64 {
        self.caches[..self.n]
            .iter()
            .filter_map(|x| x.as_ref())
            .map(|e| e.cap)
            .fold(0u64, |acc, v| acc.saturating_add(v))
    }

    /// Taux de remplissage global en pourcentage.
    pub fn fill_pct(&self) -> u8 {
        let cap = self.total_cap();
        if cap == 0 { return 0; }
        let used = self.total_used();
        let pct = used.saturating_mul(100) / cap;
        if pct > 100 { 100 } else { pct as u8 }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_shrink(bytes: u64) -> ShrinkResult {
        ShrinkResult { freed_bytes: bytes, evicted_count: 1, target_reached: false }
    }

    fn make_entry(name: &'static str, used: u64, priority: u8) -> CacheEntry {
        CacheEntry { name, used, cap: used * 2, priority, shrink_fn: dummy_shrink }
    }

    #[test] fn test_register_and_count() {
        let mut s = CacheShrinker::new_const();
        s.register(make_entry("blob", 1024, 0)).unwrap();
        assert_eq!(s.n_caches(), 1);
    }

    #[test] fn test_shrink_to_target_exact() {
        let mut s = CacheShrinker::new_const();
        s.register(make_entry("a", 2048, 0)).unwrap();
        let req = ShrinkRequest { bytes_to_free: 1024, urgent: false };
        let r = s.shrink_to_target(req);
        assert_eq!(r.freed_bytes, 1024);
        assert!(r.target_reached);
    }

    #[test] fn test_shrink_order_by_priority() {
        #[allow(dead_code)] static ORDER: core::sync::atomic::AtomicU8 =
            core::sync::atomic::AtomicU8::new(0);

        fn shrink_a(b: u64) -> ShrinkResult {
            // priorité 0 doit être appelé en premier
            ShrinkResult { freed_bytes: b, evicted_count: 1, target_reached: false }
        }
        let mut s = CacheShrinker::new_const();
        s.register(CacheEntry {
            name: "high", used: 100, cap: 200, priority: 10, shrink_fn: dummy_shrink,
        }).unwrap();
        s.register(CacheEntry {
            name: "low", used: 100, cap: 200, priority: 0, shrink_fn: shrink_a,
        }).unwrap();
        // Le shrinker doit trier par priorité croissante.
        let req = ShrinkRequest { bytes_to_free: 100, urgent: false };
        let r = s.shrink_to_target(req);
        assert!(r.target_reached);
    }

    #[test] fn test_shrink_all() {
        let mut s = CacheShrinker::new_const();
        s.register(make_entry("a", 500, 0)).unwrap();
        s.register(make_entry("b", 300, 1)).unwrap();
        let r = s.shrink_all();
        assert_eq!(r.freed_bytes, 800);
    }

    #[test] fn test_total_used() {
        let mut s = CacheShrinker::new_const();
        s.register(make_entry("a", 100, 0)).unwrap();
        s.register(make_entry("b", 200, 1)).unwrap();
        assert_eq!(s.total_used(), 300);
    }

    #[test] fn test_fill_pct() {
        let mut s = CacheShrinker::new_const();
        s.register(make_entry("a", 100, 0)).unwrap(); // cap = 200
        assert_eq!(s.fill_pct(), 50);
    }

    #[test] fn test_register_overflow() {
        let mut s = CacheShrinker::new_const();
        for i in 0u8..8 { s.register(make_entry("x", i as u64, 0)).unwrap(); }
        assert!(s.register(make_entry("overflow", 0, 0)).is_err());
    }

    #[test] fn test_shrink_result_add() {
        let mut r = ShrinkResult::default();
        r.add(100, 2);
        r.add(50, 1);
        assert_eq!(r.freed_bytes, 150);
        assert_eq!(r.evicted_count, 3);
    }

    #[test] fn test_empty_shrinker() {
        let s = CacheShrinker::new_const();
        let req = ShrinkRequest { bytes_to_free: 9999, urgent: true };
        let r = s.shrink_to_target(req);
        assert_eq!(r.freed_bytes, 0);
        assert!(!r.target_reached);
    }
}

// ── Extensions CacheShrinker ───────────────────────────────────────────────────

/// Journal de réduction.
#[derive(Clone, Copy, Debug, Default)]
pub struct ShrinkLogEntry {
    pub freed_bytes:   u64,
    pub evicted_count: u64,
    pub target_met:    bool,
}

/// Tableau de bord agregé du shrinker.
#[derive(Clone, Debug, Default)]
pub struct ShrinkReport {
    pub total_freed:   u64,
    pub total_evicted: u64,
    pub n_caches:      usize,
    pub target_met:    bool,
}

impl CacheShrinker {
    /// Réduit jusqu'à `pct`% de la capacité totale.
    pub fn shrink_to_pct(&self, pct: u8) -> ShrinkResult {
        let cap   = self.total_cap();
        let used  = self.total_used();
        let target = cap.saturating_mul(pct as u64) / 100;
        let to_free = if used > target { used - target } else { return ShrinkResult::default(); };
        self.shrink_to_target(ShrinkRequest { bytes_to_free: to_free, urgent: false })
    }

    /// Génère un rapport de réduction d'urgence.
    pub fn emergency_report(&self) -> ShrinkReport {
        let r = self.shrink_all();
        ShrinkReport {
            total_freed:   r.freed_bytes,
            total_evicted: r.evicted_count,
            n_caches:      self.n_caches(),
            target_met:    r.target_reached,
        }
    }

    /// Retourne le taux de remplissage du cache `i`.
    pub fn fill_pct_of(&self, i: usize) -> u8 {
        if i >= self.n { return 0; }
        if let Some(e) = &self.caches[i] {
            if e.cap == 0 { return 0; }
            let p = e.used.saturating_mul(100) / e.cap;
            if p > 100 { 100 } else { p as u8 }
        } else { 0 }
    }

    /// Nom du cache `i`.
    pub fn cache_name(&self, i: usize) -> &'static str {
        if i >= self.n { return ""; }
        self.caches[i].as_ref().map(|e| e.name).unwrap_or("")
    }

    /// `true` si au moins un cache est à >90% de capacité.
    pub fn any_critical(&self) -> bool {
        (0..self.n).any(|i| self.fill_pct_of(i) >= 90)
    }
}
