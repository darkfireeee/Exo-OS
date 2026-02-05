//! Metrics registry

use crate::{Counter, Gauge, Histogram};
use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;

pub enum Metric {
    Counter(Box<Counter>),
    Gauge(Box<Gauge>),
    Histogram(Box<Histogram>),
}

pub struct MetricEntry {
    pub name: String,
    pub metric: Metric,
}

pub struct MetricsRegistry {
    metrics: Vec<MetricEntry>,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self {
            metrics: Vec::new(),
        }
    }
    
    pub fn register_counter(&mut self, name: String) -> &Counter {
        let counter = Box::new(Counter::new());
        let ptr = &*counter as *const Counter;
        self.metrics.push(MetricEntry {
            name,
            metric: Metric::Counter(counter),
        });
        unsafe { &*ptr }
    }
    
    pub fn register_gauge(&mut self, name: String) -> &Gauge {
        let gauge = Box::new(Gauge::new());
        let ptr = &*gauge as *const Gauge;
        self.metrics.push(MetricEntry {
            name,
            metric: Metric::Gauge(gauge),
        });
        unsafe { &*ptr }
    }
    
    pub fn register_histogram(&mut self, name: String) -> &Histogram {
        let histogram = Box::new(Histogram::new());
        let ptr = &*histogram as *const Histogram;
        self.metrics.push(MetricEntry {
            name,
            metric: Metric::Histogram(histogram),
        });
        unsafe { &*ptr }
    }
    
    pub fn iter(&self) -> impl Iterator<Item = &MetricEntry> {
        self.metrics.iter()
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}
