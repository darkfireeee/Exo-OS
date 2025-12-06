//! # TCP Timers
//! 
//! Gestion des timers TCP (retransmission, TIME_WAIT, keepalive)

use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

/// Types de timers TCP
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerType {
    /// Retransmission timer (RTO)
    Retransmit,
    
    /// TIME_WAIT timer (2*MSL)
    TimeWait,
    
    /// Keepalive timer
    Keepalive,
    
    /// Delayed ACK timer
    DelayedAck,
    
    /// Persist timer (zero window probe)
    Persist,
}

/// Retransmission Timer (RFC 6298)
pub struct RetransmitTimer {
    /// RTO actuel (microsecondes)
    rto: AtomicU64,
    
    /// SRTT (Smoothed RTT)
    srtt: AtomicU64,
    
    /// RTTVAR (RTT variance)
    rttvar: AtomicU64,
    
    /// Timestamp du dernier envoi
    last_send: AtomicU64,
    
    /// Nombre de retransmissions
    retransmit_count: AtomicU64,
    
    /// Timer actif?
    active: AtomicBool,
}

impl RetransmitTimer {
    /// RTO initial (1 seconde - RFC 6298)
    const INITIAL_RTO: u64 = 1_000_000;
    
    /// RTO minimum (200ms - Linux)
    const MIN_RTO: u64 = 200_000;
    
    /// RTO maximum (120s)
    const MAX_RTO: u64 = 120_000_000;
    
    /// Alpha pour SRTT (1/8)
    const ALPHA: u64 = 8;
    
    /// Beta pour RTTVAR (1/4)
    const BETA: u64 = 4;
    
    /// K pour RTO (4)
    const K: u64 = 4;
    
    pub fn new() -> Self {
        Self {
            rto: AtomicU64::new(Self::INITIAL_RTO),
            srtt: AtomicU64::new(0),
            rttvar: AtomicU64::new(0),
            last_send: AtomicU64::new(0),
            retransmit_count: AtomicU64::new(0),
            active: AtomicBool::new(false),
        }
    }
    
    /// Démarre le timer
    pub fn start(&self, now: u64) {
        self.last_send.store(now, Ordering::Release);
        self.active.store(true, Ordering::Release);
    }
    
    /// Arrête le timer
    pub fn stop(&self) {
        self.active.store(false, Ordering::Release);
        self.retransmit_count.store(0, Ordering::Release);
    }
    
    /// Reset après ACK
    pub fn reset(&self) {
        self.stop();
        self.retransmit_count.store(0, Ordering::Release);
    }
    
    /// Update RTT (RFC 6298)
    pub fn update_rtt(&self, measured_rtt: u64) {
        let srtt = self.srtt.load(Ordering::Relaxed);
        let rttvar = self.rttvar.load(Ordering::Relaxed);
        
        if srtt == 0 {
            // Première mesure
            self.srtt.store(measured_rtt, Ordering::Release);
            self.rttvar.store(measured_rtt / 2, Ordering::Release);
        } else {
            // RFC 6298:
            // RTTVAR = (1 - beta) * RTTVAR + beta * |SRTT - R'|
            let diff = if srtt > measured_rtt {
                srtt - measured_rtt
            } else {
                measured_rtt - srtt
            };
            
            let new_rttvar = ((Self::BETA - 1) * rttvar + diff) / Self::BETA;
            self.rttvar.store(new_rttvar, Ordering::Release);
            
            // SRTT = (1 - alpha) * SRTT + alpha * R'
            let new_srtt = ((Self::ALPHA - 1) * srtt + measured_rtt) / Self::ALPHA;
            self.srtt.store(new_srtt, Ordering::Release);
        }
        
        // RTO = SRTT + max(G, K*RTTVAR)
        let srtt = self.srtt.load(Ordering::Relaxed);
        let rttvar = self.rttvar.load(Ordering::Relaxed);
        let rto = srtt + Self::K * rttvar;
        
        // Clamp RTO
        let rto = rto.max(Self::MIN_RTO).min(Self::MAX_RTO);
        self.rto.store(rto, Ordering::Release);
    }
    
    /// Vérifie si timeout
    pub fn has_expired(&self, now: u64) -> bool {
        if !self.active.load(Ordering::Acquire) {
            return false;
        }
        
        let last = self.last_send.load(Ordering::Acquire);
        let rto = self.rto.load(Ordering::Relaxed);
        
        now >= last + rto
    }
    
    /// Backoff exponentiel après timeout
    pub fn backoff(&self) {
        let count = self.retransmit_count.fetch_add(1, Ordering::Relaxed);
        
        // Exponentiel: RTO *= 2
        let rto = self.rto.load(Ordering::Relaxed);
        let new_rto = (rto * 2).min(Self::MAX_RTO);
        self.rto.store(new_rto, Ordering::Release);
    }
    
    pub fn rto(&self) -> u64 {
        self.rto.load(Ordering::Relaxed)
    }
    
    pub fn retransmit_count(&self) -> u64 {
        self.retransmit_count.load(Ordering::Relaxed)
    }
}

/// TIME_WAIT Timer (RFC 793)
pub struct TimeWaitTimer {
    /// Timestamp de début
    start_time: AtomicU64,
    
    /// Durée (2*MSL = 60s par défaut)
    duration: AtomicU64,
    
    /// Actif?
    active: AtomicBool,
}

impl TimeWaitTimer {
    /// 2*MSL (Maximum Segment Lifetime)
    const DEFAULT_DURATION: u64 = 60_000_000; // 60 secondes
    
    pub fn new() -> Self {
        Self {
            start_time: AtomicU64::new(0),
            duration: AtomicU64::new(Self::DEFAULT_DURATION),
            active: AtomicBool::new(false),
        }
    }
    
    pub fn start(&self, now: u64) {
        self.start_time.store(now, Ordering::Release);
        self.active.store(true, Ordering::Release);
    }
    
    pub fn stop(&self) {
        self.active.store(false, Ordering::Release);
    }
    
    pub fn has_expired(&self, now: u64) -> bool {
        if !self.active.load(Ordering::Acquire) {
            return false;
        }
        
        let start = self.start_time.load(Ordering::Acquire);
        let duration = self.duration.load(Ordering::Relaxed);
        
        now >= start + duration
    }
}

/// Keepalive Timer (RFC 1122)
pub struct KeepaliveTimer {
    /// Dernier envoi de keepalive
    last_probe: AtomicU64,
    
    /// Intervalle entre probes
    interval: AtomicU64,
    
    /// Nombre de probes envoyés
    probe_count: AtomicU64,
    
    /// Max probes avant fermeture
    max_probes: AtomicU64,
    
    /// Actif?
    active: AtomicBool,
}

impl KeepaliveTimer {
    /// Intervalle par défaut (2 heures)
    const DEFAULT_INTERVAL: u64 = 7200_000_000;
    
    /// Max probes (9 - Linux)
    const DEFAULT_MAX_PROBES: u64 = 9;
    
    pub fn new() -> Self {
        Self {
            last_probe: AtomicU64::new(0),
            interval: AtomicU64::new(Self::DEFAULT_INTERVAL),
            probe_count: AtomicU64::new(0),
            max_probes: AtomicU64::new(Self::DEFAULT_MAX_PROBES),
            active: AtomicBool::new(false),
        }
    }
    
    pub fn enable(&self, now: u64) {
        self.last_probe.store(now, Ordering::Release);
        self.active.store(true, Ordering::Release);
        self.probe_count.store(0, Ordering::Release);
    }
    
    pub fn disable(&self) {
        self.active.store(false, Ordering::Release);
    }
    
    pub fn should_probe(&self, now: u64) -> bool {
        if !self.active.load(Ordering::Acquire) {
            return false;
        }
        
        let last = self.last_probe.load(Ordering::Acquire);
        let interval = self.interval.load(Ordering::Relaxed);
        
        now >= last + interval
    }
    
    pub fn probe_sent(&self, now: u64) {
        self.last_probe.store(now, Ordering::Release);
        self.probe_count.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn reset(&self, now: u64) {
        self.probe_count.store(0, Ordering::Release);
        self.last_probe.store(now, Ordering::Release);
    }
    
    pub fn is_dead(&self) -> bool {
        let count = self.probe_count.load(Ordering::Relaxed);
        let max = self.max_probes.load(Ordering::Relaxed);
        count >= max
    }
}

/// Delayed ACK Timer (RFC 1122)
pub struct DelayedAckTimer {
    /// Timestamp du premier segment non-ACKé
    first_unacked: AtomicU64,
    
    /// Délai (200ms - RFC 1122)
    delay: AtomicU64,
    
    /// Actif?
    active: AtomicBool,
}

impl DelayedAckTimer {
    /// Délai max pour delayed ACK (200ms)
    const DEFAULT_DELAY: u64 = 200_000;
    
    pub fn new() -> Self {
        Self {
            first_unacked: AtomicU64::new(0),
            delay: AtomicU64::new(Self::DEFAULT_DELAY),
            active: AtomicBool::new(false),
        }
    }
    
    pub fn start(&self, now: u64) {
        if !self.active.load(Ordering::Acquire) {
            self.first_unacked.store(now, Ordering::Release);
            self.active.store(true, Ordering::Release);
        }
    }
    
    pub fn stop(&self) {
        self.active.store(false, Ordering::Release);
    }
    
    pub fn should_send_ack(&self, now: u64) -> bool {
        if !self.active.load(Ordering::Acquire) {
            return false;
        }
        
        let first = self.first_unacked.load(Ordering::Acquire);
        let delay = self.delay.load(Ordering::Relaxed);
        
        now >= first + delay
    }
}

/// Gestionnaire de tous les timers TCP
pub struct TcpTimers {
    pub retransmit: RetransmitTimer,
    pub time_wait: TimeWaitTimer,
    pub keepalive: KeepaliveTimer,
    pub delayed_ack: DelayedAckTimer,
}

impl TcpTimers {
    pub fn new() -> Self {
        Self {
            retransmit: RetransmitTimer::new(),
            time_wait: TimeWaitTimer::new(),
            keepalive: KeepaliveTimer::new(),
            delayed_ack: DelayedAckTimer::new(),
        }
    }
    
    /// Vérifie tous les timers et retourne ceux qui ont expiré
    pub fn check_expired(&self, now: u64) -> Vec<TimerType> {
        let mut expired = Vec::new();
        
        if self.retransmit.has_expired(now) {
            expired.push(TimerType::Retransmit);
        }
        
        if self.time_wait.has_expired(now) {
            expired.push(TimerType::TimeWait);
        }
        
        if self.keepalive.should_probe(now) {
            expired.push(TimerType::Keepalive);
        }
        
        if self.delayed_ack.should_send_ack(now) {
            expired.push(TimerType::DelayedAck);
        }
        
        expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_retransmit_timer() {
        let timer = RetransmitTimer::new();
        
        timer.start(0);
        assert!(timer.active.load(Ordering::Relaxed));
        
        // Pas expiré avant RTO
        assert!(!timer.has_expired(500_000));
        
        // Expiré après RTO (1s)
        assert!(timer.has_expired(1_000_001));
        
        // Backoff
        timer.backoff();
        assert_eq!(timer.rto(), 2_000_000);
    }
    
    #[test]
    fn test_rtt_update() {
        let timer = RetransmitTimer::new();
        
        // Première mesure : 100ms
        timer.update_rtt(100_000);
        assert_eq!(timer.srtt.load(Ordering::Relaxed), 100_000);
        
        // Deuxième mesure : 120ms
        timer.update_rtt(120_000);
        let srtt = timer.srtt.load(Ordering::Relaxed);
        assert!(srtt > 100_000 && srtt < 120_000);
    }
    
    #[test]
    fn test_time_wait() {
        let timer = TimeWaitTimer::new();
        
        timer.start(0);
        assert!(!timer.has_expired(30_000_000)); // 30s
        assert!(timer.has_expired(60_000_001));  // >60s
    }
    
    #[test]
    fn test_keepalive() {
        let timer = KeepaliveTimer::new();
        
        timer.enable(0);
        assert!(!timer.should_probe(1_000_000)); // 1s
        
        // Après 2h
        assert!(timer.should_probe(7200_000_001));
        
        timer.probe_sent(7200_000_001);
        assert_eq!(timer.probe_count.load(Ordering::Relaxed), 1);
        
        // Max probes
        for _ in 0..8 {
            timer.probe_sent(0);
        }
        assert!(timer.is_dead());
    }
}
