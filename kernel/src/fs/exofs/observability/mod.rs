// SPDX-License-Identifier: MIT
// ExoFS Observability — mod.rs
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

//! Module d'observabilité ExoFS.
//!
//! Regroupe métriques, alertes, santé, latences, débits,
//! compteurs de perf, espace disque, tracing et interface de debug.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─── Sous-modules ─────────────────────────────────────────────────────────────

pub mod metrics;
pub mod alert;
pub mod health_check;
pub mod latency_histogram;
pub mod perf_counters;
pub mod space_tracker;
pub mod throughput_tracker;
pub mod tracing;
pub mod debug_interface;

// ─── Re-exports principaux ───────────────────────────────────────────────────

// metrics
pub use metrics::{
    MetricId, MetricKind, ExofsMetrics, MetricsSnapshot, MetricsDiff,
    MetricsHistory, EXOFS_METRICS, METRICS_HISTORY,
};

// alert
pub use alert::{
    AlertLevel, AlertCode, Alert, AlertLog, AlertFilter, AlertManager,
    ALERT_LOG,
};

// health_check
pub use health_check::{
    HealthStatus, HealthProbeId, HealthProbeResult, HealthProbeRing,
    HealthThresholds, HealthCheck, HEALTH,
};

// latency_histogram
pub use latency_histogram::{
    LatencyHistogram, LatencySummary, LatencyTracker, LatencyCategory,
    LatencyWindow, LATENCY_HIST, LATENCY_TRACKER,
};

// perf_counters
pub use perf_counters::{
    PerfCounterId, PerfCounterSet, PerfSnapshot, PerfDelta,
    PerfRateWindow, PerfReport, PERF_COUNTERS, PERF_RATE,
};

// space_tracker
pub use space_tracker::{
    SpaceZone, SpaceZoneStats, SpaceTracker, SpaceSnapshot,
    SpaceQuota, FragmentationInfo, SpaceHistory, SPACE_TRACKER, SPACE_HISTORY,
};

// throughput_tracker
pub use throughput_tracker::{
    ThroughputSample, ThroughputWindow, ThroughputTracker, ThroughputSnapshot,
    ThroughputRate, ThroughputThresholds, THROUGHPUT_TRACKER,
};

// tracing
pub use tracing::{
    TraceLevel, ComponentId, TraceEvent, TraceFilter, TraceRing,
    TraceSession, TraceSummary, TRACE_RING,
};

// debug_interface
pub use debug_interface::{
    DebugCommandId, DebugCommand, DebugResponseStatus, DebugResponse,
    DebugQueue, DebugSession, DebugStats, DEBUG_QUEUE,
};

// ─── ObservabilityConfig ─────────────────────────────────────────────────────

/// Configuration globale du module d'observabilité.
#[derive(Clone, Copy, Debug)]
pub struct ObservabilityConfig {
    /// Active la collecte de métriques.
    pub metrics_enabled:     bool,
    /// Active les alertes.
    pub alerts_enabled:      bool,
    /// Niveau de trace minimum.
    pub min_trace_level:     TraceLevel,
    /// Période d'évaluation de santé en µs.
    pub health_eval_period_us: u64,
    /// Taille de fenêtre pour le calcul de débit (nombre de périodes).
    pub throughput_window:   u64,
    /// Activer le debug via la queue.
    pub debug_enabled:       bool,
}

impl ObservabilityConfig {
    pub const fn default_config() -> Self {
        Self {
            metrics_enabled:      true,
            alerts_enabled:       true,
            min_trace_level:      TraceLevel::Info,
            health_eval_period_us: 1_000_000, // 1s
            throughput_window:    16,
            debug_enabled:        true,
        }
    }

    /// Valide la configuration (ARITH-02 : valeurs bornées).
    pub fn validate(&self) -> ExofsResult<()> {
        if self.health_eval_period_us == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        if self.throughput_window == 0 || self.throughput_window > 64 {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }

    pub fn is_verbose(&self) -> bool {
        self.min_trace_level >= TraceLevel::Debug
    }
}

// ─── ObservabilityStatus ─────────────────────────────────────────────────────

/// État agrégé du module d'observabilité.
#[derive(Clone, Copy, Debug)]
pub struct ObservabilityStatus {
    pub health:        HealthStatus,
    pub has_critical_alert: bool,
    pub error_rate_ppt:     u64,
    pub space_usage_pct:    u64,
    pub throughput_bpt:     u64,
    pub trace_dropped:      u64,
}

impl ObservabilityStatus {
    pub fn is_nominal(&self) -> bool {
        matches!(self.health, HealthStatus::Healthy | HealthStatus::Degraded)
            && !self.has_critical_alert
            && self.error_rate_ppt < 50
    }

    pub fn needs_attention(&self) -> bool {
        !self.is_nominal()
    }
}

// ─── ObservabilityModule ─────────────────────────────────────────────────────

/// Facade principale du module d'observabilité.
pub struct ObservabilityModule {
    config:      ObservabilityConfig,
    init_tick:   AtomicU64,
    event_count: AtomicU64,
}

impl ObservabilityModule {
    pub const fn new_const() -> Self {
        Self {
            config:      ObservabilityConfig::default_config(),
            init_tick:   AtomicU64::new(0),
            event_count: AtomicU64::new(0),
        }
    }

    /// Initialise tous les sous-modules avec la configuration donnée.
    pub fn init(&self, config: ObservabilityConfig, tick: u64) -> ExofsResult<()> {
        config.validate()?;
        TRACE_RING.set_min_level(config.min_trace_level);
        self.init_tick.store(tick, Ordering::Relaxed);
        TRACE_RING.emit(tick, ComponentId::OBSERVER, TraceLevel::Info, "observability init ok");
        Ok(())
    }

    /// Enregistre un événement interne (metrics + compteur).
    pub fn record_event(&self) {
        self.event_count.fetch_add(1, Ordering::Relaxed);
        EXOFS_METRICS.inc_read(1); // exemple d'instrumentation
    }

    /// Retourne le statut agrégé courant.
    pub fn status(&self) -> ObservabilityStatus {
        ObservabilityStatus {
            health:             HEALTH.status(),
            has_critical_alert: ALERT_LOG.has_critical(),
            error_rate_ppt:     EXOFS_METRICS.error_rate_pct10().saturating_mul(100)
                                    .checked_div(10).unwrap_or(0),
            space_usage_pct:    SPACE_TRACKER.usage_pct() as u64,
            throughput_bpt:     THROUGHPUT_TRACKER.avg_total_bpt(),
            trace_dropped:      TRACE_RING.dropped(),
        }
    }

    /// Produit un snapshot agrégeant toutes les métriques.
    pub fn full_snapshot(&self) -> ExofsResult<ObservabilitySnapshot> {
        let metrics = EXOFS_METRICS.snapshot();
        let space   = SPACE_TRACKER.snapshot();
        let perf    = PERF_COUNTERS.snapshot();
        let thru    = THROUGHPUT_TRACKER.snapshot()?;
        Ok(ObservabilitySnapshot {
            status:    self.status(),
            metrics,
            space,
            perf,
            throughput: thru,
            event_count: self.event_count.load(Ordering::Relaxed),
            uptime_ticks: self.uptime(0),
        })
    }

    /// Temps écoulé depuis l'init en ticks.
    pub fn uptime(&self, current_tick: u64) -> u64 {
        current_tick.saturating_sub(self.init_tick.load(Ordering::Relaxed))
    }

    /// Émet une alerte critique via les sous-modules.
    pub fn emit_critical(&self, tick: u64, msg: &str) {
        ALERT_LOG.critical(alert::AlertCode::new(0x0101), msg.as_bytes());
        TRACE_RING.emit(tick, ComponentId::OBSERVER, TraceLevel::Error, msg);
    }

    /// Émet une alerte warning.
    pub fn emit_warning(&self, tick: u64, msg: &str) {
        ALERT_LOG.warning(alert::AlertCode::new(0x0201), msg.as_bytes());
        TRACE_RING.emit(tick, ComponentId::OBSERVER, TraceLevel::Warn, msg);
    }

    /// Réinitialise tous les compteurs statistiques.
    pub fn reset_counters(&self) {
        EXOFS_METRICS.reset();
        PERF_COUNTERS.reset();
        LATENCY_TRACKER.reset_all();
        THROUGHPUT_TRACKER.reset();
    }
}

/// Instance globale du module.
pub static OBSERVABILITY: ObservabilityModule = ObservabilityModule::new_const();

// ─── ObservabilitySnapshot ────────────────────────────────────────────────────

/// Snapshot agrégé de tout le module d'observabilité.
#[derive(Debug)]
pub struct ObservabilitySnapshot {
    pub status:       ObservabilityStatus,
    pub metrics:      MetricsSnapshot,
    pub space:        SpaceSnapshot,
    pub perf:         PerfSnapshot,
    pub throughput:   ThroughputSnapshot,
    pub event_count:  u64,
    pub uptime_ticks: u64,
}

impl ObservabilitySnapshot {
    /// Vrai si le système est dans un état nominal.
    pub fn is_healthy(&self) -> bool {
        self.status.is_nominal()
    }

    /// Total I/O en octets.
    pub fn total_io_bytes(&self) -> u64 {
        self.metrics.total_bytes()
    }

    /// Résumé des taux d'erreur.
    pub fn error_rate_ppt(&self) -> u64 {
        self.status.error_rate_ppt
    }

    /// Résumé du débit total.
    pub fn avg_throughput_bpt(&self) -> u64 {
        self.throughput.avg_total_bpt()
    }

    /// Copie les métriques dans un Vec (OOM-02).
    pub fn metrics_to_vec(&self) -> ExofsResult<Vec<u64>> {
        let mut v = Vec::new();
        v.try_reserve(metrics::MetricId::COUNT).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < metrics::MetricId::COUNT {
            v.push(self.metrics.values[i]);
            i = i.wrapping_add(1);
        }
        Ok(v)
    }
}

// ─── Helpers publics ─────────────────────────────────────────────────────────

/// Initialise le module d'observabilité avec la config par défaut.
pub fn observability_init(tick: u64) -> ExofsResult<()> {
    OBSERVABILITY.init(ObservabilityConfig::default_config(), tick)
}

/// Retourne vrai si le système est sain.
pub fn is_healthy() -> bool {
    OBSERVABILITY.status().is_nominal()
}

/// Retourne le taux d'erreur global en ‰.
pub fn global_error_rate_ppt() -> u64 {
    OBSERVABILITY.status().error_rate_ppt
}

/// Retourne le statut de santé courant.
pub fn health_status() -> HealthStatus {
    OBSERVABILITY.status().health
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default_valid() {
        let c = ObservabilityConfig::default_config();
        assert!(c.validate().is_ok());
        assert!(c.metrics_enabled);
        assert!(c.alerts_enabled);
    }

    #[test]
    fn test_config_invalid_period() {
        let mut c = ObservabilityConfig::default_config();
        c.health_eval_period_us = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn test_config_invalid_window() {
        let mut c = ObservabilityConfig::default_config();
        c.throughput_window = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn test_config_is_verbose() {
        let mut c = ObservabilityConfig::default_config();
        c.min_trace_level = TraceLevel::Debug;
        assert!(c.is_verbose());
        c.min_trace_level = TraceLevel::Info;
        assert!(!c.is_verbose());
    }

    #[test]
    fn test_module_init() {
        let m = ObservabilityModule::new_const();
        assert!(m.init(ObservabilityConfig::default_config(), 1000).is_ok());
    }

    #[test]
    fn test_module_status() {
        let m = ObservabilityModule::new_const();
        let _ = m.init(ObservabilityConfig::default_config(), 0);
        let s = m.status();
        let _ = s.is_nominal();
    }

    #[test]
    fn test_module_uptime() {
        let m = ObservabilityModule::new_const();
        let _ = m.init(ObservabilityConfig::default_config(), 100);
        assert_eq!(m.uptime(600), 500);
        assert_eq!(m.uptime(50), 0); // saturating_sub
    }

    #[test]
    fn test_module_snapshot() {
        let m = ObservabilityModule::new_const();
        let _ = m.init(ObservabilityConfig::default_config(), 0);
        let snap = m.full_snapshot().expect("snapshot");
        let _ = snap.is_healthy();
        let _ = snap.total_io_bytes();
    }

    #[test]
    fn test_snapshot_metrics_to_vec() {
        let m = ObservabilityModule::new_const();
        let _ = m.init(ObservabilityConfig::default_config(), 0);
        let snap = m.full_snapshot().expect("snapshot");
        let v = snap.metrics_to_vec().expect("vec");
        assert_eq!(v.len(), metrics::MetricId::COUNT);
    }

    #[test]
    fn test_observability_status_nominal() {
        let s = ObservabilityStatus {
            health:             HealthStatus::Healthy,
            has_critical_alert: false,
            error_rate_ppt:     0,
            space_usage_pct:    50,
            throughput_bpt:     1024,
            trace_dropped:      0,
        };
        assert!(s.is_nominal());
        assert!(!s.needs_attention());
    }

    #[test]
    fn test_observability_status_degraded_nominal() {
        let s = ObservabilityStatus {
            health:             HealthStatus::Degraded,
            has_critical_alert: false,
            error_rate_ppt:     10,
            space_usage_pct:    80,
            throughput_bpt:     512,
            trace_dropped:      0,
        };
        assert!(s.is_nominal()); // Degraded sans critical est encore nominal
    }

    #[test]
    fn test_observability_status_critical_not_nominal() {
        let s = ObservabilityStatus {
            health:             HealthStatus::Critical,
            has_critical_alert: true,
            error_rate_ppt:     200,
            space_usage_pct:    99,
            throughput_bpt:     0,
            trace_dropped:      10,
        };
        assert!(!s.is_nominal());
        assert!(s.needs_attention());
    }

    #[test]
    fn test_helper_observability_init() {
        assert!(observability_init(42).is_ok());
    }

    #[test]
    fn test_helper_is_healthy() {
        let _ = observability_init(0);
        let _ = is_healthy();
    }
}
