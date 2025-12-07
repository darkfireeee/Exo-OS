//! # Prometheus Metrics Export
//! 
//! Export de métriques réseau au format Prometheus

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::sync::atomic::{AtomicU64, Ordering};

/// Metric type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricType {
    Counter,
    Gauge,
    Histogram,
}

/// Network metric
pub struct NetworkMetric {
    pub name: String,
    pub metric_type: MetricType,
    pub value: AtomicU64,
    pub help: String,
}

impl NetworkMetric {
    pub fn new_counter(name: String, help: String) -> Self {
        Self {
            name,
            metric_type: MetricType::Counter,
            value: AtomicU64::new(0),
            help,
        }
    }
    
    pub fn new_gauge(name: String, help: String) -> Self {
        Self {
            name,
            metric_type: MetricType::Gauge,
            value: AtomicU64::new(0),
            help,
        }
    }
    
    pub fn inc(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn add(&self, val: u64) {
        self.value.fetch_add(val, Ordering::Relaxed);
    }
    
    pub fn set(&self, val: u64) {
        self.value.store(val, Ordering::Relaxed);
    }
    
    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }
    
    /// Export to Prometheus format
    pub fn to_prometheus(&self) -> String {
        let type_str = match self.metric_type {
            MetricType::Counter => "counter",
            MetricType::Gauge => "gauge",
            MetricType::Histogram => "histogram",
        };
        
        format!(
            "# HELP {} {}\n# TYPE {} {}\n{} {}\n",
            self.name,
            self.help,
            self.name,
            type_str,
            self.name,
            self.get()
        )
    }
}

/// Metrics registry
pub struct MetricsRegistry {
    metrics: Vec<NetworkMetric>,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            metrics: Vec::new(),
        };
        
        // Register default metrics
        registry.register(NetworkMetric::new_counter(
            "net_packets_rx_total".into(),
            "Total packets received".into(),
        ));
        
        registry.register(NetworkMetric::new_counter(
            "net_packets_tx_total".into(),
            "Total packets transmitted".into(),
        ));
        
        registry.register(NetworkMetric::new_counter(
            "net_bytes_rx_total".into(),
            "Total bytes received".into(),
        ));
        
        registry.register(NetworkMetric::new_counter(
            "net_bytes_tx_total".into(),
            "Total bytes transmitted".into(),
        ));
        
        registry.register(NetworkMetric::new_counter(
            "net_errors_rx_total".into(),
            "Total receive errors".into(),
        ));
        
        registry.register(NetworkMetric::new_counter(
            "net_errors_tx_total".into(),
            "Total transmit errors".into(),
        ));
        
        registry.register(NetworkMetric::new_gauge(
            "net_connections_active".into(),
            "Active connections".into(),
        ));
        
        registry
    }
    
    pub fn register(&mut self, metric: NetworkMetric) {
        self.metrics.push(metric);
    }
    
    /// Export all metrics to Prometheus format
    pub fn export(&self) -> String {
        let mut output = String::new();
        
        for metric in &self.metrics {
            output.push_str(&metric.to_prometheus());
        }
        
        output
    }
}
