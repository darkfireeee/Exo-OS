//! cache_pressure.rs — Surveillance de la pression mémoire ExoFS (no_std).
//!
//! `CachePressure` : détection multi-niveaux de la saturation du cache.
//! `CACHE_PRESSURE` : instance globale lock-free.
//! Règles : ARITH-02, ONDISK-03, RECUR-01.

use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// PressureLevel
// ─────────────────────────────────────────────────────────────────────────────

/// Niveau de pression mémoire.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum PressureLevel {
    /// Espace libre suffisant, aucune action requise.
    #[default]
    Low = 0,
    /// Premier seuil dépassé, surveiller.
    Medium = 1,
    /// Pression sérieuse, lancer une éviction.
    High = 2,
    /// Cache saturé, refus de nouvelles insertions possible.
    Critical = 3,
}

impl PressureLevel {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Low,
            1 => Self::Medium,
            2 => Self::High,
            _ => Self::Critical,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Critical => "Critical",
        }
    }

    /// `true` si des évictions sont nécessaires.
    pub fn needs_eviction(self) -> bool {
        self >= Self::High
    }

    /// `true` si les nouvelles insertions doivent être refusées.
    pub fn must_reject_inserts(self) -> bool {
        self == Self::Critical
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PressureThresholds
// ─────────────────────────────────────────────────────────────────────────────

/// Seuils de pression en pourcentage (0–100) d'utilisation.
#[derive(Clone, Copy, Debug)]
pub struct PressureThresholds {
    pub medium_pct: u8,
    pub high_pct: u8,
    pub critical_pct: u8,
}

impl PressureThresholds {
    pub const DEFAULT: Self = Self {
        medium_pct: 60,
        high_pct: 80,
        critical_pct: 95,
    };

    pub fn validate(&self) -> ExofsResult<()> {
        if self.medium_pct >= self.high_pct
            || self.high_pct >= self.critical_pct
            || self.critical_pct > 100
        {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }

    pub fn level_for(&self, used_pct: u8) -> PressureLevel {
        if used_pct >= self.critical_pct {
            PressureLevel::Critical
        } else if used_pct >= self.high_pct {
            PressureLevel::High
        } else if used_pct >= self.medium_pct {
            PressureLevel::Medium
        } else {
            PressureLevel::Low
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CachePressure
// ─────────────────────────────────────────────────────────────────────────────

/// Moniteur de pression mémoire du cache, lock-free.
pub struct CachePressure {
    used_bytes: AtomicU64,
    max_bytes: AtomicU64,
    level: AtomicU8,
    crits: AtomicU64,
    updates: AtomicU64,
}

pub static CACHE_PRESSURE: CachePressure = CachePressure::new_const();

impl CachePressure {
    pub const fn new_const() -> Self {
        Self {
            used_bytes: AtomicU64::new(0),
            max_bytes: AtomicU64::new(256 * 1024 * 1024),
            level: AtomicU8::new(0),
            crits: AtomicU64::new(0),
            updates: AtomicU64::new(0),
        }
    }

    pub fn set_max_bytes(&self, max: u64) {
        self.max_bytes.store(max, Ordering::Relaxed);
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.load(Ordering::Relaxed)
    }

    pub fn update(&self, used: u64, thresholds: PressureThresholds) -> PressureLevel {
        self.used_bytes.store(used, Ordering::Relaxed);
        let max = self.max_bytes.load(Ordering::Relaxed);
        let pct = if max == 0 {
            100u8
        } else {
            let p = used.saturating_mul(100) / max;
            if p > 100 {
                100u8
            } else {
                p as u8
            }
        };
        let new_level = thresholds.level_for(pct);
        let prev = self.level.swap(new_level as u8, Ordering::AcqRel);
        self.updates.fetch_add(1, Ordering::Relaxed);
        if new_level == PressureLevel::Critical
            && PressureLevel::from_u8(prev) != PressureLevel::Critical
        {
            self.crits.fetch_add(1, Ordering::Relaxed);
        }
        new_level
    }

    pub fn update_default(&self, used: u64) -> PressureLevel {
        self.update(used, PressureThresholds::DEFAULT)
    }

    pub fn level(&self) -> PressureLevel {
        PressureLevel::from_u8(self.level.load(Ordering::Relaxed))
    }

    pub fn used_bytes(&self) -> u64 {
        self.used_bytes.load(Ordering::Relaxed)
    }

    pub fn is_critical(&self) -> bool {
        self.level() == PressureLevel::Critical
    }
    pub fn needs_eviction(&self) -> bool {
        self.level().needs_eviction()
    }
    pub fn is_under_pressure(&self) -> bool {
        self.level().needs_eviction()
    }
    pub fn must_reject_inserts(&self) -> bool {
        self.level().must_reject_inserts()
    }

    pub fn reclaim_target(&self, target_pct: u8) -> u64 {
        let max = self.max_bytes.load(Ordering::Relaxed);
        let used = self.used_bytes.load(Ordering::Relaxed);
        let target = max.saturating_mul(target_pct as u64) / 100;
        if used <= target {
            0
        } else {
            used - target
        }
    }

    pub fn critical_transitions(&self) -> u64 {
        self.crits.load(Ordering::Relaxed)
    }
    pub fn update_count(&self) -> u64 {
        self.updates.load(Ordering::Relaxed)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PressureSnapshot
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct PressureSnapshot {
    pub level: PressureLevel,
    pub used_bytes: u64,
    pub max_bytes: u64,
    pub used_pct: u8,
}

impl PressureSnapshot {
    pub fn from(p: &CachePressure) -> Self {
        let used = p.used_bytes();
        let max = p.max_bytes();
        let pct = if max == 0 {
            100u8
        } else {
            let v = used.saturating_mul(100) / max;
            if v > 100 {
                100u8
            } else {
                v as u8
            }
        };
        Self {
            level: p.level(),
            used_bytes: used,
            max_bytes: max,
            used_pct: pct,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make() -> CachePressure {
        CachePressure::new_const()
    }

    #[test]
    fn test_initial_level_low() {
        let p = make();
        assert_eq!(p.level(), PressureLevel::Low);
    }

    #[test]
    fn test_update_reaches_critical() {
        let p = make();
        p.set_max_bytes(100);
        let lv = p.update_default(99);
        assert_eq!(lv, PressureLevel::Critical);
        assert!(p.is_critical());
    }

    #[test]
    fn test_update_low() {
        let p = make();
        p.set_max_bytes(1000);
        let lv = p.update_default(100);
        assert_eq!(lv, PressureLevel::Low);
    }

    #[test]
    fn test_update_medium() {
        let p = make();
        p.set_max_bytes(100);
        let lv = p.update_default(65);
        assert_eq!(lv, PressureLevel::Medium);
    }

    #[test]
    fn test_update_high() {
        let p = make();
        p.set_max_bytes(100);
        let lv = p.update_default(82);
        assert_eq!(lv, PressureLevel::High);
        assert!(p.needs_eviction());
    }

    #[test]
    fn test_reclaim_target_zero_when_ok() {
        let p = make();
        p.set_max_bytes(1000);
        p.update_default(100);
        assert_eq!(p.reclaim_target(80), 0);
    }

    #[test]
    fn test_reclaim_target_positive() {
        let p = make();
        p.set_max_bytes(100);
        p.update_default(90);
        assert_eq!(p.reclaim_target(80), 10);
    }

    #[test]
    fn test_thresholds_validate() {
        assert!(PressureThresholds::DEFAULT.validate().is_ok());
    }

    #[test]
    fn test_thresholds_invalid() {
        let t = PressureThresholds {
            medium_pct: 80,
            high_pct: 60,
            critical_pct: 95,
        };
        assert!(t.validate().is_err());
    }

    #[test]
    fn test_must_reject_inserts_only_critical() {
        assert!(!PressureLevel::High.must_reject_inserts());
        assert!(PressureLevel::Critical.must_reject_inserts());
    }

    #[test]
    fn test_critical_transitions_count() {
        let p = make();
        p.set_max_bytes(100);
        p.update_default(10);
        p.update_default(97);
        p.update_default(10);
        p.update_default(99);
        assert_eq!(p.critical_transitions(), 2);
    }

    #[test]
    fn test_pressure_snapshot() {
        let p = make();
        p.set_max_bytes(100);
        p.update_default(80);
        let snap = PressureSnapshot::from(&p);
        assert_eq!(snap.used_pct, 80);
    }
}

// ── Extensions CachePressure ──────────────────────────────────────────────

impl CachePressure {
    /// Calcule le % d'utilisation courant.
    pub fn used_pct(&self) -> u8 {
        let max = self.max_bytes.load(Ordering::Relaxed);
        let used = self.used_bytes.load(Ordering::Relaxed);
        if max == 0 {
            return 100u8;
        }
        let p = used.saturating_mul(100) / max;
        if p > 100 {
            100u8
        } else {
            p as u8
        }
    }

    /// `true` si la pression est au moins Medium.
    pub fn is_elevated(&self) -> bool {
        self.level() >= PressureLevel::Medium
    }

    /// Octets disponibles avant d'atteindre `high_watermark_pct`.
    pub fn headroom(&self, high_pct: u8) -> u64 {
        let max = self.max_bytes.load(Ordering::Relaxed);
        let used = self.used_bytes.load(Ordering::Relaxed);
        let hwm = max.saturating_mul(high_pct as u64) / 100;
        if used >= hwm {
            0
        } else {
            hwm - used
        }
    }
}
