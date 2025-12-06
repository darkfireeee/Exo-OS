//! # Network Performance Monitoring & Telemetry
//! 
//! Système de monitoring ultra-performant pour diagnostics réseau.
//! 
//! ## Features
//! - Zero-overhead metrics (atomic counters)
//! - Per-CPU statistics
//! - Latency histograms
//! - Bandwidth tracking
//! - Connection tracking stats

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use crate::sync::SpinLock;
use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};

/// Métriques réseau globales
pub struct NetworkMetrics {
    // Packets
    pub rx_packets: AtomicU64,
    pub tx_packets: AtomicU64,
    pub rx_bytes: AtomicU64,
    pub tx_bytes: AtomicU64,
    
    // Errors
    pub rx_errors: AtomicU64,
    pub tx_errors: AtomicU64,
    pub rx_dropped: AtomicU64,
    pub tx_dropped: AtomicU64,
    
    // TCP
    pub tcp_active_opens: AtomicU64,
    pub tcp_passive_opens: AtomicU64,
    pub tcp_failed_conns: AtomicU64,
    pub tcp_resets: AtomicU64,
    pub tcp_curr_estab: AtomicU32,
    pub tcp_retrans_segs: AtomicU64,
    
    // UDP
    pub udp_in_datagrams: AtomicU64,
    pub udp_out_datagrams: AtomicU64,
    pub udp_no_ports: AtomicU64,
    pub udp_in_errors: AtomicU64,
    
    // ICMP
    pub icmp_in_msgs: AtomicU64,
    pub icmp_out_msgs: AtomicU64,
    pub icmp_in_errors: AtomicU64,
    
    // Performance
    pub zero_copy_tx: AtomicU64,
    pub zero_copy_rx: AtomicU64,
    pub gso_packets: AtomicU64,
    pub gro_packets: AtomicU64,
}

impl NetworkMetrics {
    pub const fn new() -> Self {
        Self {
            rx_packets: AtomicU64::new(0),
            tx_packets: AtomicU64::new(0),
            rx_bytes: AtomicU64::new(0),
            tx_bytes: AtomicU64::new(0),
            rx_errors: AtomicU64::new(0),
            tx_errors: AtomicU64::new(0),
            rx_dropped: AtomicU64::new(0),
            tx_dropped: AtomicU64::new(0),
            tcp_active_opens: AtomicU64::new(0),
            tcp_passive_opens: AtomicU64::new(0),
            tcp_failed_conns: AtomicU64::new(0),
            tcp_resets: AtomicU64::new(0),
            tcp_curr_estab: AtomicU32::new(0),
            tcp_retrans_segs: AtomicU64::new(0),
            udp_in_datagrams: AtomicU64::new(0),
            udp_out_datagrams: AtomicU64::new(0),
            udp_no_ports: AtomicU64::new(0),
            udp_in_errors: AtomicU64::new(0),
            icmp_in_msgs: AtomicU64::new(0),
            icmp_out_msgs: AtomicU64::new(0),
            icmp_in_errors: AtomicU64::new(0),
            zero_copy_tx: AtomicU64::new(0),
            zero_copy_rx: AtomicU64::new(0),
            gso_packets: AtomicU64::new(0),
            gro_packets: AtomicU64::new(0),
        }
    }
    
    /// Snapshot des métriques
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            rx_packets: self.rx_packets.load(Ordering::Relaxed),
            tx_packets: self.tx_packets.load(Ordering::Relaxed),
            rx_bytes: self.rx_bytes.load(Ordering::Relaxed),
            tx_bytes: self.tx_bytes.load(Ordering::Relaxed),
            rx_errors: self.rx_errors.load(Ordering::Relaxed),
            tx_errors: self.tx_errors.load(Ordering::Relaxed),
            rx_dropped: self.rx_dropped.load(Ordering::Relaxed),
            tx_dropped: self.tx_dropped.load(Ordering::Relaxed),
            tcp_active_opens: self.tcp_active_opens.load(Ordering::Relaxed),
            tcp_passive_opens: self.tcp_passive_opens.load(Ordering::Relaxed),
            tcp_failed_conns: self.tcp_failed_conns.load(Ordering::Relaxed),
            tcp_resets: self.tcp_resets.load(Ordering::Relaxed),
            tcp_curr_estab: self.tcp_curr_estab.load(Ordering::Relaxed),
            tcp_retrans_segs: self.tcp_retrans_segs.load(Ordering::Relaxed),
            udp_in_datagrams: self.udp_in_datagrams.load(Ordering::Relaxed),
            udp_out_datagrams: self.udp_out_datagrams.load(Ordering::Relaxed),
            udp_no_ports: self.udp_no_ports.load(Ordering::Relaxed),
            udp_in_errors: self.udp_in_errors.load(Ordering::Relaxed),
            icmp_in_msgs: self.icmp_in_msgs.load(Ordering::Relaxed),
            icmp_out_msgs: self.icmp_out_msgs.load(Ordering::Relaxed),
            icmp_in_errors: self.icmp_in_errors.load(Ordering::Relaxed),
            zero_copy_tx: self.zero_copy_tx.load(Ordering::Relaxed),
            zero_copy_rx: self.zero_copy_rx.load(Ordering::Relaxed),
            gso_packets: self.gso_packets.load(Ordering::Relaxed),
            gro_packets: self.gro_packets.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot immutable des métriques
#[derive(Debug, Clone, Copy)]
pub struct MetricsSnapshot {
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
    pub rx_dropped: u64,
    pub tx_dropped: u64,
    pub tcp_active_opens: u64,
    pub tcp_passive_opens: u64,
    pub tcp_failed_conns: u64,
    pub tcp_resets: u64,
    pub tcp_curr_estab: u32,
    pub tcp_retrans_segs: u64,
    pub udp_in_datagrams: u64,
    pub udp_out_datagrams: u64,
    pub udp_no_ports: u64,
    pub udp_in_errors: u64,
    pub icmp_in_msgs: u64,
    pub icmp_out_msgs: u64,
    pub icmp_in_errors: u64,
    pub zero_copy_tx: u64,
    pub zero_copy_rx: u64,
    pub gso_packets: u64,
    pub gro_packets: u64,
}

impl MetricsSnapshot {
    /// Calcule delta entre deux snapshots
    pub fn delta(&self, previous: &Self) -> Self {
        Self {
            rx_packets: self.rx_packets.saturating_sub(previous.rx_packets),
            tx_packets: self.tx_packets.saturating_sub(previous.tx_packets),
            rx_bytes: self.rx_bytes.saturating_sub(previous.rx_bytes),
            tx_bytes: self.tx_bytes.saturating_sub(previous.tx_bytes),
            rx_errors: self.rx_errors.saturating_sub(previous.rx_errors),
            tx_errors: self.tx_errors.saturating_sub(previous.tx_errors),
            rx_dropped: self.rx_dropped.saturating_sub(previous.rx_dropped),
            tx_dropped: self.tx_dropped.saturating_sub(previous.tx_dropped),
            tcp_active_opens: self.tcp_active_opens.saturating_sub(previous.tcp_active_opens),
            tcp_passive_opens: self.tcp_passive_opens.saturating_sub(previous.tcp_passive_opens),
            tcp_failed_conns: self.tcp_failed_conns.saturating_sub(previous.tcp_failed_conns),
            tcp_resets: self.tcp_resets.saturating_sub(previous.tcp_resets),
            tcp_curr_estab: self.tcp_curr_estab, // Pas de delta pour valeur instantanée
            tcp_retrans_segs: self.tcp_retrans_segs.saturating_sub(previous.tcp_retrans_segs),
            udp_in_datagrams: self.udp_in_datagrams.saturating_sub(previous.udp_in_datagrams),
            udp_out_datagrams: self.udp_out_datagrams.saturating_sub(previous.udp_out_datagrams),
            udp_no_ports: self.udp_no_ports.saturating_sub(previous.udp_no_ports),
            udp_in_errors: self.udp_in_errors.saturating_sub(previous.udp_in_errors),
            icmp_in_msgs: self.icmp_in_msgs.saturating_sub(previous.icmp_in_msgs),
            icmp_out_msgs: self.icmp_out_msgs.saturating_sub(previous.icmp_out_msgs),
            icmp_in_errors: self.icmp_in_errors.saturating_sub(previous.icmp_in_errors),
            zero_copy_tx: self.zero_copy_tx.saturating_sub(previous.zero_copy_tx),
            zero_copy_rx: self.zero_copy_rx.saturating_sub(previous.zero_copy_rx),
            gso_packets: self.gso_packets.saturating_sub(previous.gso_packets),
            gro_packets: self.gro_packets.saturating_sub(previous.gro_packets),
        }
    }
    
    /// Calcule taux par seconde
    pub fn rates(&self, interval_secs: u64) -> RatesSnapshot {
        if interval_secs == 0 {
            return RatesSnapshot::default();
        }
        
        let interval = interval_secs as f64;
        
        RatesSnapshot {
            rx_pps: self.rx_packets as f64 / interval,
            tx_pps: self.tx_packets as f64 / interval,
            rx_bps: (self.rx_bytes * 8) as f64 / interval,
            tx_bps: (self.tx_bytes * 8) as f64 / interval,
            rx_error_rate: self.rx_errors as f64 / interval,
            tx_error_rate: self.tx_errors as f64 / interval,
            rx_drop_rate: self.rx_dropped as f64 / interval,
            tx_drop_rate: self.tx_dropped as f64 / interval,
        }
    }
}

/// Taux par seconde
#[derive(Debug, Clone, Copy, Default)]
pub struct RatesSnapshot {
    pub rx_pps: f64,     // packets/sec
    pub tx_pps: f64,
    pub rx_bps: f64,     // bits/sec
    pub tx_bps: f64,
    pub rx_error_rate: f64,
    pub tx_error_rate: f64,
    pub rx_drop_rate: f64,
    pub tx_drop_rate: f64,
}

/// Histogramme de latences
pub struct LatencyHistogram {
    buckets: [AtomicU64; 32], // 32 buckets exponentiels
    count: AtomicU64,
    sum: AtomicU64,
}

impl LatencyHistogram {
    pub const fn new() -> Self {
        const BUCKET: AtomicU64 = AtomicU64::new(0);
        Self {
            buckets: [BUCKET; 32],
            count: AtomicU64::new(0),
            sum: AtomicU64::new(0),
        }
    }
    
    /// Enregistre une latence (en microsecondes)
    pub fn record(&self, latency_us: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum.fetch_add(latency_us, Ordering::Relaxed);
        
        // Trouve bucket (échelle log)
        let bucket = if latency_us == 0 {
            0
        } else {
            let bits = 64 - latency_us.leading_zeros();
            bits.min(31) as usize
        };
        
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn percentile(&self, p: f64) -> u64 {
        let total = self.count.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        
        let target = ((total as f64) * p) as u64;
        let mut cumulative = 0u64;
        
        for (i, bucket) in self.buckets.iter().enumerate() {
            cumulative += bucket.load(Ordering::Relaxed);
            if cumulative >= target {
                return 1u64 << i;
            }
        }
        
        u64::MAX
    }
    
    pub fn mean(&self) -> f64 {
        let count = self.count.load(Ordering::Relaxed);
        if count == 0 {
            return 0.0;
        }
        
        let sum = self.sum.load(Ordering::Relaxed);
        sum as f64 / count as f64
    }
}

/// Métriques par interface
pub struct InterfaceMetrics {
    pub name: &'static str,
    pub metrics: NetworkMetrics,
    pub latency: LatencyHistogram,
}

impl InterfaceMetrics {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            metrics: NetworkMetrics::new(),
            latency: LatencyHistogram::new(),
        }
    }
}

/// Monitoring global
pub struct NetworkMonitoring {
    global: NetworkMetrics,
    interfaces: SpinLock<BTreeMap<u32, InterfaceMetrics>>,
    latency: LatencyHistogram,
}

impl NetworkMonitoring {
    pub const fn new() -> Self {
        Self {
            global: NetworkMetrics::new(),
            interfaces: SpinLock::new(BTreeMap::new()),
            latency: LatencyHistogram::new(),
        }
    }
    
    pub fn global(&self) -> &NetworkMetrics {
        &self.global
    }
    
    pub fn add_interface(&self, id: u32, name: &'static str) {
        self.interfaces.lock().insert(id, InterfaceMetrics::new(name));
    }
    
    pub fn get_interface(&self, id: u32) -> Option<MetricsSnapshot> {
        self.interfaces.lock()
            .get(&id)
            .map(|iface| iface.metrics.snapshot())
    }
    
    pub fn record_latency(&self, latency_us: u64) {
        self.latency.record(latency_us);
    }
    
    pub fn latency_stats(&self) -> LatencyStats {
        LatencyStats {
            mean: self.latency.mean(),
            p50: self.latency.percentile(0.5),
            p95: self.latency.percentile(0.95),
            p99: self.latency.percentile(0.99),
            p999: self.latency.percentile(0.999),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LatencyStats {
    pub mean: f64,
    pub p50: u64,
    pub p95: u64,
    pub p99: u64,
    pub p999: u64,
}

/// Instance globale
static MONITORING: NetworkMonitoring = NetworkMonitoring::new();

pub fn monitoring() -> &'static NetworkMonitoring {
    &MONITORING
}

/// Macros helper pour metrics
#[macro_export]
macro_rules! net_metric_inc {
    ($metric:ident) => {
        $crate::net::monitoring::monitoring()
            .global()
            .$metric
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    };
    ($metric:ident, $value:expr) => {
        $crate::net::monitoring::monitoring()
            .global()
            .$metric
            .fetch_add($value, core::sync::atomic::Ordering::Relaxed);
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_histogram() {
        let hist = LatencyHistogram::new();
        
        hist.record(10);
        hist.record(20);
        hist.record(100);
        hist.record(1000);
        
        let mean = hist.mean();
        assert!(mean > 0.0);
        
        let p50 = hist.percentile(0.5);
        assert!(p50 > 0);
    }
    
    #[test]
    fn test_delta() {
        let mut snap1 = MetricsSnapshot {
            rx_packets: 1000,
            tx_packets: 500,
            rx_bytes: 100_000,
            tx_bytes: 50_000,
            ..unsafe { core::mem::zeroed() }
        };
        
        let snap2 = MetricsSnapshot {
            rx_packets: 2000,
            tx_packets: 1000,
            rx_bytes: 250_000,
            tx_bytes: 125_000,
            ..unsafe { core::mem::zeroed() }
        };
        
        let delta = snap2.delta(&snap1);
        assert_eq!(delta.rx_packets, 1000);
        assert_eq!(delta.tx_packets, 500);
    }
}
