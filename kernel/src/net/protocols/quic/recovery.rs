//! QUIC Loss Recovery - Reliable Delivery
//!
//! Implements QUIC loss detection and congestion control (RFC 9002).

use alloc::vec::Vec;
use alloc::collections::{BTreeMap, VecDeque};
use crate::sync::SpinLock;
use core::sync::atomic::{AtomicU64, Ordering};

/// Packet information for loss recovery
#[derive(Debug, Clone)]
pub struct SentPacket {
    pub packet_number: u64,
    pub time_sent: u64,      // Microseconds
    pub size: usize,
    pub ack_eliciting: bool,
    pub in_flight: bool,
}

/// Loss recovery state
pub struct LossRecovery {
    /// Sent packets awaiting acknowledgment
    sent_packets: SpinLock<BTreeMap<u64, SentPacket>>,
    
    /// Largest acknowledged packet number
    largest_acked: AtomicU64,
    
    /// RTT (round-trip time) in microseconds
    smoothed_rtt: AtomicU64,
    rtt_variance: AtomicU64,
    min_rtt: AtomicU64,
    
    /// Loss detection timer
    loss_time: AtomicU64,
    
    /// Congestion control
    congestion_window: AtomicU64,   // Bytes
    bytes_in_flight: AtomicU64,
    ssthresh: AtomicU64,            // Slow start threshold
    
    /// Statistics
    stats: RecoveryStats,
}

impl LossRecovery {
    pub fn new() -> Self {
        Self {
            sent_packets: SpinLock::new(BTreeMap::new()),
            largest_acked: AtomicU64::new(0),
            smoothed_rtt: AtomicU64::new(333_000), // 333ms initial
            rtt_variance: AtomicU64::new(166_500), // 166.5ms
            min_rtt: AtomicU64::new(u64::MAX),
            loss_time: AtomicU64::new(0),
            congestion_window: AtomicU64::new(10 * 1200), // 10 packets * 1200 bytes
            bytes_in_flight: AtomicU64::new(0),
            ssthresh: AtomicU64::new(u64::MAX),
            stats: RecoveryStats::default(),
        }
    }
    
    /// Record packet sent
    pub fn on_packet_sent(&self, packet_number: u64, size: usize, ack_eliciting: bool) {
        let now = Self::get_time();
        
        let packet = SentPacket {
            packet_number,
            time_sent: now,
            size,
            ack_eliciting,
            in_flight: true,
        };
        
        let mut sent = self.sent_packets.lock();
        sent.insert(packet_number, packet);
        
        if ack_eliciting {
            self.bytes_in_flight.fetch_add(size as u64, Ordering::Relaxed);
        }
        
        self.stats.packets_sent.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Process ACK frame
    pub fn on_ack_received(&self, acked_ranges: &[(u64, u64)]) {
        let now = Self::get_time();
        let mut sent = self.sent_packets.lock();
        
        for (start, end) in acked_ranges {
            for pn in *start..=*end {
                if let Some(packet) = sent.remove(&pn) {
                    // Update RTT
                    if packet.ack_eliciting {
                        let rtt_sample = now.saturating_sub(packet.time_sent);
                        self.update_rtt(rtt_sample);
                    }
                    
                    // Update bytes in flight
                    if packet.in_flight {
                        self.bytes_in_flight.fetch_sub(packet.size as u64, Ordering::Relaxed);
                    }
                    
                    // Update largest acked
                    self.largest_acked.store(
                        self.largest_acked.load(Ordering::Relaxed).max(pn),
                        Ordering::Relaxed
                    );
                    
                    // Congestion control (on ACK)
                    self.on_packet_acked(packet.size);
                }
            }
        }
        
        // Detect lost packets
        self.detect_lost_packets();
        
        self.stats.acks_received.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Update RTT estimates
    fn update_rtt(&self, latest_rtt: u64) {
        // Update min RTT
        let min_rtt = self.min_rtt.load(Ordering::Relaxed);
        if latest_rtt < min_rtt {
            self.min_rtt.store(latest_rtt, Ordering::Relaxed);
        }
        
        // Update smoothed RTT and variance (RFC 6298)
        let smoothed_rtt = self.smoothed_rtt.load(Ordering::Relaxed);
        let rtt_var = self.rtt_variance.load(Ordering::Relaxed);
        
        let rtt_var_sample = if latest_rtt > smoothed_rtt {
            latest_rtt - smoothed_rtt
        } else {
            smoothed_rtt - latest_rtt
        };
        
        let new_rtt_var = (3 * rtt_var + rtt_var_sample) / 4;
        let new_smoothed_rtt = (7 * smoothed_rtt + latest_rtt) / 8;
        
        self.rtt_variance.store(new_rtt_var, Ordering::Relaxed);
        self.smoothed_rtt.store(new_smoothed_rtt, Ordering::Relaxed);
    }
    
    /// Detect lost packets
    fn detect_lost_packets(&self) {
        let now = Self::get_time();
        let largest_acked = self.largest_acked.load(Ordering::Relaxed);
        let mut sent = self.sent_packets.lock();
        
        let mut lost_packets = Vec::new();
        
        for (&pn, packet) in sent.iter() {
            // Packet is lost if:
            // 1. Older than largest_acked by threshold (3 packets)
            // 2. Or sent long enough ago (time threshold)
            
            let packet_threshold_lost = pn + 3 < largest_acked;
            let time_threshold = self.smoothed_rtt.load(Ordering::Relaxed) * 9 / 8;
            let time_threshold_lost = now.saturating_sub(packet.time_sent) > time_threshold;
            
            if packet_threshold_lost || time_threshold_lost {
                lost_packets.push(pn);
            }
        }
        
        // Remove lost packets
        for pn in lost_packets {
            if let Some(packet) = sent.remove(&pn) {
                if packet.in_flight {
                    self.bytes_in_flight.fetch_sub(packet.size as u64, Ordering::Relaxed);
                }
                
                // Congestion control (on loss)
                self.on_packet_lost(packet.size);
                
                self.stats.packets_lost.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    
    /// Congestion control on packet ACKed
    fn on_packet_acked(&self, size: usize) {
        let cwnd = self.congestion_window.load(Ordering::Relaxed);
        let ssthresh = self.ssthresh.load(Ordering::Relaxed);
        
        if cwnd < ssthresh {
            // Slow start: exponential growth
            self.congestion_window.fetch_add(size as u64, Ordering::Relaxed);
        } else {
            // Congestion avoidance: linear growth
            let increment = (size as u64 * 1200) / cwnd;
            self.congestion_window.fetch_add(increment, Ordering::Relaxed);
        }
    }
    
    /// Congestion control on packet lost
    fn on_packet_lost(&self, size: usize) {
        let cwnd = self.congestion_window.load(Ordering::Relaxed);
        
        // Multiplicative decrease
        let new_ssthresh = cwnd / 2;
        let new_cwnd = new_ssthresh;
        
        self.ssthresh.store(new_ssthresh, Ordering::Relaxed);
        self.congestion_window.store(new_cwnd, Ordering::Relaxed);
    }
    
    /// Check if we can send more data
    pub fn can_send(&self, packet_size: usize) -> bool {
        let cwnd = self.congestion_window.load(Ordering::Relaxed);
        let in_flight = self.bytes_in_flight.load(Ordering::Relaxed);
        
        in_flight + packet_size as u64 <= cwnd
    }
    
    /// Get current RTT
    pub fn rtt(&self) -> u64 {
        self.smoothed_rtt.load(Ordering::Relaxed)
    }
    
    /// Get congestion window
    pub fn congestion_window(&self) -> u64 {
        self.congestion_window.load(Ordering::Relaxed)
    }
    
    /// Get statistics
    pub fn stats(&self) -> RecoveryStats {
        self.stats.clone()
    }
    
    fn get_time() -> u64 {
        // TODO: Get real monotonic time in microseconds
        0
    }
}

/// Recovery statistics
#[derive(Debug, Default, Clone)]
pub struct RecoveryStats {
    pub packets_sent: AtomicU64,
    pub packets_lost: AtomicU64,
    pub acks_received: AtomicU64,
    pub spurious_losses: AtomicU64,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_loss_recovery_basic() {
        let recovery = LossRecovery::new();
        
        // Send packets
        recovery.on_packet_sent(0, 1200, true);
        recovery.on_packet_sent(1, 1200, true);
        recovery.on_packet_sent(2, 1200, true);
        
        assert_eq!(recovery.bytes_in_flight.load(Ordering::Relaxed), 3600);
        
        // ACK packets 0 and 2
        recovery.on_ack_received(&[(0, 0), (2, 2)]);
        
        // Packet 1 should be detected as lost
        let stats = recovery.stats();
        assert!(stats.packets_lost.load(Ordering::Relaxed) >= 1);
    }
    
    #[test]
    fn test_congestion_window_growth() {
        let recovery = LossRecovery::new();
        let initial_cwnd = recovery.congestion_window.load(Ordering::Relaxed);
        
        // ACK packet (slow start)
        recovery.on_packet_acked(1200);
        
        let new_cwnd = recovery.congestion_window.load(Ordering::Relaxed);
        assert!(new_cwnd > initial_cwnd);
    }
}
