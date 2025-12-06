//! # TCP Window Management - Flow Control
//! 
//! Gestion avancée de la fenêtre TCP pour flow control optimal.

use core::sync::atomic::{AtomicU32, Ordering};

/// TCP Window (fenêtre glissante)
pub struct TcpWindow {
    // Send window
    pub snd_una: AtomicU32,  // Oldest unacked sequence
    pub snd_nxt: AtomicU32,  // Next sequence to send
    pub snd_wnd: AtomicU32,  // Send window size (advertised by peer)
    pub snd_wl1: AtomicU32,  // Segment seq for last window update
    pub snd_wl2: AtomicU32,  // Segment ack for last window update
    
    // Receive window
    pub rcv_nxt: AtomicU32,  // Next sequence expected
    pub rcv_wnd: AtomicU32,  // Receive window size (advertise to peer)
    
    // Window scaling (RFC 7323)
    pub snd_wscale: u8,      // Send window scale
    pub rcv_wscale: u8,      // Receive window scale
    
    // Initial sequence numbers
    pub iss: u32,            // Initial send sequence
    pub irs: u32,            // Initial receive sequence
}

impl TcpWindow {
    pub fn new(iss: u32, window_size: u32) -> Self {
        Self {
            snd_una: AtomicU32::new(iss),
            snd_nxt: AtomicU32::new(iss),
            snd_wnd: AtomicU32::new(window_size),
            snd_wl1: AtomicU32::new(0),
            snd_wl2: AtomicU32::new(0),
            rcv_nxt: AtomicU32::new(0),
            rcv_wnd: AtomicU32::new(window_size),
            snd_wscale: 0,
            rcv_wscale: 0,
            iss,
            irs: 0,
        }
    }
    
    /// Combien de bytes peuvent être envoyés?
    pub fn send_available(&self) -> u32 {
        let una = self.snd_una.load(Ordering::Relaxed);
        let nxt = self.snd_nxt.load(Ordering::Relaxed);
        let wnd = self.snd_wnd.load(Ordering::Relaxed);
        
        let in_flight = nxt.wrapping_sub(una);
        wnd.saturating_sub(in_flight)
    }
    
    /// Update send window (appelé quand ACK reçu)
    pub fn update_send_window(&self, seq: u32, ack: u32, window: u16) {
        let wl1 = self.snd_wl1.load(Ordering::Relaxed);
        let wl2 = self.snd_wl2.load(Ordering::Relaxed);
        
        // RFC 793: Update window si:
        // - Segment ACK avance la fenêtre, OU
        // - Même segment mais window plus grande
        if seq.wrapping_sub(wl1) > 0 || 
           (seq == wl1 && ack.wrapping_sub(wl2) >= 0) {
            let scaled_window = (window as u32) << self.snd_wscale;
            self.snd_wnd.store(scaled_window, Ordering::Release);
            self.snd_wl1.store(seq, Ordering::Release);
            self.snd_wl2.store(ack, Ordering::Release);
        }
    }
    
    /// Avance SND.UNA (données ACKées)
    pub fn advance_una(&self, ack: u32) {
        self.snd_una.store(ack, Ordering::Release);
    }
    
    /// Avance SND.NXT (données envoyées)
    pub fn advance_nxt(&self, bytes: u32) {
        self.snd_nxt.fetch_add(bytes, Ordering::Release);
    }
    
    /// Avance RCV.NXT (données reçues)
    pub fn advance_rcv_nxt(&self, bytes: u32) {
        self.rcv_nxt.fetch_add(bytes, Ordering::Release);
    }
    
    /// Calcule window à advertiser au peer
    pub fn advertise_window(&self, buffer_available: u32) -> u16 {
        let window = buffer_available.min(0xFFFF << self.rcv_wscale);
        (window >> self.rcv_wscale) as u16
    }
    
    /// Vérifie si sequence est dans la receive window
    pub fn in_window(&self, seq: u32) -> bool {
        let rcv_nxt = self.rcv_nxt.load(Ordering::Relaxed);
        let rcv_wnd = self.rcv_wnd.load(Ordering::Relaxed);
        
        let offset = seq.wrapping_sub(rcv_nxt);
        offset < rcv_wnd
    }
}

/// Window probe (quand peer window = 0)
pub struct WindowProbe {
    next_probe_time: u64,
    probe_interval: u64,
    max_interval: u64,
}

impl WindowProbe {
    pub fn new() -> Self {
        Self {
            next_probe_time: 0,
            probe_interval: 1_000_000, // 1 second
            max_interval: 60_000_000,  // 60 seconds
        }
    }
    
    pub fn should_probe(&self, now: u64) -> bool {
        now >= self.next_probe_time
    }
    
    pub fn schedule_next(&mut self, now: u64) {
        self.next_probe_time = now + self.probe_interval;
        
        // Exponential backoff
        self.probe_interval = (self.probe_interval * 2).min(self.max_interval);
    }
    
    pub fn reset(&mut self) {
        self.probe_interval = 1_000_000;
        self.next_probe_time = 0;
    }
}

/// Silly Window Syndrome avoidance (RFC 813)
pub struct SillyWindowAvoidance {
    min_send_size: u32,
}

impl SillyWindowAvoidance {
    pub fn new(mss: u32) -> Self {
        Self {
            min_send_size: mss,
        }
    }
    
    /// Doit-on envoyer maintenant?
    pub fn should_send(&self, available: u32, to_send: u32, window: u32) -> bool {
        // Envoie si:
        // 1. On a au moins MSS bytes OU tout ce qui reste
        // 2. Window est >= MSS
        // 3. On peut envoyer au moins 50% de window
        
        (to_send >= self.min_send_size || to_send == available) &&
        (window >= self.min_send_size || window >= available / 2)
    }
}

/// Nagle's Algorithm (RFC 896)
pub struct NagleAlgorithm {
    enabled: bool,
}

impl NagleAlgorithm {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }
    
    /// Doit-on envoyer maintenant?
    pub fn should_send(&self, unacked_data: u32, to_send: u32, mss: u32) -> bool {
        if !self.enabled {
            return true;
        }
        
        // Nagle: Envoie si:
        // - Pas de données non-ACKées (unacked == 0), OU
        // - On a au moins MSS bytes à envoyer
        unacked_data == 0 || to_send >= mss
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_send_available() {
        let window = TcpWindow::new(1000, 65535);
        
        // Au début: tout est disponible
        assert_eq!(window.send_available(), 65535);
        
        // Après avoir envoyé 1000 bytes
        window.advance_nxt(1000);
        assert_eq!(window.send_available(), 64535);
        
        // Après ACK de 500 bytes
        window.advance_una(1500);
        assert_eq!(window.send_available(), 65035);
    }
    
    #[test]
    fn test_window_update() {
        let window = TcpWindow::new(1000, 65535);
        
        window.update_send_window(2000, 1000, 32768);
        assert_eq!(window.snd_wnd.load(Ordering::Relaxed), 32768);
    }
}
