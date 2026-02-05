//! Prometheus text format exporter

use crate::{MetricsRegistry, Metric};
use alloc::string::String;
use alloc::format;

pub struct PrometheusExporter;

impl PrometheusExporter {
    /// Export all metrics in Prometheus text format
    pub fn export(registry: &MetricsRegistry) -> String {
        let mut output = String::new();
        
        for entry in registry.iter() {
            match &entry.metric {
                Metric::Counter(counter) => {
                    output.push_str(&format!("# TYPE {} counter\n", entry.name));
                    output.push_str(&format!("{} {}\n", entry.name, counter.get()));
                }
                Metric::Gauge(gauge) => {
                    output.push_str(&format!("# TYPE {} gauge\n", entry.name));
                    output.push_str(&format!("{} {}\n", entry.name, gauge.get()));
                }
                Metric::Histogram(histogram) => {
                    output.push_str(&format!("# TYPE {} histogram\n", entry.name));
                    
                    // Buckets
                    for (boundary, count) in histogram.buckets() {
                        if boundary == u64::MAX {
                            output.push_str(&format!("{}_bucket{{le=\"+Inf\"}} {}\n", entry.name, count));
                        } else {
                            output.push_str(&format!("{}_bucket{{le=\"{}\"}} {}\n", entry.name, boundary, count));
                        }
                    }
                    
                    // Sum and count
                    output.push_str(&format!("{}_sum {}\n", entry.name, histogram.sum()));
                    output.push_str(&format!("{}_count {}\n", entry.name, histogram.count()));
                }
            }
            output.push('\n');
        }
        
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    
    #[test]
    fn test_prometheus_export() {
        let mut registry = MetricsRegistry::new();
        
        let counter = registry.register_counter("test_counter".to_string());
        counter.inc();
        counter.add(5);
        
        let gauge = registry.register_gauge("test_gauge".to_string());
        gauge.set(42);
        
        let output = PrometheusExporter::export(&registry);
        
        assert!(output.contains("# TYPE test_counter counter"));
        assert!(output.contains("test_counter 6"));
        assert!(output.contains("# TYPE test_gauge gauge"));
        assert!(output.contains("test_gauge 42"));
    }
}
