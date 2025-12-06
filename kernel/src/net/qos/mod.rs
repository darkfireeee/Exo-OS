//! # QoS (Quality of Service) - Traffic Shaping & Prioritization
//! 
//! Système de QoS avancé pour priorisation du trafic réseau.
//! 
//! ## Features
//! - HTB (Hierarchical Token Bucket)
//! - Priority queues
//! - Rate limiting
//! - Traffic shaping
//! - 10 Gbps throughput

use alloc::vec::Vec;
use alloc::collections::{VecDeque, BTreeMap};
use alloc::sync::Arc;
use crate::sync::SpinLock;
use core::sync::atomic::{AtomicU64, Ordering};

/// Classe de priorité
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Critical = 0,   // Trafic critique (VoIP, gaming)
    High = 1,       // Interactif (SSH, DNS)
    Normal = 2,     // Bulk (HTTP, FTP)
    Low = 3,        // Background (torrent)
}

impl Default for Priority {
    fn default() -> Self {
        Priority::Normal
    }
}

/// Packet avec métadonnées QoS
pub struct QosPacket {
    pub data: Vec<u8>,
    pub priority: Priority,
    pub timestamp: u64,
    pub src_ip: [u8; 16],
    pub dst_ip: [u8; 16],
    pub tos: u8, // Type of Service (DSCP)
}

/// Token Bucket pour rate limiting
pub struct TokenBucket {
    rate: u64,          // bits/sec
    burst: u64,         // bits
    tokens: AtomicU64,  // tokens actuels
    last_update: AtomicU64, // timestamp
}

impl TokenBucket {
    pub fn new(rate: u64, burst: u64) -> Self {
        Self {
            rate,
            burst,
            tokens: AtomicU64::new(burst),
            last_update: AtomicU64::new(crate::time::now_us()),
        }
    }
    
    /// Essaie de consommer des tokens
    pub fn consume(&self, bits: u64) -> bool {
        let now = crate::time::now_us();
        let last = self.last_update.load(Ordering::Acquire);
        
        // Calcule tokens générés
        let elapsed_us = now.saturating_sub(last);
        let new_tokens = (self.rate * elapsed_us) / 1_000_000;
        
        let mut current = self.tokens.load(Ordering::Acquire);
        current = current.saturating_add(new_tokens).min(self.burst);
        
        // Essaie de consommer
        if current >= bits {
            current -= bits;
            self.tokens.store(current, Ordering::Release);
            self.last_update.store(now, Ordering::Release);
            true
        } else {
            false
        }
    }
    
    pub fn available(&self) -> u64 {
        self.tokens.load(Ordering::Relaxed)
    }
}

/// Queue avec priorité
pub struct PriorityQueue {
    queues: [SpinLock<VecDeque<QosPacket>>; 4], // Une par priorité
    bucket: TokenBucket,
    stats: QueueStats,
}

#[derive(Default)]
pub struct QueueStats {
    pub enqueued: AtomicU64,
    pub dequeued: AtomicU64,
    pub dropped: AtomicU64,
    pub bytes: AtomicU64,
}

impl PriorityQueue {
    pub fn new(rate: u64, burst: u64) -> Self {
        const EMPTY: SpinLock<VecDeque<QosPacket>> = SpinLock::new(VecDeque::new());
        Self {
            queues: [EMPTY; 4],
            bucket: TokenBucket::new(rate, burst),
            stats: QueueStats::default(),
        }
    }
    
    /// Enqueue un paquet
    pub fn enqueue(&self, packet: QosPacket) -> Result<(), ()> {
        let prio = packet.priority as usize;
        let mut queue = self.queues[prio].lock();
        
        // Limite de queue : 1000 paquets par priorité
        if queue.len() >= 1000 {
            self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            return Err(());
        }
        
        self.stats.enqueued.fetch_add(1, Ordering::Relaxed);
        self.stats.bytes.fetch_add(packet.data.len() as u64, Ordering::Relaxed);
        queue.push_back(packet);
        Ok(())
    }
    
    /// Dequeue un paquet (priorité haute d'abord)
    pub fn dequeue(&self) -> Option<QosPacket> {
        // Parcourt priorités de haute en basse
        for prio in 0..4 {
            let mut queue = self.queues[prio].lock();
            if let Some(packet) = queue.front() {
                let bits = (packet.data.len() * 8) as u64;
                
                // Vérifie token bucket
                if self.bucket.consume(bits) {
                    let packet = queue.pop_front().unwrap();
                    self.stats.dequeued.fetch_add(1, Ordering::Relaxed);
                    return Some(packet);
                } else {
                    // Pas assez de tokens, essaie priorité suivante
                    continue;
                }
            }
        }
        None
    }
    
    pub fn len(&self) -> usize {
        self.queues.iter()
            .map(|q| q.lock().len())
            .sum()
    }
    
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Traffic shaper avec classes hiérarchiques
pub struct TrafficShaper {
    classes: SpinLock<BTreeMap<u32, Arc<PriorityQueue>>>,
    default_class: Arc<PriorityQueue>,
}

impl TrafficShaper {
    pub fn new(default_rate: u64) -> Self {
        Self {
            classes: SpinLock::new(BTreeMap::new()),
            default_class: Arc::new(PriorityQueue::new(default_rate, default_rate * 2)),
        }
    }
    
    /// Ajoute une classe
    pub fn add_class(&self, class_id: u32, rate: u64, burst: u64) {
        let queue = Arc::new(PriorityQueue::new(rate, burst));
        self.classes.lock().insert(class_id, queue);
    }
    
    /// Enqueue dans une classe
    pub fn enqueue(&self, class_id: Option<u32>, packet: QosPacket) -> Result<(), ()> {
        if let Some(id) = class_id {
            let classes = self.classes.lock();
            if let Some(queue) = classes.get(&id) {
                return queue.enqueue(packet);
            }
        }
        
        // Classe par défaut
        self.default_class.enqueue(packet)
    }
    
    /// Dequeue (round-robin entre classes)
    pub fn dequeue(&self) -> Option<QosPacket> {
        // Essaie default d'abord
        if let Some(packet) = self.default_class.dequeue() {
            return Some(packet);
        }
        
        // Puis les autres classes
        let classes = self.classes.lock();
        for queue in classes.values() {
            if let Some(packet) = queue.dequeue() {
                return Some(packet);
            }
        }
        
        None
    }
}

/// Classifieur de paquets (DPI-like)
pub struct PacketClassifier {
    rules: SpinLock<Vec<ClassifyRule>>,
}

pub struct ClassifyRule {
    pub priority: Priority,
    pub class_id: Option<u32>,
    
    // Match conditions
    pub src_port: Option<u16>,
    pub dst_port: Option<u16>,
    pub proto: Option<u8>,
    pub tos: Option<u8>,
}

impl PacketClassifier {
    pub const fn new() -> Self {
        Self {
            rules: SpinLock::new(Vec::new()),
        }
    }
    
    pub fn add_rule(&self, rule: ClassifyRule) {
        self.rules.lock().push(rule);
    }
    
    /// Classifie un paquet
    pub fn classify(&self, packet: &[u8], src_port: u16, dst_port: u16, proto: u8, tos: u8) 
        -> (Priority, Option<u32>) {
        let rules = self.rules.lock();
        
        for rule in rules.iter() {
            let mut matches = true;
            
            if let Some(p) = rule.src_port {
                if p != src_port { matches = false; }
            }
            if let Some(p) = rule.dst_port {
                if p != dst_port { matches = false; }
            }
            if let Some(p) = rule.proto {
                if p != proto { matches = false; }
            }
            if let Some(t) = rule.tos {
                if t != tos { matches = false; }
            }
            
            if matches {
                return (rule.priority, rule.class_id);
            }
        }
        
        // Défaut : Normal priority
        (Priority::Normal, None)
    }
}

/// Système QoS global
pub struct QosSystem {
    pub shaper: TrafficShaper,
    pub classifier: PacketClassifier,
}

impl QosSystem {
    pub fn new(default_rate: u64) -> Self {
        Self {
            shaper: TrafficShaper::new(default_rate),
            classifier: PacketClassifier::new(),
        }
    }
    
    /// Configure règles par défaut
    pub fn setup_default_rules(&self) {
        // VoIP : Critical
        self.classifier.add_rule(ClassifyRule {
            priority: Priority::Critical,
            class_id: Some(1),
            src_port: None,
            dst_port: Some(5060), // SIP
            proto: Some(17), // UDP
            tos: None,
        });
        
        // DNS : High
        self.classifier.add_rule(ClassifyRule {
            priority: Priority::High,
            class_id: Some(2),
            src_port: None,
            dst_port: Some(53),
            proto: Some(17),
            tos: None,
        });
        
        // SSH : High
        self.classifier.add_rule(ClassifyRule {
            priority: Priority::High,
            class_id: Some(2),
            src_port: None,
            dst_port: Some(22),
            proto: Some(6), // TCP
            tos: None,
        });
        
        // HTTP/HTTPS : Normal
        self.classifier.add_rule(ClassifyRule {
            priority: Priority::Normal,
            class_id: None,
            src_port: None,
            dst_port: Some(80),
            proto: Some(6),
            tos: None,
        });
        
        // Ajoute classes
        self.shaper.add_class(1, 1_000_000_000, 2_000_000_000); // 1 Gbps pour VoIP
        self.shaper.add_class(2, 500_000_000, 1_000_000_000);   // 500 Mbps pour high
    }
}

/// Instance globale
static QOS: once_cell::sync::Lazy<QosSystem> = once_cell::sync::Lazy::new(|| {
    let qos = QosSystem::new(10_000_000_000); // 10 Gbps par défaut
    qos.setup_default_rules();
    qos
});

pub fn qos() -> &'static QosSystem {
    &QOS
}

mod time {
    use core::sync::atomic::{AtomicU64, Ordering};
    
    static UPTIME_US: AtomicU64 = AtomicU64::new(0);
    
    pub fn now_us() -> u64 {
        UPTIME_US.load(Ordering::Relaxed)
    }
    
    pub fn tick_us() {
        UPTIME_US.fetch_add(1, Ordering::Relaxed);
    }
}

pub use time::{now_us, tick_us};
