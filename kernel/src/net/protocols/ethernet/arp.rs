// kernel/src/net/arp.rs - Address Resolution Protocol (ARP)
// Production-grade implementation avec cache, timeouts, gratuitous ARP

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::RwLock;

use super::buffer::NetBuffer;
use super::ethernet::EtherType;

// ============================================================================
// ARP Constants
// ============================================================================

const ARP_HARDWARE_ETHERNET: u16 = 1;
const ARP_PROTOCOL_IPV4: u16 = 0x0800;

const ARP_OP_REQUEST: u16 = 1;
const ARP_OP_REPLY: u16 = 2;

// Cache timeouts
const ARP_CACHE_TIMEOUT: u64 = 300_000_000; // 5 minutes in microseconds
const ARP_PENDING_TIMEOUT: u64 = 1_000_000; // 1 second

// ============================================================================
// ARP Packet Structure
// ============================================================================

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ArpPacket {
    pub hardware_type: u16,
    pub protocol_type: u16,
    pub hardware_len: u8,
    pub protocol_len: u8,
    pub operation: u16,
    pub sender_hw: [u8; 6],
    pub sender_proto: [u8; 4],
    pub target_hw: [u8; 6],
    pub target_proto: [u8; 4],
}

impl ArpPacket {
    pub fn new_request(sender_mac: [u8; 6], sender_ip: [u8; 4], target_ip: [u8; 4]) -> Self {
        Self {
            hardware_type: ARP_HARDWARE_ETHERNET.to_be(),
            protocol_type: ARP_PROTOCOL_IPV4.to_be(),
            hardware_len: 6,
            protocol_len: 4,
            operation: ARP_OP_REQUEST.to_be(),
            sender_hw: sender_mac,
            sender_proto: sender_ip,
            target_hw: [0; 6], // Unknown
            target_proto: target_ip,
        }
    }

    pub fn new_reply(sender_mac: [u8; 6], sender_ip: [u8; 4], target_mac: [u8; 6], target_ip: [u8; 4]) -> Self {
        Self {
            hardware_type: ARP_HARDWARE_ETHERNET.to_be(),
            protocol_type: ARP_PROTOCOL_IPV4.to_be(),
            hardware_len: 6,
            protocol_len: 4,
            operation: ARP_OP_REPLY.to_be(),
            sender_hw: sender_mac,
            sender_proto: sender_ip,
            target_hw: target_mac,
            target_proto: target_ip,
        }
    }

    pub fn operation(&self) -> u16 {
        u16::from_be(self.operation)
    }

    pub fn sender_mac(&self) -> [u8; 6] {
        self.sender_hw
    }

    pub fn sender_ip(&self) -> [u8; 4] {
        self.sender_proto
    }

    pub fn target_ip(&self) -> [u8; 4] {
        self.target_proto
    }
}

// ============================================================================
// ARP Cache Entry
// ============================================================================

#[derive(Debug, Clone)]
struct ArpCacheEntry {
    mac: [u8; 6],
    timestamp: u64,
    state: ArpState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArpState {
    Reachable,
    Stale,
    Pending,
}

// ============================================================================
// ARP Cache
// ============================================================================

pub struct ArpCache {
    entries: RwLock<BTreeMap<u32, ArpCacheEntry>>, // IP -> MAC
    pending_requests: RwLock<BTreeMap<u32, u64>>,  // IP -> timestamp
    stats: ArpStats,
}

#[derive(Debug, Default)]
struct ArpStats {
    requests_sent: AtomicU64,
    replies_sent: AtomicU64,
    requests_received: AtomicU64,
    replies_received: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    timeouts: AtomicU64,
}

impl ArpCache {
    pub const fn new() -> Self {
        Self {
            entries: RwLock::new(BTreeMap::new()),
            pending_requests: RwLock::new(BTreeMap::new()),
            stats: ArpStats {
                requests_sent: AtomicU64::new(0),
                replies_sent: AtomicU64::new(0),
                requests_received: AtomicU64::new(0),
                replies_received: AtomicU64::new(0),
                cache_hits: AtomicU64::new(0),
                cache_misses: AtomicU64::new(0),
                timeouts: AtomicU64::new(0),
            },
        }
    }

    // ========================================================================
    // Lookup
    // ========================================================================

    pub fn lookup(&self, ip: [u8; 4]) -> Option<[u8; 6]> {
        let ip_u32 = u32::from_be_bytes(ip);
        let entries = self.entries.read();
        
        if let Some(entry) = entries.get(&ip_u32) {
            let now = crate::time::monotonic_time();
            
            // Vérifier si l'entrée n'est pas expirée
            if now - entry.timestamp < ARP_CACHE_TIMEOUT {
                self.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Some(entry.mac);
            } else {
                // Entrée expirée
                drop(entries);
                self.remove(ip);
                self.stats.timeouts.fetch_add(1, Ordering::Relaxed);
            }
        }
        
        self.stats.cache_misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    // ========================================================================
    // Insert
    // ========================================================================

    pub fn insert(&self, ip: [u8; 4], mac: [u8; 6]) {
        let ip_u32 = u32::from_be_bytes(ip);
        let now = crate::time::monotonic_time();

        let entry = ArpCacheEntry {
            mac,
            timestamp: now,
            state: ArpState::Reachable,
        };

        self.entries.write().insert(ip_u32, entry);
        
        // Retirer des pending requests
        self.pending_requests.write().remove(&ip_u32);

        log::debug!("[ARP] Cached {} -> {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            format_ip(ip), mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
    }

    // ========================================================================
    // Remove
    // ========================================================================

    pub fn remove(&self, ip: [u8; 4]) {
        let ip_u32 = u32::from_be_bytes(ip);
        self.entries.write().remove(&ip_u32);
        log::debug!("[ARP] Removed cache entry for {}", format_ip(ip));
    }

    // ========================================================================
    // Pending Requests
    // ========================================================================

    pub fn is_pending(&self, ip: [u8; 4]) -> bool {
        let ip_u32 = u32::from_be_bytes(ip);
        let pending = self.pending_requests.read();
        
        if let Some(&timestamp) = pending.get(&ip_u32) {
            let now = crate::time::monotonic_time();
            if now - timestamp < ARP_PENDING_TIMEOUT {
                return true; // Still pending
            } else {
                // Timeout
                drop(pending);
                self.pending_requests.write().remove(&ip_u32);
                self.stats.timeouts.fetch_add(1, Ordering::Relaxed);
            }
        }
        
        false
    }

    pub fn mark_pending(&self, ip: [u8; 4]) {
        let ip_u32 = u32::from_be_bytes(ip);
        let now = crate::time::monotonic_time();
        self.pending_requests.write().insert(ip_u32, now);
    }

    // ========================================================================
    // Cleanup - Remove expired entries
    // ========================================================================

    pub fn cleanup(&self) {
        let now = crate::time::monotonic_time();
        let mut entries = self.entries.write();
        
        entries.retain(|_, entry| {
            now - entry.timestamp < ARP_CACHE_TIMEOUT
        });
    }

    // ========================================================================
    // Statistics
    // ========================================================================

    pub fn stats(&self) -> (u64, u64, u64, u64, u64, u64) {
        (
            self.stats.requests_sent.load(Ordering::Relaxed),
            self.stats.replies_sent.load(Ordering::Relaxed),
            self.stats.cache_hits.load(Ordering::Relaxed),
            self.stats.cache_misses.load(Ordering::Relaxed),
            self.stats.timeouts.load(Ordering::Relaxed),
            self.entries.read().len() as u64,
        )
    }

    pub fn record_request_sent(&self) {
        self.stats.requests_sent.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_reply_sent(&self) {
        self.stats.replies_sent.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_request_received(&self) {
        self.stats.requests_received.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_reply_received(&self) {
        self.stats.replies_received.fetch_add(1, Ordering::Relaxed);
    }
}

// ============================================================================
// Global ARP Cache
// ============================================================================

pub static ARP_CACHE: ArpCache = ArpCache::new();

// ============================================================================
// ARP Protocol Handler
// ============================================================================

pub struct ArpHandler {
    local_mac: [u8; 6],
    local_ip: [u8; 4],
}

impl ArpHandler {
    pub fn new(local_mac: [u8; 6], local_ip: [u8; 4]) -> Self {
        Self {
            local_mac,
            local_ip,
        }
    }

    // ========================================================================
    // Send ARP Request
    // ========================================================================

    pub fn send_request(&self, target_ip: [u8; 4]) -> Result<Vec<u8>, ArpError> {
        // Vérifier si déjà pending
        if ARP_CACHE.is_pending(target_ip) {
            return Err(ArpError::AlreadyPending);
        }

        let arp_packet = ArpPacket::new_request(self.local_mac, self.local_ip, target_ip);
        
        // Créer paquet Ethernet
        let mut packet = Vec::new();
        
        // Ethernet header (broadcast)
        packet.extend_from_slice(&[0xFF; 6]); // Destination MAC (broadcast)
        packet.extend_from_slice(&self.local_mac); // Source MAC
        packet.extend_from_slice(&EtherType::Arp.to_be_bytes()); // EtherType
        
        // ARP packet
        packet.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &arp_packet as *const _ as *const u8,
                core::mem::size_of::<ArpPacket>()
            )
        });

        ARP_CACHE.mark_pending(target_ip);
        ARP_CACHE.record_request_sent();

        log::debug!("[ARP] Sent request for {}", format_ip(target_ip));
        Ok(packet)
    }

    // ========================================================================
    // Send ARP Reply
    // ========================================================================

    pub fn send_reply(&self, target_mac: [u8; 6], target_ip: [u8; 4]) -> Result<Vec<u8>, ArpError> {
        let arp_packet = ArpPacket::new_reply(self.local_mac, self.local_ip, target_mac, target_ip);
        
        let mut packet = Vec::new();
        
        // Ethernet header
        packet.extend_from_slice(&target_mac); // Destination MAC
        packet.extend_from_slice(&self.local_mac); // Source MAC
        packet.extend_from_slice(&EtherType::Arp.to_be_bytes());
        
        // ARP packet
        packet.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &arp_packet as *const _ as *const u8,
                core::mem::size_of::<ArpPacket>()
            )
        });

        ARP_CACHE.record_reply_sent();

        log::debug!("[ARP] Sent reply to {}", format_ip(target_ip));
        Ok(packet)
    }

    // ========================================================================
    // Handle Incoming ARP Packet
    // ========================================================================

    pub fn handle_packet(&self, data: &[u8]) -> Result<Option<Vec<u8>>, ArpError> {
        if data.len() < core::mem::size_of::<ArpPacket>() {
            return Err(ArpError::PacketTooSmall);
        }

        let arp_packet = unsafe {
            &*(data.as_ptr() as *const ArpPacket)
        };

        let op = arp_packet.operation();
        let sender_ip = arp_packet.sender_ip();
        let sender_mac = arp_packet.sender_mac();
        let target_ip = arp_packet.target_ip();

        // Mettre en cache l'entrée (apprendre des requêtes ET réponses)
        ARP_CACHE.insert(sender_ip, sender_mac);

        match op {
            ARP_OP_REQUEST => {
                ARP_CACHE.record_request_received();
                
                // Est-ce pour nous ?
                if target_ip == self.local_ip {
                    log::debug!("[ARP] Received request from {} for {}",
                        format_ip(sender_ip), format_ip(target_ip));
                    
                    // Envoyer une réponse
                    let reply = self.send_reply(sender_mac, sender_ip)?;
                    return Ok(Some(reply));
                }
            }
            ARP_OP_REPLY => {
                ARP_CACHE.record_reply_received();
                
                log::debug!("[ARP] Received reply from {} (MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x})",
                    format_ip(sender_ip),
                    sender_mac[0], sender_mac[1], sender_mac[2],
                    sender_mac[3], sender_mac[4], sender_mac[5]);
            }
            _ => {
                log::warn!("[ARP] Unknown operation: {}", op);
                return Err(ArpError::UnknownOperation);
            }
        }

        Ok(None)
    }

    // ========================================================================
    // Send Gratuitous ARP
    // ========================================================================

    pub fn send_gratuitous(&self) -> Result<Vec<u8>, ArpError> {
        // Gratuitous ARP = announce notre IP/MAC sans être sollicité
        let arp_packet = ArpPacket::new_request(self.local_mac, self.local_ip, self.local_ip);
        
        let mut packet = Vec::new();
        packet.extend_from_slice(&[0xFF; 6]); // Broadcast
        packet.extend_from_slice(&self.local_mac);
        packet.extend_from_slice(&EtherType::Arp.to_be_bytes());
        packet.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &arp_packet as *const _ as *const u8,
                core::mem::size_of::<ArpPacket>()
            )
        });

        log::info!("[ARP] Sent gratuitous ARP for {}", format_ip(self.local_ip));
        Ok(packet)
    }
}

// ============================================================================
// Utilities
// ============================================================================

fn format_ip(ip: [u8; 4]) -> alloc::string::String {
    alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArpError {
    PacketTooSmall,
    UnknownOperation,
    AlreadyPending,
    Timeout,
}
