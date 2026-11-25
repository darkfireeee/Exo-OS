//! ExoFS Observability facade (`ObservabilityConfig`, `ObservabilityModule`,
//! `ObservabilitySnapshot`, global `OBSERVABILITY`) and tests.
//!
//! Extracted from `exofs/observability/mod.rs` so `mod.rs` remains a pure wiring facade.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use super::{
    alert, metrics, ComponentId, HealthStatus, MetricsSnapshot, PerfSnapshot, SpaceSnapshot,
    ThroughputSnapshot, TraceLevel, ALERT_LOG, EXOFS_METRICS, HEALTH, LATENCY_TRACKER,
    PERF_COUNTERS, SPACE_TRACKER, THROUGHPUT_TRACKER, TRACE_RING,
};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

#[derive(Clone, Copy, Debug)]
pub struct ObservabilityConfig {
    pub metrics_enabled: bool,
    pub alerts_enabled: bool,
    pub min_trace_level: TraceLevel,
    pub health_eval_period_us: u64,
    pub throughput_window: u64,
    pub debug_enabled: bool,
}

impl ObservabilityConfig {
    pub const fn default_config() -> Self {
        Self {
            metrics_enabled: true,
            alerts_enabled: true,
            min_trace_level: TraceLevel::Info,
            health_eval_period_us: 1_000_000,
            throughput_window: 16,
            debug_enabled: true,
        }
    }

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

#[derive(Clone, Copy, Debug)]
pub struct ObservabilityStatus {
    pub health: HealthStatus,
    pub has_critical_alert: bool,
    pub error_rate_ppt: u64,
    pub space_usage_pct: u64,
    pub throughput_bpt: u64,
    pub trace_dropped: u64,
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

#[allow(dead_code)]
pub struct ObservabilityModule {
    config: ObservabilityConfig,
    init_tick: AtomicU64,
    event_count: AtomicU64,
}

impl ObservabilityModule {
    pub const fn new_const() -> Self {
        Self {
            config: ObservabilityConfig::default_config(),
            init_tick: AtomicU64::new(0),
            event_count: AtomicU64::new(0),
        }
    }

    pub fn init(&self, config: ObservabilityConfig, tick: u64) -> ExofsResult<()> {
        config.validate()?;
        TRACE_RING.set_min_level(config.min_trace_level);
        self.init_tick.store(tick, Ordering::Relaxed);
        TRACE_RING.emit(
            tick,
            ComponentId::OBSERVER,
            TraceLevel::Info,
            "observability init ok",
        );
        Ok(())
    }

    pub fn record_event(&self) {
        self.event_count.fetch_add(1, Ordering::Relaxed);
        EXOFS_METRICS.inc_read(1);
    }

    pub fn status(&self) -> ObservabilityStatus {
        ObservabilityStatus {
            health: HEALTH.status(),
            has_critical_alert: ALERT_LOG.has_critical(),
            error_rate_ppt: EXOFS_METRICS
                .error_rate_pct10()
                .saturating_mul(100)
                .checked_div(10)
                .unwrap_or(0),
            space_usage_pct: SPACE_TRACKER.usage_pct() as u64,
            throughput_bpt: THROUGHPUT_TRACKER.avg_total_bpt(),
            trace_dropped: TRACE_RING.dropped(),
        }
    }

    pub fn full_snapshot(&self) -> ExofsResult<ObservabilitySnapshot> {
        let metrics = EXOFS_METRICS.snapshot();
        let space = SPACE_TRACKER.snapshot();
        let perf = PERF_COUNTERS.snapshot();
        let thru = THROUGHPUT_TRACKER.snapshot()?;
        Ok(ObservabilitySnapshot {
            status: self.status(),
            metrics,
            space,
            perf,
            throughput: thru,
            event_count: self.event_count.load(Ordering::Relaxed),
            uptime_ticks: self.uptime(0),
        })
    }

    pub fn uptime(&self, current_tick: u64) -> u64 {
        current_tick.saturating_sub(self.init_tick.load(Ordering::Relaxed))
    }

    pub fn emit_critical(&self, tick: u64, msg: &str) {
        ALERT_LOG.critical(alert::AlertCode::new(0x0101), msg.as_bytes());
        TRACE_RING.emit(tick, ComponentId::OBSERVER, TraceLevel::Error, msg);
    }

    pub fn emit_warning(&self, tick: u64, msg: &str) {
        ALERT_LOG.warning(alert::AlertCode::new(0x0201), msg.as_bytes());
        TRACE_RING.emit(tick, ComponentId::OBSERVER, TraceLevel::Warn, msg);
    }

    pub fn reset_counters(&self) {
        EXOFS_METRICS.reset();
        PERF_COUNTERS.reset();
        LATENCY_TRACKER.reset_all();
        THROUGHPUT_TRACKER.reset();
    }
}

pub static OBSERVABILITY: ObservabilityModule = ObservabilityModule::new_const();

#[derive(Debug)]
pub struct ObservabilitySnapshot {
    pub status: ObservabilityStatus,
    pub metrics: MetricsSnapshot,
    pub space: SpaceSnapshot,
    pub perf: PerfSnapshot,
    pub throughput: ThroughputSnapshot,
    pub event_count: u64,
    pub uptime_ticks: u64,
}

impl ObservabilitySnapshot {
    pub fn is_healthy(&self) -> bool {
        self.status.is_nominal()
    }

    pub fn total_io_bytes(&self) -> u64 {
        self.metrics.total_bytes()
    }

    pub fn error_rate_ppt(&self) -> u64 {
        self.status.error_rate_ppt
    }

    pub fn avg_throughput_bpt(&self) -> u64 {
        self.throughput.avg_total_bpt()
    }

    pub fn metrics_to_vec(&self) -> ExofsResult<Vec<u64>> {
        let mut v = Vec::new();
        v.try_reserve(metrics::MetricId::COUNT)
            .map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < metrics::MetricId::COUNT {
            v.push(self.metrics.values[i]);
            i = i.wrapping_add(1);
        }
        Ok(v)
    }
}

pub fn observability_init(tick: u64) -> ExofsResult<()> {
    OBSERVABILITY.init(ObservabilityConfig::default_config(), tick)
}

pub fn is_healthy() -> bool {
    OBSERVABILITY.status().is_nominal()
}

pub fn global_error_rate_ppt() -> u64 {
    OBSERVABILITY.status().error_rate_ppt
}

pub fn health_status() -> HealthStatus {
    OBSERVABILITY.status().health
}

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
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
        let _ = m.status().is_nominal();
    }

    #[test]
    fn test_module_uptime() {
        let m = ObservabilityModule::new_const();
        let _ = m.init(ObservabilityConfig::default_config(), 100);
        assert_eq!(m.uptime(600), 500);
        assert_eq!(m.uptime(50), 0);
    }

    #[test]
    fn test_module_snapshot() {
        let m = ObservabilityModule::new_const();
        let _ = m.init(ObservabilityConfig::default_config(), 0);
        let snap = m.full_snapshot().test_expect("snapshot");
        let _ = snap.is_healthy();
        let _ = snap.total_io_bytes();
    }

    #[test]
    fn test_snapshot_metrics_to_vec() {
        let m = ObservabilityModule::new_const();
        let _ = m.init(ObservabilityConfig::default_config(), 0);
        let snap = m.full_snapshot().test_expect("snapshot");
        let v = snap.metrics_to_vec().test_expect("vec");
        assert_eq!(v.len(), metrics::MetricId::COUNT);
    }

    #[test]
    fn test_observability_status_nominal() {
        let s = ObservabilityStatus {
            health: HealthStatus::Healthy,
            has_critical_alert: false,
            error_rate_ppt: 0,
            space_usage_pct: 50,
            throughput_bpt: 1024,
            trace_dropped: 0,
        };
        assert!(s.is_nominal());
        assert!(!s.needs_attention());
    }

    #[test]
    fn test_observability_status_degraded_nominal() {
        let s = ObservabilityStatus {
            health: HealthStatus::Degraded,
            has_critical_alert: false,
            error_rate_ppt: 10,
            space_usage_pct: 80,
            throughput_bpt: 512,
            trace_dropped: 0,
        };
        assert!(s.is_nominal());
    }

    #[test]
    fn test_observability_status_critical_not_nominal() {
        let s = ObservabilityStatus {
            health: HealthStatus::Critical,
            has_critical_alert: true,
            error_rate_ppt: 200,
            space_usage_pct: 99,
            throughput_bpt: 0,
            trace_dropped: 10,
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
