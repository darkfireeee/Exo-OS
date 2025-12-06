//! # Packet Processing Pipeline
//! 
//! Fast path packet processing avec zero-copy

use alloc::vec::Vec;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};
use super::skb::SocketBuffer;
use super::netdev::NetworkDevice;

/// Résultat du traitement d'un paquet
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketAction {
    /// Continuer le traitement
    Continue,
    
    /// Accepter et délivrer
    Accept,
    
    /// Drop le paquet
    Drop,
    
    /// Rediriger vers un autre device
    Redirect(u32), // device index
    
    /// Steal (ownership transféré)
    Stolen,
}

/// Hook pour packet processing
pub trait PacketHook: Send + Sync {
    /// Traite un paquet entrant (RX)
    fn process_rx(&self, skb: &mut SocketBuffer, dev: &NetworkDevice) -> PacketAction;
    
    /// Traite un paquet sortant (TX)
    fn process_tx(&self, skb: &mut SocketBuffer, dev: &NetworkDevice) -> PacketAction;
    
    /// Priorité (0 = plus haute)
    fn priority(&self) -> u32;
}

/// Pipeline de traitement des paquets
pub struct PacketPipeline {
    /// Hooks RX (triés par priorité)
    rx_hooks: Vec<Arc<dyn PacketHook>>,
    
    /// Hooks TX (triés par priorité)
    tx_hooks: Vec<Arc<dyn PacketHook>>,
    
    /// Stats
    stats: PacketStats,
}

impl PacketPipeline {
    pub fn new() -> Self {
        Self {
            rx_hooks: Vec::new(),
            tx_hooks: Vec::new(),
            stats: PacketStats::new(),
        }
    }
    
    /// Enregistre un hook RX
    pub fn register_rx_hook(&mut self, hook: Arc<dyn PacketHook>) {
        self.rx_hooks.push(hook);
        self.rx_hooks.sort_by_key(|h| h.priority());
    }
    
    /// Enregistre un hook TX
    pub fn register_tx_hook(&mut self, hook: Arc<dyn PacketHook>) {
        self.tx_hooks.push(hook);
        self.tx_hooks.sort_by_key(|h| h.priority());
    }
    
    /// Traite un paquet RX
    pub fn process_rx(&self, skb: &mut SocketBuffer, dev: &NetworkDevice) -> PacketAction {
        self.stats.rx_packets.fetch_add(1, Ordering::Relaxed);
        
        for hook in &self.rx_hooks {
            match hook.process_rx(skb, dev) {
                PacketAction::Continue => continue,
                action => {
                    self.update_rx_stats(action);
                    return action;
                }
            }
        }
        
        PacketAction::Accept
    }
    
    /// Traite un paquet TX
    pub fn process_tx(&self, skb: &mut SocketBuffer, dev: &NetworkDevice) -> PacketAction {
        self.stats.tx_packets.fetch_add(1, Ordering::Relaxed);
        
        for hook in &self.tx_hooks {
            match hook.process_tx(skb, dev) {
                PacketAction::Continue => continue,
                action => {
                    self.update_tx_stats(action);
                    return action;
                }
            }
        }
        
        PacketAction::Accept
    }
    
    fn update_rx_stats(&self, action: PacketAction) {
        match action {
            PacketAction::Accept => self.stats.rx_accepted.fetch_add(1, Ordering::Relaxed),
            PacketAction::Drop => self.stats.rx_dropped.fetch_add(1, Ordering::Relaxed),
            PacketAction::Redirect(_) => self.stats.rx_redirected.fetch_add(1, Ordering::Relaxed),
            PacketAction::Stolen => self.stats.rx_stolen.fetch_add(1, Ordering::Relaxed),
            _ => 0,
        };
    }
    
    fn update_tx_stats(&self, action: PacketAction) {
        match action {
            PacketAction::Accept => self.stats.tx_accepted.fetch_add(1, Ordering::Relaxed),
            PacketAction::Drop => self.stats.tx_dropped.fetch_add(1, Ordering::Relaxed),
            PacketAction::Redirect(_) => self.stats.tx_redirected.fetch_add(1, Ordering::Relaxed),
            PacketAction::Stolen => self.stats.tx_stolen.fetch_add(1, Ordering::Relaxed),
            _ => 0,
        };
    }
    
    pub fn stats(&self) -> PacketStatsSnapshot {
        self.stats.snapshot()
    }
}

/// Stats du pipeline
struct PacketStats {
    rx_packets: AtomicU64,
    rx_accepted: AtomicU64,
    rx_dropped: AtomicU64,
    rx_redirected: AtomicU64,
    rx_stolen: AtomicU64,
    
    tx_packets: AtomicU64,
    tx_accepted: AtomicU64,
    tx_dropped: AtomicU64,
    tx_redirected: AtomicU64,
    tx_stolen: AtomicU64,
}

impl PacketStats {
    fn new() -> Self {
        Self {
            rx_packets: AtomicU64::new(0),
            rx_accepted: AtomicU64::new(0),
            rx_dropped: AtomicU64::new(0),
            rx_redirected: AtomicU64::new(0),
            rx_stolen: AtomicU64::new(0),
            tx_packets: AtomicU64::new(0),
            tx_accepted: AtomicU64::new(0),
            tx_dropped: AtomicU64::new(0),
            tx_redirected: AtomicU64::new(0),
            tx_stolen: AtomicU64::new(0),
        }
    }
    
    fn snapshot(&self) -> PacketStatsSnapshot {
        PacketStatsSnapshot {
            rx_packets: self.rx_packets.load(Ordering::Relaxed),
            rx_accepted: self.rx_accepted.load(Ordering::Relaxed),
            rx_dropped: self.rx_dropped.load(Ordering::Relaxed),
            rx_redirected: self.rx_redirected.load(Ordering::Relaxed),
            rx_stolen: self.rx_stolen.load(Ordering::Relaxed),
            tx_packets: self.tx_packets.load(Ordering::Relaxed),
            tx_accepted: self.tx_accepted.load(Ordering::Relaxed),
            tx_dropped: self.tx_dropped.load(Ordering::Relaxed),
            tx_redirected: self.tx_redirected.load(Ordering::Relaxed),
            tx_stolen: self.tx_stolen.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PacketStatsSnapshot {
    pub rx_packets: u64,
    pub rx_accepted: u64,
    pub rx_dropped: u64,
    pub rx_redirected: u64,
    pub rx_stolen: u64,
    pub tx_packets: u64,
    pub tx_accepted: u64,
    pub tx_dropped: u64,
    pub tx_redirected: u64,
    pub tx_stolen: u64,
}

/// Pipeline global
pub static PACKET_PIPELINE: spin::Once<PacketPipeline> = spin::Once::new();

pub fn init() {
    PACKET_PIPELINE.call_once(PacketPipeline::new);
}
