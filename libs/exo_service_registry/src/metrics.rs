//! Metrics export pour monitoring
//!
//! Export des métriques registry au format Prometheus et formats personnalisés.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::Ordering;

use crate::registry::RegistryStats;
use crate::types::ServiceStatus;

/// Format d'export des métriques
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricsFormat {
    /// Format Prometheus (texte)
    Prometheus,
    /// Format JSON
    Json,
    /// Format texte simple
    Plain,
}

/// Exporte les métriques du registry
pub struct MetricsExporter {
    /// Format d'export
    format: MetricsFormat,

    /// Préfixe pour les métriques
    prefix: String,
}

impl MetricsExporter {
    /// Crée un nouvel exporter
    pub fn new(format: MetricsFormat) -> Self {
        Self {
            format,
            prefix: "exo_registry".into(),
        }
    }

    /// Définit le préfixe des métriques
    pub fn with_prefix(mut self, prefix: String) -> Self {
        self.prefix = prefix;
        self
    }

    /// Exporte les stats au format choisi
    pub fn export(&self, stats: &RegistryStats) -> String {
        match self.format {
            MetricsFormat::Prometheus => self.export_prometheus(stats),
            MetricsFormat::Json => self.export_json(stats),
            MetricsFormat::Plain => self.export_plain(stats),
        }
    }

    /// Export au format Prometheus
    fn export_prometheus(&self, stats: &RegistryStats) -> String {
        let mut output = String::new();

        // Total lookups
        output.push_str(&format!(
            "# HELP {}_lookups_total Total number of service lookups\n",
            self.prefix
        ));
        output.push_str(&format!(
            "# TYPE {}_lookups_total counter\n",
            self.prefix
        ));
        output.push_str(&format!(
            "{}_lookups_total {}\n\n",
            self.prefix,
            stats.total_lookups.load(Ordering::Relaxed)
        ));

        // Cache hits
        output.push_str(&format!(
            "# HELP {}_cache_hits_total Cache hit count\n",
            self.prefix
        ));
        output.push_str(&format!(
            "# TYPE {}_cache_hits_total counter\n",
            self.prefix
        ));
        output.push_str(&format!(
            "{}_cache_hits_total {}\n\n",
            self.prefix,
            stats.cache_hits.load(Ordering::Relaxed)
        ));

        // Cache misses
        output.push_str(&format!(
            "# HELP {}_cache_misses_total Cache miss count\n",
            self.prefix
        ));
        output.push_str(&format!(
            "# TYPE {}_cache_misses_total counter\n",
            self.prefix
        ));
        output.push_str(&format!(
            "{}_cache_misses_total {}\n\n",
            self.prefix,
            stats.cache_misses.load(Ordering::Relaxed)
        ));

        // Cache hit rate
        let total = stats.total_lookups.load(Ordering::Relaxed);
        let hits = stats.cache_hits.load(Ordering::Relaxed);
        let hit_rate = if total > 0 {
            hits as f64 / total as f64
        } else {
            0.0
        };

        output.push_str(&format!(
            "# HELP {}_cache_hit_rate Cache hit rate (0.0 to 1.0)\n",
            self.prefix
        ));
        output.push_str(&format!(
            "# TYPE {}_cache_hit_rate gauge\n",
            self.prefix
        ));
        output.push_str(&format!(
            "{}_cache_hit_rate {:.4}\n\n",
            self.prefix,
            hit_rate
        ));

        // Bloom rejections
        output.push_str(&format!(
            "# HELP {}_bloom_rejections_total Bloom filter rejection count\n",
            self.prefix
        ));
        output.push_str(&format!(
            "# TYPE {}_bloom_rejections_total counter\n",
            self.prefix
        ));
        output.push_str(&format!(
            "{}_bloom_rejections_total {}\n\n",
            self.prefix,
            stats.bloom_rejections.load(Ordering::Relaxed)
        ));

        // Total registrations
        output.push_str(&format!(
            "# HELP {}_registrations_total Total service registrations\n",
            self.prefix
        ));
        output.push_str(&format!(
            "# TYPE {}_registrations_total counter\n",
            self.prefix
        ));
        output.push_str(&format!(
            "{}_registrations_total {}\n\n",
            self.prefix,
            stats.total_registrations.load(Ordering::Relaxed)
        ));

        // Total unregistrations
        output.push_str(&format!(
            "# HELP {}_unregistrations_total Total service unregistrations\n",
            self.prefix
        ));
        output.push_str(&format!(
            "# TYPE {}_unregistrations_total counter\n",
            self.prefix
        ));
        output.push_str(&format!(
            "{}_unregistrations_total {}\n\n",
            self.prefix,
            stats.total_unregistrations.load(Ordering::Relaxed)
        ));

        // Active services
        output.push_str(&format!(
            "# HELP {}_active_services Current number of active services\n",
            self.prefix
        ));
        output.push_str(&format!(
            "# TYPE {}_active_services gauge\n",
            self.prefix
        ));
        output.push_str(&format!(
            "{}_active_services {}\n",
            self.prefix,
            stats.active_services.load(Ordering::Relaxed)
        ));

        output
    }

    /// Export au format JSON
    fn export_json(&self, stats: &RegistryStats) -> String {
        let total = stats.total_lookups.load(Ordering::Relaxed);
        let hits = stats.cache_hits.load(Ordering::Relaxed);
        let hit_rate = if total > 0 {
            hits as f64 / total as f64
        } else {
            0.0
        };

        format!(
            r#"{{
  "total_lookups": {},
  "cache_hits": {},
  "cache_misses": {},
  "cache_hit_rate": {:.4},
  "bloom_rejections": {},
  "total_registrations": {},
  "total_unregistrations": {},
  "active_services": {}
}}"#,
            total,
            hits,
            stats.cache_misses.load(Ordering::Relaxed),
            hit_rate,
            stats.bloom_rejections.load(Ordering::Relaxed),
            stats.total_registrations.load(Ordering::Relaxed),
            stats.total_unregistrations.load(Ordering::Relaxed),
            stats.active_services.load(Ordering::Relaxed)
        )
    }

    /// Export au format texte
    fn export_plain(&self, stats: &RegistryStats) -> String {
        let total = stats.total_lookups.load(Ordering::Relaxed);
        let hits = stats.cache_hits.load(Ordering::Relaxed);
        let hit_rate = if total > 0 {
            hits as f64 / total as f64
        } else {
            0.0
        };

        format!(
            "Registry Statistics:
  Total Lookups: {}
  Cache Hits: {}
  Cache Misses: {}
  Cache Hit Rate: {:.1}%
  Bloom Rejections: {}
  Total Registrations: {}
  Total Unregistrations: {}
  Active Services: {}",
            total,
            hits,
            stats.cache_misses.load(Ordering::Relaxed),
            hit_rate * 100.0,
            stats.bloom_rejections.load(Ordering::Relaxed),
            stats.total_registrations.load(Ordering::Relaxed),
            stats.total_unregistrations.load(Ordering::Relaxed),
            stats.active_services.load(Ordering::Relaxed)
        )
    }
}

/// HTTP endpoint pour metrics (simulé)
pub struct MetricsEndpoint {
    exporter: MetricsExporter,
}

impl MetricsEndpoint {
    /// Crée un nouveau endpoint
    pub fn new(format: MetricsFormat) -> Self {
        Self {
            exporter: MetricsExporter::new(format),
        }
    }

    /// Gère une requête GET /metrics
    pub fn handle_request(&self, stats: &RegistryStats) -> (u16, String, String) {
        let body = self.exporter.export(stats);
        let content_type = match self.exporter.format {
            MetricsFormat::Prometheus => "text/plain; version=0.0.4",
            MetricsFormat::Json => "application/json",
            MetricsFormat::Plain => "text/plain",
        };

        (200, content_type.into(), body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::AtomicUsize;

    fn create_test_stats() -> RegistryStats {
        RegistryStats {
            total_lookups: AtomicUsize::new(1000),
            cache_hits: AtomicUsize::new(900),
            cache_misses: AtomicUsize::new(100),
            bloom_rejections: AtomicUsize::new(50),
            total_registrations: AtomicUsize::new(150),
            total_unregistrations: AtomicUsize::new(10),
            active_services: AtomicUsize::new(140),
        }
    }

    #[test]
    fn test_prometheus_export() {
        let exporter = MetricsExporter::new(MetricsFormat::Prometheus);
        let stats = create_test_stats();
        let output = exporter.export(&stats);

        assert!(output.contains("exo_registry_lookups_total 1000"));
        assert!(output.contains("exo_registry_cache_hits_total 900"));
        assert!(output.contains("exo_registry_active_services 140"));
        assert!(output.contains("# HELP"));
        assert!(output.contains("# TYPE"));
    }

    #[test]
    fn test_json_export() {
        let exporter = MetricsExporter::new(MetricsFormat::Json);
        let stats = create_test_stats();
        let output = exporter.export(&stats);

        assert!(output.contains(r#""total_lookups": 1000"#));
        assert!(output.contains(r#""cache_hits": 900"#));
        assert!(output.contains(r#""active_services": 140"#));
    }

    #[test]
    fn test_plain_export() {
        let exporter = MetricsExporter::new(MetricsFormat::Plain);
        let stats = create_test_stats();
        let output = exporter.export(&stats);

        assert!(output.contains("Total Lookups: 1000"));
        assert!(output.contains("Cache Hit Rate: 90.0%"));
        assert!(output.contains("Active Services: 140"));
    }

    #[test]
    fn test_metrics_endpoint() {
        let endpoint = MetricsEndpoint::new(MetricsFormat::Prometheus);
        let stats = create_test_stats();

        let (status, content_type, body) = endpoint.handle_request(&stats);

        assert_eq!(status, 200);
        assert_eq!(content_type, "text/plain; version=0.0.4");
        assert!(body.contains("exo_registry_lookups_total"));
    }

    #[test]
    fn test_custom_prefix() {
        let exporter = MetricsExporter::new(MetricsFormat::Prometheus)
            .with_prefix("custom_registry".into());
        let stats = create_test_stats();
        let output = exporter.export(&stats);

        assert!(output.contains("custom_registry_lookups_total"));
        assert!(!output.contains("exo_registry_lookups_total"));
    }
}
