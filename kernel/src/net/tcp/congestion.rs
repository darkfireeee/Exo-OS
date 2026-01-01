//! TCP Congestion Control - CUBIC Algorithm
//!
//! Phase 2d: CUBIC is the default congestion control algorithm in Linux
//!
//! CUBIC features:
//! - Window growth function independent of RTT
//! - Fast convergence for fairness
//! - TCP-friendly for short flows
//! - Scalable for high-speed networks
//!
//! References:
//! - RFC 8312: CUBIC for Fast Long-Distance Networks
//! - Linux kernel: net/ipv4/tcp_cubic.c

use core::cmp::{min, max};
use core::sync::atomic::{AtomicU32, Ordering};

/// CUBIC constants
const BETA_CUBIC: u32 = 717; // β = 0.7 (scaled by 1024)
const C_CUBIC: u32 = 410;     // C = 0.4 (scaled by 1024)
const FAST_CONVERGENCE: bool = true;

/// CUBIC state
pub struct CubicState {
    /// Last maximum congestion window (before loss)
    w_max: AtomicU32,
    
    /// Time of last congestion event (in ms)
    epoch_start: AtomicU32,
    
    /// Window at start of current epoch
    w_last_max: AtomicU32,
    
    /// Current congestion window
    cwnd: AtomicU32,
    
    /// Slow start threshold
    ssthresh: AtomicU32,
    
    /// ACK counter (for counting ACKs)
    ack_count: AtomicU32,
    
    /// RTT minimum (in ms)
    min_rtt: AtomicU32,
    
    /// Hybrid slow start
    hystart_enabled: bool,
}

impl CubicState {
    /// Create new CUBIC state
    pub fn new(initial_cwnd: u32) -> Self {
        Self {
            w_max: AtomicU32::new(0),
            epoch_start: AtomicU32::new(0),
            w_last_max: AtomicU32::new(0),
            cwnd: AtomicU32::new(initial_cwnd),
            ssthresh: AtomicU32::new(u32::MAX),
            ack_count: AtomicU32::new(0),
            min_rtt: AtomicU32::new(u32::MAX),
            hystart_enabled: true,
        }
    }
    
    /// Get current congestion window
    pub fn cwnd(&self) -> u32 {
        self.cwnd.load(Ordering::Relaxed)
    }
    
    /// Get slow start threshold
    pub fn ssthresh(&self) -> u32 {
        self.ssthresh.load(Ordering::Relaxed)
    }
    
    /// Update RTT
    pub fn update_rtt(&self, rtt_ms: u32) {
        let current_min = self.min_rtt.load(Ordering::Relaxed);
        if rtt_ms < current_min {
            self.min_rtt.store(rtt_ms, Ordering::Relaxed);
        }
    }
    
    /// On ACK received (in bytes)
    pub fn on_ack(&self, acked_bytes: u32, rtt_ms: u32) {
        self.update_rtt(rtt_ms);
        
        let cwnd = self.cwnd.load(Ordering::Relaxed);
        let ssthresh = self.ssthresh.load(Ordering::Relaxed);
        
        if cwnd < ssthresh {
            // Slow start: exponential growth
            let new_cwnd = cwnd + acked_bytes;
            self.cwnd.store(new_cwnd, Ordering::Relaxed);
            
            crate::logger::debug(&alloc::format!(
                "[CUBIC] Slow start: cwnd {} -> {} (+{})",
                cwnd,
                new_cwnd,
                acked_bytes
            ));
        } else {
            // Congestion avoidance: CUBIC growth
            let new_cwnd = self.cubic_update(cwnd, rtt_ms);
            self.cwnd.store(new_cwnd, Ordering::Relaxed);
            
            crate::logger::debug(&alloc::format!(
                "[CUBIC] Congestion avoidance: cwnd {} -> {}",
                cwnd,
                new_cwnd
            ));
        }
    }
    
    /// CUBIC window update function
    ///
    /// W_cubic(t) = C * (t - K)^3 + W_max
    /// where K = ∛(W_max * β / C)
    fn cubic_update(&self, cwnd: u32, rtt_ms: u32) -> u32 {
        let w_max = self.w_max.load(Ordering::Relaxed);
        
        if w_max == 0 {
            // First epoch, use linear growth
            return cwnd + 1;
        }
        
        let epoch_start = self.epoch_start.load(Ordering::Relaxed);
        let t = self.get_time_ms().saturating_sub(epoch_start);
        
        // Calculate K = ∛(W_max * β / C)
        // Using integer approximation
        let k = self.cubic_root((w_max * BETA_CUBIC) / C_CUBIC);
        
        // Calculate (t - K)
        let delta_t = if t > k { t - k } else { 0 };
        
        // Calculate (t - K)^3
        let cube = self.cube(delta_t);
        
        // W_cubic(t) = C * (t - K)^3 + W_max
        let w_cubic = (C_CUBIC * cube) / 1024 + w_max;
        
        // TCP-friendly window
        let w_tcp = self.tcp_friendly_window(cwnd, rtt_ms);
        
        // Use max(W_cubic, W_tcp) for fairness
        let target = max(w_cubic, w_tcp);
        
        // Increment cwnd gradually
        if target > cwnd {
            min(cwnd + 1, target)
        } else {
            cwnd
        }
    }
    
    /// TCP-friendly window (for fairness with standard TCP)
    ///
    /// W_tcp = W_max * β + (3 * (1-β) / (1+β)) * (t / RTT)
    fn tcp_friendly_window(&self, cwnd: u32, rtt_ms: u32) -> u32 {
        let w_max = self.w_max.load(Ordering::Relaxed);
        let epoch_start = self.epoch_start.load(Ordering::Relaxed);
        let t = self.get_time_ms().saturating_sub(epoch_start);
        
        if rtt_ms == 0 {
            return cwnd;
        }
        
        // Simplified: W_tcp ≈ W_max * 0.7 + 3 * t / RTT
        let base = (w_max * BETA_CUBIC) / 1024;
        let linear = (3 * t) / rtt_ms;
        
        base + linear
    }
    
    /// On congestion event (packet loss)
    pub fn on_congestion(&self) {
        let cwnd = self.cwnd.load(Ordering::Relaxed);
        let w_max = self.w_max.load(Ordering::Relaxed);
        
        // Fast convergence: reduce W_max if previous W_max was higher
        let new_w_max = if FAST_CONVERGENCE && cwnd < w_max {
            (cwnd * (1024 + BETA_CUBIC)) / (2 * 1024)
        } else {
            cwnd
        };
        
        self.w_max.store(new_w_max, Ordering::Relaxed);
        self.w_last_max.store(cwnd, Ordering::Relaxed);
        
        // Multiplicative decrease: cwnd = β * cwnd
        let new_cwnd = (cwnd * BETA_CUBIC) / 1024;
        let new_ssthresh = max(new_cwnd, 2); // At least 2 MSS
        
        self.cwnd.store(new_cwnd, Ordering::Relaxed);
        self.ssthresh.store(new_ssthresh, Ordering::Relaxed);
        
        // Start new epoch
        self.epoch_start.store(self.get_time_ms(), Ordering::Relaxed);
        
        crate::logger::warn(&alloc::format!(
            "[CUBIC] Congestion: cwnd {} -> {}, ssthresh {}, W_max {}",
            cwnd,
            new_cwnd,
            new_ssthresh,
            new_w_max
        ));
    }
    
    /// On timeout (more severe than loss)
    pub fn on_timeout(&self) {
        let cwnd = self.cwnd.load(Ordering::Relaxed);
        
        self.ssthresh.store(max(cwnd / 2, 2), Ordering::Relaxed);
        self.cwnd.store(1, Ordering::Relaxed); // Reset to 1 MSS
        self.w_max.store(0, Ordering::Relaxed); // Reset CUBIC state
        self.epoch_start.store(self.get_time_ms(), Ordering::Relaxed);
        
        crate::logger::error(&alloc::format!(
            "[CUBIC] Timeout: cwnd reset to 1 (was {})",
            cwnd
        ));
    }
    
    /// Reset state
    pub fn reset(&self, initial_cwnd: u32) {
        self.cwnd.store(initial_cwnd, Ordering::Relaxed);
        self.ssthresh.store(u32::MAX, Ordering::Relaxed);
        self.w_max.store(0, Ordering::Relaxed);
        self.epoch_start.store(0, Ordering::Relaxed);
        self.ack_count.store(0, Ordering::Relaxed);
    }
    
    // Helper functions
    
    /// Integer cube root (approximation)
    fn cubic_root(&self, x: u32) -> u32 {
        if x == 0 {
            return 0;
        }
        
        // Newton's method for cube root
        let mut r = x;
        let mut last_r = 0;
        
        for _ in 0..10 {
            // Avoid oscillation
            if r == last_r {
                break;
            }
            
            last_r = r;
            r = (2 * r + x / (r * r)) / 3;
        }
        
        r
    }
    
    /// Integer cube
    fn cube(&self, x: u32) -> u32 {
        x.saturating_mul(x).saturating_mul(x)
    }
    
    /// Get current time in milliseconds
    fn get_time_ms(&self) -> u32 {
        // Use TSC or system timer
        // For now, approximate using a counter
        static TICK_COUNTER: AtomicU32 = AtomicU32::new(0);
        TICK_COUNTER.fetch_add(1, Ordering::Relaxed)
    }
}

/// Congestion control algorithm trait
pub trait CongestionControl {
    fn on_ack(&mut self, acked_bytes: u32, rtt_ms: u32);
    fn on_loss(&mut self);
    fn on_timeout(&mut self);
    fn cwnd(&self) -> u32;
    fn ssthresh(&self) -> u32;
}

impl CongestionControl for CubicState {
    fn on_ack(&mut self, acked_bytes: u32, rtt_ms: u32) {
        CubicState::on_ack(self, acked_bytes, rtt_ms)
    }
    
    fn on_loss(&mut self) {
        self.on_congestion()
    }
    
    fn on_timeout(&mut self) {
        CubicState::on_timeout(self)
    }
    
    fn cwnd(&self) -> u32 {
        CubicState::cwnd(self)
    }
    
    fn ssthresh(&self) -> u32 {
        CubicState::ssthresh(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_cubic_slow_start() {
        let cubic = CubicState::new(10);
        
        // Initial state
        assert_eq!(cubic.cwnd(), 10);
        assert_eq!(cubic.ssthresh(), u32::MAX);
        
        // Slow start: exponential growth
        cubic.on_ack(10, 10);
        assert_eq!(cubic.cwnd(), 20);
        
        cubic.on_ack(10, 10);
        assert_eq!(cubic.cwnd(), 30);
    }
    
    #[test]
    fn test_cubic_congestion() {
        let cubic = CubicState::new(100);
        cubic.ssthresh.store(50, Ordering::Relaxed); // Force congestion avoidance
        
        let initial_cwnd = cubic.cwnd();
        
        // Congestion event
        cubic.on_congestion();
        
        // Should decrease by β
        let expected = (initial_cwnd * BETA_CUBIC) / 1024;
        assert_eq!(cubic.cwnd(), expected);
        assert!(cubic.w_max.load(Ordering::Relaxed) > 0);
    }
    
    #[test]
    fn test_cubic_timeout() {
        let cubic = CubicState::new(100);
        
        cubic.on_timeout();
        
        // Timeout resets to 1 MSS
        assert_eq!(cubic.cwnd(), 1);
        assert!(cubic.ssthresh() > 0);
    }
}
