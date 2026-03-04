//! health_check.rs — Vérification de santé du filesystem ExoFS (no_std).
//!
//! Fournit :
//!  - `HealthStatus`        : état de santé (enum 5 niveaux).
//!  - `HealthProbeId`       : identifiant de sonde.
//!  - `HealthProbeResult`   : résultat d'une sonde.
//!  - `HealthProbeRing`     : ring des résultats récents.
//!  - `HealthThresholds`    : seuils configurable.
//!  - `HealthCheck`         : évaluateur principal.
//!  - `HEALTH`              : singleton global.
//!
//! RECUR-01 : while uniquement.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_*.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─── HealthStatus ────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HealthStatus {
    Healthy   = 0,
    Degraded  = 1,   // Performances dégradées mais fonctionnel.
    Warning   = 2,   // Seuils dépassés, attention requise.
    Critical  = 3,   // Espace faible / erreurs fréquentes.
    ReadOnly  = 4,   // Monté en lecture seule.
    Unmounted = 5,
}

impl HealthStatus {
    pub fn name(self) -> &'static str {
        match self {
            Self::Healthy   => "healthy",
            Self::Degraded  => "degraded",
            Self::Warning   => "warning",
            Self::Critical  => "critical",
            Self::ReadOnly  => "read_only",
            Self::Unmounted => "unmounted",
        }
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Degraded,
            2 => Self::Warning,
            3 => Self::Critical,
            4 => Self::ReadOnly,
            5 => Self::Unmounted,
            _ => Self::Healthy,
        }
    }

    pub fn is_writable(self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded | Self::Warning)
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::ReadOnly | Self::Unmounted)
    }

    /// Retourne le pire des deux états.
    pub fn worst(self, other: Self) -> Self {
        if other > self { other } else { self }
    }
}

// ─── HealthProbeId ────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthProbeId {
    SpaceUsage      = 0,
    ErrorRate       = 1,
    CacheEfficiency = 2,
    GcPressure      = 3,
    LatencyP99      = 4,
    WritePending    = 5,
    EpochHealth     = 6,
    MetadataIntegrity = 7,
}

impl HealthProbeId {
    pub const COUNT: usize = 8;

    pub fn name(self) -> &'static str {
        match self {
            Self::SpaceUsage        => "space_usage",
            Self::ErrorRate         => "error_rate",
            Self::CacheEfficiency   => "cache_efficiency",
            Self::GcPressure        => "gc_pressure",
            Self::LatencyP99        => "latency_p99",
            Self::WritePending      => "write_pending",
            Self::EpochHealth       => "epoch_health",
            Self::MetadataIntegrity => "metadata_integrity",
        }
    }
}

// ─── HealthProbeResult ───────────────────────────────────────────────────────

/// Résultat d'une sonde de santé.
#[derive(Clone, Copy, Debug)]
pub struct HealthProbeResult {
    pub probe:   HealthProbeId,
    pub status:  HealthStatus,
    pub value:   u64,   // valeur mesurée (unité dépend de la sonde)
    pub tick:    u64,
}

impl HealthProbeResult {
    pub fn ok(probe: HealthProbeId, value: u64, tick: u64) -> Self {
        Self { probe, status: HealthStatus::Healthy, value, tick }
    }

    pub fn warn(probe: HealthProbeId, value: u64, tick: u64) -> Self {
        Self { probe, status: HealthStatus::Warning, value, tick }
    }

    pub fn critical(probe: HealthProbeId, value: u64, tick: u64) -> Self {
        Self { probe, status: HealthStatus::Critical, value, tick }
    }
}

// ─── HealthProbeRing ─────────────────────────────────────────────────────────

pub const PROBE_RING_SIZE: usize = 64;

/// Ring des résultats de sondes récents.
pub struct HealthProbeRing {
    slots: [UnsafeCell<HealthProbeResult>; PROBE_RING_SIZE],
    head:  AtomicU64,
}

// SAFETY : accès par index atomique tournant.
unsafe impl Sync for HealthProbeRing {}
unsafe impl Send for HealthProbeRing {}

impl HealthProbeRing {
    const ZERO: HealthProbeResult = HealthProbeResult {
        probe:  HealthProbeId::SpaceUsage,
        status: HealthStatus::Healthy,
        value:  0, tick: 0,
    };

    pub const fn new_const() -> Self {
        const Z: UnsafeCell<HealthProbeResult> = UnsafeCell::new(HealthProbeResult {
            probe:  HealthProbeId::SpaceUsage,
            status: HealthStatus::Healthy,
            value:  0,
            tick:   0,
        });
        Self { slots: [Z; PROBE_RING_SIZE], head: AtomicU64::new(0) }
    }

    pub fn push(&self, r: HealthProbeResult) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) as usize % PROBE_RING_SIZE;
        // SAFETY : index atomique tournant.
        unsafe { *self.slots[idx].get() = r; }
    }

    pub fn latest(&self) -> HealthProbeResult {
        let head = self.head.load(Ordering::Relaxed) as usize;
        let idx  = (head.wrapping_add(PROBE_RING_SIZE).wrapping_sub(1)) % PROBE_RING_SIZE;
        unsafe { *self.slots[idx].get() }
    }

    /// Collecte les n derniers résultats filtrés par probe (OOM-02 / RECUR-01).
    pub fn last_n_for_probe(&self, probe: HealthProbeId, n: usize, out: &mut Vec<HealthProbeResult>) -> ExofsResult<()> {
        let cap = n.min(PROBE_RING_SIZE);
        out.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;
        let head = self.head.load(Ordering::Relaxed) as usize;
        let mut found = 0usize;
        let mut i = 0usize;
        while i < PROBE_RING_SIZE && found < cap {
            let idx = (head.wrapping_add(PROBE_RING_SIZE).wrapping_sub(i).wrapping_sub(1)) % PROBE_RING_SIZE;
            let r = unsafe { *self.slots[idx].get() };
            if r.tick > 0 && r.probe == probe {
                out.push(r);
                found = found.wrapping_add(1);
            }
            i = i.wrapping_add(1);
        }
        Ok(())
    }
}

// ─── HealthThresholds ────────────────────────────────────────────────────────

/// Seuils configurables par sonde.
#[derive(Clone, Copy, Debug)]
pub struct HealthThresholds {
    pub space_warn_pct:    u8,   // % utilisation → Warning
    pub space_crit_pct:    u8,   // % utilisation → Critical
    pub error_warn_pct10:  u64,  // taux d'erreur * 1000 → Warning
    pub error_crit_pct10:  u64,  // taux d'erreur * 1000 → Critical
    pub latency_warn_us:   u64,
    pub latency_crit_us:   u64,
}

impl HealthThresholds {
    pub fn default_thresholds() -> Self {
        Self {
            space_warn_pct:   80,
            space_crit_pct:   95,
            error_warn_pct10: 10,    // 1%
            error_crit_pct10: 50,    // 5%
            latency_warn_us:  10_000,
            latency_crit_us:  100_000,
        }
    }

    pub fn validate(&self) -> ExofsResult<()> {
        if self.space_warn_pct >= self.space_crit_pct { return Err(ExofsError::InvalidArgument); }
        if self.error_warn_pct10 >= self.error_crit_pct10 { return Err(ExofsError::InvalidArgument); }
        Ok(())
    }
}

// ─── HealthCheck ─────────────────────────────────────────────────────────────

/// Évaluateur de santé du filesystem.
pub struct HealthCheck {
    status:     AtomicU8,
    thresholds: UnsafeCell<HealthThresholds>,
    probe_ring: HealthProbeRing,
    eval_tick:  AtomicU64,
}

unsafe impl Sync for HealthCheck {}
unsafe impl Send for HealthCheck {}

impl HealthCheck {
    pub const fn new_const() -> Self {
        const DEFAULT_THR: HealthThresholds = HealthThresholds {
            space_warn_pct:   80,
            space_crit_pct:   95,
            error_warn_pct10: 10,
            error_crit_pct10: 50,
            latency_warn_us:  10_000,
            latency_crit_us:  100_000,
        };
        Self {
            status:     AtomicU8::new(HealthStatus::Healthy as u8),
            thresholds: UnsafeCell::new(DEFAULT_THR),
            probe_ring: HealthProbeRing::new_const(),
            eval_tick:  AtomicU64::new(0),
        }
    }

    pub fn status(&self) -> HealthStatus {
        HealthStatus::from_u8(self.status.load(Ordering::Acquire))
    }

    pub fn set_status(&self, s: HealthStatus) {
        // Ne jamais rétrograder ReadOnly/Unmounted automatiquement.
        let cur = self.status();
        if !cur.is_terminal() {
            self.status.store(s as u8, Ordering::Release);
        }
    }

    pub fn force_status(&self, s: HealthStatus) {
        self.status.store(s as u8, Ordering::Release);
    }

    pub fn is_writable(&self) -> bool { self.status().is_writable() }

    /// Met à jour les seuils (SAFETY : appelé sans concurrent dans init).
    pub fn set_thresholds(&self, t: HealthThresholds) {
        unsafe { *self.thresholds.get() = t; }
    }

    fn thresholds(&self) -> &HealthThresholds {
        // SAFETY : lecture seule après init.
        unsafe { &*self.thresholds.get() }
    }

    /// Évalue l'état d'espace (entreable avec des valeurs externes).
    pub fn probe_space(&self, usage_pct: u8, tick: u64) -> HealthProbeResult {
        let thr = self.thresholds();
        let status = if usage_pct >= thr.space_crit_pct {
            HealthStatus::Critical
        } else if usage_pct >= thr.space_warn_pct {
            HealthStatus::Warning
        } else {
            HealthStatus::Healthy
        };
        let r = HealthProbeResult { probe: HealthProbeId::SpaceUsage, status, value: usage_pct as u64, tick };
        self.probe_ring.push(r);
        r
    }

    /// Évalue le taux d'erreur.
    pub fn probe_error_rate(&self, errors: u64, total_ops: u64, tick: u64) -> HealthProbeResult {
        let thr = self.thresholds();
        let rate = errors.saturating_mul(1000).checked_div(total_ops.max(1)).unwrap_or(0);
        let status = if rate >= thr.error_crit_pct10 {
            HealthStatus::Critical
        } else if rate >= thr.error_warn_pct10 {
            HealthStatus::Warning
        } else {
            HealthStatus::Healthy
        };
        let r = HealthProbeResult { probe: HealthProbeId::ErrorRate, status, value: rate, tick };
        self.probe_ring.push(r);
        r
    }

    /// Évalue la latence P99 en µs.
    pub fn probe_latency(&self, p99_us: u64, tick: u64) -> HealthProbeResult {
        let thr = self.thresholds();
        let status = if p99_us >= thr.latency_crit_us {
            HealthStatus::Critical
        } else if p99_us >= thr.latency_warn_us {
            HealthStatus::Warning
        } else {
            HealthStatus::Healthy
        };
        let r = HealthProbeResult { probe: HealthProbeId::LatencyP99, status, value: p99_us, tick };
        self.probe_ring.push(r);
        r
    }

    /// Évaluation globale : combine plusieurs sondes → pire état.
    pub fn evaluate(&self, usage_pct: u8, errors: u64, total_ops: u64, p99_us: u64) -> HealthStatus {
        let tick = self.eval_tick.fetch_add(1, Ordering::Relaxed);
        let s1 = self.probe_space(usage_pct, tick);
        let s2 = self.probe_error_rate(errors, total_ops, tick);
        let s3 = self.probe_latency(p99_us, tick);
        let worst = s1.status.worst(s2.status).worst(s3.status);
        self.set_status(worst);
        worst
    }

    pub fn probe_ring(&self) -> &HealthProbeRing { &self.probe_ring }
}

pub static HEALTH: HealthCheck = HealthCheck::new_const();

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_status() {
        let h = HealthCheck::new_const();
        assert_eq!(h.status(), HealthStatus::Healthy);
        assert!(h.is_writable());
    }

    #[test]
    fn test_set_status() {
        let h = HealthCheck::new_const();
        h.set_status(HealthStatus::Degraded);
        assert_eq!(h.status(), HealthStatus::Degraded);
    }

    #[test]
    fn test_status_readonly_not_downgraded() {
        let h = HealthCheck::new_const();
        h.force_status(HealthStatus::ReadOnly);
        h.set_status(HealthStatus::Healthy); // ne doit pas changer
        assert_eq!(h.status(), HealthStatus::ReadOnly);
    }

    #[test]
    fn test_probe_space_critical() {
        let h = HealthCheck::new_const();
        let r = h.probe_space(96, 1);
        assert_eq!(r.status, HealthStatus::Critical);
    }

    #[test]
    fn test_probe_space_warning() {
        let h = HealthCheck::new_const();
        let r = h.probe_space(85, 1);
        assert_eq!(r.status, HealthStatus::Warning);
    }

    #[test]
    fn test_probe_space_healthy() {
        let h = HealthCheck::new_const();
        let r = h.probe_space(50, 1);
        assert_eq!(r.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_probe_error_rate() {
        let h = HealthCheck::new_const();
        // 5/100 * 1000 = 50 → critical (>= 50)
        let r = h.probe_error_rate(5, 100, 1);
        assert_eq!(r.status, HealthStatus::Critical);
    }

    #[test]
    fn test_probe_latency_warning() {
        let h = HealthCheck::new_const();
        let r = h.probe_latency(15_000, 1);
        assert_eq!(r.status, HealthStatus::Warning);
    }

    #[test]
    fn test_evaluate_sets_status() {
        let h = HealthCheck::new_const();
        let s = h.evaluate(96, 0, 100, 1000);
        assert_eq!(s, HealthStatus::Critical);
        assert_eq!(h.status(), HealthStatus::Critical);
    }

    #[test]
    fn test_health_status_worst() {
        assert_eq!(HealthStatus::Healthy.worst(HealthStatus::Critical), HealthStatus::Critical);
        assert_eq!(HealthStatus::Warning.worst(HealthStatus::Degraded), HealthStatus::Warning);
    }

    #[test]
    fn test_health_status_is_writable() {
        assert!(HealthStatus::Healthy.is_writable());
        assert!(HealthStatus::Degraded.is_writable());
        assert!(!HealthStatus::ReadOnly.is_writable());
    }

    #[test]
    fn test_thresholds_validate() {
        let mut t = HealthThresholds::default_thresholds();
        assert!(t.validate().is_ok());
        t.space_warn_pct = 96;
        t.space_crit_pct = 95;
        assert!(t.validate().is_err());
    }

    #[test]
    fn test_probe_ring_last_n() {
        let h = HealthCheck::new_const();
        h.probe_space(50, 1);
        h.probe_space(85, 2);
        let mut out = Vec::new();
        h.probe_ring().last_n_for_probe(HealthProbeId::SpaceUsage, 10, &mut out).expect("ok");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn test_status_name() {
        assert_eq!(HealthStatus::Critical.name(), "critical");
        assert_eq!(HealthStatus::Healthy.name(), "healthy");
    }
}
