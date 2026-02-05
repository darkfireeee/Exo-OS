//! Prometheus exporter

use crate::Result;

/// Prometheus exporter
pub struct PrometheusExporter;

impl PrometheusExporter {
    /// Create new exporter
    pub fn new() -> Self {
        Self
    }

    /// Export metrics in Prometheus text format
    pub fn export(&self) -> Result<()> {
        Ok(())
    }
}
