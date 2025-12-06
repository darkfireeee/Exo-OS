//! # Network Statistics Collection
//! 
//! Centralized stats collection pour monitoring

use core::sync::atomic::{AtomicU64, Ordering};

/// Stats globales réseau
pub struct NetworkStats {
    /// Packets
    pub total_rx_packets: AtomicU64,
    pub total_tx_packets: AtomicU64,
    pub total_rx_bytes: AtomicU64,
    pub total_tx_bytes: AtomicU64,
    
    /// Errors
    pub total_rx_errors: AtomicU64,
    pub total_tx_errors: AtomicU64,
    pub total_rx_dropped: AtomicU64,
    pub total_tx_dropped: AtomicU64,
    
    /// Protocol-specific
    pub tcp_connections: AtomicU64,
    pub tcp_segments_sent: AtomicU64,
    pub tcp_segments_recv: AtomicU64,
    pub tcp_retransmits: AtomicU64,
    
    pub udp_datagrams_sent: AtomicU64,
    pub udp_datagrams_recv: AtomicU64,
    pub udp_errors: AtomicU64,
    
    pub icmp_msgs_sent: AtomicU64,
    pub icmp_msgs_recv: AtomicU64,
    
    /// Advanced
    pub zero_copy_ops: AtomicU64,
    pub cache_hits: AtomicU64,
    pub cache_misses: AtomicU64,
}

impl NetworkStats {
    pub const fn new() -> Self {
        Self {
            total_rx_packets: AtomicU64::new(0),
            total_tx_packets: AtomicU64::new(0),
            total_rx_bytes: AtomicU64::new(0),
            total_tx_bytes: AtomicU64::new(0),
            total_rx_errors: AtomicU64::new(0),
            total_tx_errors: AtomicU64::new(0),
            total_rx_dropped: AtomicU64::new(0),
            total_tx_dropped: AtomicU64::new(0),
            tcp_connections: AtomicU64::new(0),
            tcp_segments_sent: AtomicU64::new(0),
            tcp_segments_recv: AtomicU64::new(0),
            tcp_retransmits: AtomicU64::new(0),
            udp_datagrams_sent: AtomicU64::new(0),
            udp_datagrams_recv: AtomicU64::new(0),
            udp_errors: AtomicU64::new(0),
            icmp_msgs_sent: AtomicU64::new(0),
            icmp_msgs_recv: AtomicU64::new(0),
            zero_copy_ops: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
        }
    }
    
    pub fn snapshot(&self) -> NetworkStatsSnapshot {
        NetworkStatsSnapshot {
            total_rx_packets: self.total_rx_packets.load(Ordering::Relaxed),
            total_tx_packets: self.total_tx_packets.load(Ordering::Relaxed),
            total_rx_bytes: self.total_rx_bytes.load(Ordering::Relaxed),
            total_tx_bytes: self.total_tx_bytes.load(Ordering::Relaxed),
            total_rx_errors: self.total_rx_errors.load(Ordering::Relaxed),
            total_tx_errors: self.total_tx_errors.load(Ordering::Relaxed),
            total_rx_dropped: self.total_rx_dropped.load(Ordering::Relaxed),
            total_tx_dropped: self.total_tx_dropped.load(Ordering::Relaxed),
            tcp_connections: self.tcp_connections.load(Ordering::Relaxed),
            tcp_segments_sent: self.tcp_segments_sent.load(Ordering::Relaxed),
            tcp_segments_recv: self.tcp_segments_recv.load(Ordering::Relaxed),
            tcp_retransmits: self.tcp_retransmits.load(Ordering::Relaxed),
            udp_datagrams_sent: self.udp_datagrams_sent.load(Ordering::Relaxed),
            udp_datagrams_recv: self.udp_datagrams_recv.load(Ordering::Relaxed),
            udp_errors: self.udp_errors.load(Ordering::Relaxed),
            icmp_msgs_sent: self.icmp_msgs_sent.load(Ordering::Relaxed),
            icmp_msgs_recv: self.icmp_msgs_recv.load(Ordering::Relaxed),
            zero_copy_ops: self.zero_copy_ops.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.cache_misses.load(Ordering::Relaxed),
        }
    }
    
    /// Reset all stats
    pub fn reset(&self) {
        self.total_rx_packets.store(0, Ordering::Relaxed);
        self.total_tx_packets.store(0, Ordering::Relaxed);
        self.total_rx_bytes.store(0, Ordering::Relaxed);
        self.total_tx_bytes.store(0, Ordering::Relaxed);
        self.total_rx_errors.store(0, Ordering::Relaxed);
        self.total_tx_errors.store(0, Ordering::Relaxed);
        self.total_rx_dropped.store(0, Ordering::Relaxed);
        self.total_tx_dropped.store(0, Ordering::Relaxed);
        self.tcp_connections.store(0, Ordering::Relaxed);
        self.tcp_segments_sent.store(0, Ordering::Relaxed);
        self.tcp_segments_recv.store(0, Ordering::Relaxed);
        self.tcp_retransmits.store(0, Ordering::Relaxed);
        self.udp_datagrams_sent.store(0, Ordering::Relaxed);
        self.udp_datagrams_recv.store(0, Ordering::Relaxed);
        self.udp_errors.store(0, Ordering::Relaxed);
        self.icmp_msgs_sent.store(0, Ordering::Relaxed);
        self.icmp_msgs_recv.store(0, Ordering::Relaxed);
        self.zero_copy_ops.store(0, Ordering::Relaxed);
        self.cache_hits.store(0, Ordering::Relaxed);
        self.cache_misses.store(0, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NetworkStatsSnapshot {
    pub total_rx_packets: u64,
    pub total_tx_packets: u64,
    pub total_rx_bytes: u64,
    pub total_tx_bytes: u64,
    pub total_rx_errors: u64,
    pub total_tx_errors: u64,
    pub total_rx_dropped: u64,
    pub total_tx_dropped: u64,
    pub tcp_connections: u64,
    pub tcp_segments_sent: u64,
    pub tcp_segments_recv: u64,
    pub tcp_retransmits: u64,
    pub udp_datagrams_sent: u64,
    pub udp_datagrams_recv: u64,
    pub udp_errors: u64,
    pub icmp_msgs_sent: u64,
    pub icmp_msgs_recv: u64,
    pub zero_copy_ops: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

impl NetworkStatsSnapshot {
    /// Calcule le cache hit rate
    pub fn cache_hit_rate(&self) -> f64 {
        let total = self.cache_hits + self.cache_misses;
        if total == 0 {
            return 0.0;
        }
        (self.cache_hits as f64) / (total as f64) * 100.0
    }
    
    /// Calcule le error rate
    pub fn error_rate(&self) -> f64 {
        let total = self.total_rx_packets + self.total_tx_packets;
        let errors = self.total_rx_errors + self.total_tx_errors;
        if total == 0 {
            return 0.0;
        }
        (errors as f64) / (total as f64) * 100.0
    }
    
    /// Calcule le drop rate
    pub fn drop_rate(&self) -> f64 {
        let total = self.total_rx_packets + self.total_tx_packets;
        let drops = self.total_rx_dropped + self.total_tx_dropped;
        if total == 0 {
            return 0.0;
        }
        (drops as f64) / (total as f64) * 100.0
    }
}

/// Stats globales
pub static NETWORK_STATS: NetworkStats = NetworkStats::new();
