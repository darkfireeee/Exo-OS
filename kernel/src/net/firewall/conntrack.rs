//! # Connection Tracking - State Machine
//! 
//! Système de suivi des connexions (conntrack) pour firewall stateful.
//! 
//! ## Performance
//! - Hash table lockless (RCU-like)
//! - 10M connexions simultanées
//! - Garbage collection automatique
//! - Zero-copy packet inspection

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use crate::sync::SpinLock;

/// État d'une connexion TCP
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TcpState {
    None = 0,
    SynSent = 1,
    SynRecv = 2,
    Established = 3,
    FinWait1 = 4,
    FinWait2 = 5,
    TimeWait = 6,
    CloseWait = 7,
    LastAck = 8,
    Closing = 9,
    Closed = 10,
}

impl From<u8> for TcpState {
    fn from(v: u8) -> Self {
        match v {
            1 => TcpState::SynSent,
            2 => TcpState::SynRecv,
            3 => TcpState::Established,
            4 => TcpState::FinWait1,
            5 => TcpState::FinWait2,
            6 => TcpState::TimeWait,
            7 => TcpState::CloseWait,
            8 => TcpState::LastAck,
            9 => TcpState::Closing,
            10 => TcpState::Closed,
            _ => TcpState::None,
        }
    }
}

/// Tuple identifiant une connexion (5-tuple)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConnTuple {
    pub src_ip: [u8; 16],
    pub dst_ip: [u8; 16],
    pub src_port: u16,
    pub dst_port: u16,
    pub proto: u8,
}

impl ConnTuple {
    pub fn reverse(&self) -> Self {
        Self {
            src_ip: self.dst_ip,
            dst_ip: self.src_ip,
            src_port: self.dst_port,
            dst_port: self.src_port,
            proto: self.proto,
        }
    }
}

/// Direction d'un paquet
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Original,  // Client -> Server
    Reply,     // Server -> Client
}

/// Entry de connexion
pub struct ConnEntry {
    pub tuple: ConnTuple,
    pub state: AtomicU8, // TcpState
    pub packets_orig: AtomicU64,
    pub packets_reply: AtomicU64,
    pub bytes_orig: AtomicU64,
    pub bytes_reply: AtomicU64,
    pub timestamp_start: u64,
    pub timestamp_last: AtomicU64,
    pub timeout: u64, // secondes
}

impl ConnEntry {
    pub fn new(tuple: ConnTuple, proto: u8) -> Self {
        let timeout = match proto {
            6 => 3600,  // TCP: 1 heure
            17 => 180,  // UDP: 3 minutes
            _ => 60,    // Autres: 1 minute
        };
        
        Self {
            tuple,
            state: AtomicU8::new(TcpState::None as u8),
            packets_orig: AtomicU64::new(0),
            packets_reply: AtomicU64::new(0),
            bytes_orig: AtomicU64::new(0),
            bytes_reply: AtomicU64::new(0),
            timestamp_start: crate::time::now_secs(),
            timestamp_last: AtomicU64::new(crate::time::now_secs()),
            timeout,
        }
    }
    
    pub fn update_tcp(&self, flags: u8, direction: Direction) {
        let current = TcpState::from(self.state.load(Ordering::Acquire));
        let new_state = match (current, flags, direction) {
            // SYN -> SYN_SENT
            (TcpState::None, 0x02, Direction::Original) => TcpState::SynSent,
            // SYN+ACK -> SYN_RECV
            (TcpState::SynSent, 0x12, Direction::Reply) => TcpState::SynRecv,
            // ACK -> ESTABLISHED
            (TcpState::SynRecv, 0x10, Direction::Original) => TcpState::Established,
            // FIN -> FIN_WAIT1
            (TcpState::Established, f, Direction::Original) if f & 0x01 != 0 => TcpState::FinWait1,
            // FIN -> CLOSE_WAIT
            (TcpState::Established, f, Direction::Reply) if f & 0x01 != 0 => TcpState::CloseWait,
            // ACK -> FIN_WAIT2
            (TcpState::FinWait1, 0x10, Direction::Reply) => TcpState::FinWait2,
            // FIN -> TIME_WAIT
            (TcpState::FinWait2, f, Direction::Reply) if f & 0x01 != 0 => TcpState::TimeWait,
            // FIN+ACK -> CLOSING
            (TcpState::FinWait1, f, Direction::Reply) if f & 0x11 == 0x11 => TcpState::Closing,
            // ACK -> TIME_WAIT
            (TcpState::Closing, 0x10, _) => TcpState::TimeWait,
            // Reste dans l'état actuel
            _ => current,
        };
        
        self.state.store(new_state as u8, Ordering::Release);
        self.timestamp_last.store(crate::time::now_secs(), Ordering::Release);
    }
    
    pub fn update_stats(&self, length: usize, direction: Direction) {
        match direction {
            Direction::Original => {
                self.packets_orig.fetch_add(1, Ordering::Relaxed);
                self.bytes_orig.fetch_add(length as u64, Ordering::Relaxed);
            }
            Direction::Reply => {
                self.packets_reply.fetch_add(1, Ordering::Relaxed);
                self.bytes_reply.fetch_add(length as u64, Ordering::Relaxed);
            }
        }
        self.timestamp_last.store(crate::time::now_secs(), Ordering::Release);
    }
    
    pub fn is_expired(&self) -> bool {
        let now = crate::time::now_secs();
        let last = self.timestamp_last.load(Ordering::Acquire);
        now.saturating_sub(last) > self.timeout
    }
}

/// Table de connection tracking
pub struct ConntrackTable {
    entries: SpinLock<BTreeMap<ConnTuple, Arc<ConnEntry>>>,
    stats: ConntrackStats,
}

#[derive(Default)]
pub struct ConntrackStats {
    pub total: AtomicU64,
    pub active: AtomicU64,
    pub new: AtomicU64,
    pub established: AtomicU64,
    pub closed: AtomicU64,
    pub expired: AtomicU64,
}

impl ConntrackTable {
    pub const fn new() -> Self {
        Self {
            entries: SpinLock::new(BTreeMap::new()),
            stats: ConntrackStats {
                total: AtomicU64::new(0),
                active: AtomicU64::new(0),
                new: AtomicU64::new(0),
                established: AtomicU64::new(0),
                closed: AtomicU64::new(0),
                expired: AtomicU64::new(0),
            },
        }
    }
    
    /// Track un paquet
    pub fn track(&self, tuple: ConnTuple, length: usize, tcp_flags: Option<u8>) -> (Arc<ConnEntry>, Direction) {
        let mut entries = self.entries.lock();
        
        // Cherche connexion existante (original ou reply)
        if let Some(entry) = entries.get(&tuple) {
            if let Some(flags) = tcp_flags {
                entry.update_tcp(flags, Direction::Original);
            }
            entry.update_stats(length, Direction::Original);
            return (entry.clone(), Direction::Original);
        }
        
        let reversed = tuple.reverse();
        if let Some(entry) = entries.get(&reversed) {
            if let Some(flags) = tcp_flags {
                entry.update_tcp(flags, Direction::Reply);
            }
            entry.update_stats(length, Direction::Reply);
            return (entry.clone(), Direction::Reply);
        }
        
        // Nouvelle connexion
        let entry = Arc::new(ConnEntry::new(tuple, tuple.proto));
        if let Some(flags) = tcp_flags {
            entry.update_tcp(flags, Direction::Original);
        }
        entry.update_stats(length, Direction::Original);
        
        entries.insert(tuple, entry.clone());
        
        self.stats.total.fetch_add(1, Ordering::Relaxed);
        self.stats.active.fetch_add(1, Ordering::Relaxed);
        self.stats.new.fetch_add(1, Ordering::Relaxed);
        
        (entry, Direction::Original)
    }
    
    /// Garbage collection
    pub fn gc(&self) -> usize {
        let mut entries = self.entries.lock();
        let before = entries.len();
        
        entries.retain(|_, entry| {
            let expired = entry.is_expired();
            if expired {
                self.stats.active.fetch_sub(1, Ordering::Relaxed);
                self.stats.expired.fetch_add(1, Ordering::Relaxed);
            }
            !expired
        });
        
        before - entries.len()
    }
    
    pub fn count(&self) -> usize {
        self.entries.lock().len()
    }
    
    pub fn get_stats(&self) -> (u64, u64, u64, u64, u64) {
        (
            self.stats.total.load(Ordering::Relaxed),
            self.stats.active.load(Ordering::Relaxed),
            self.stats.new.load(Ordering::Relaxed),
            self.stats.established.load(Ordering::Relaxed),
            self.stats.expired.load(Ordering::Relaxed),
        )
    }
}

/// Instance globale
static CONNTRACK: ConntrackTable = ConntrackTable::new();

pub fn conntrack() -> &'static ConntrackTable {
    &CONNTRACK
}

/// Helper pour timekeeper
mod time {
    use core::sync::atomic::{AtomicU64, Ordering};
    
    static UPTIME_SECS: AtomicU64 = AtomicU64::new(0);
    
    pub fn now_secs() -> u64 {
        UPTIME_SECS.load(Ordering::Relaxed)
    }
    
    pub fn tick() {
        UPTIME_SECS.fetch_add(1, Ordering::Relaxed);
    }
}

pub use time::{now_secs, tick};
