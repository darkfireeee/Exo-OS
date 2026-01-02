//! ARP Protocol - Address Resolution Protocol
//!
//! Maps IPv4 addresses to MAC addresses

use super::socket::Ipv4Addr;
use super::ethernet::MacAddr;
use alloc::vec::Vec;
use spin::Mutex;

/// ARP packet
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ArpPacket {
    /// Hardware type (1 = Ethernet)
    pub hardware_type: u16,
    
    /// Protocol type (0x0800 = IPv4)
    pub protocol_type: u16,
    
    /// Hardware address length (6 for MAC)
    pub hardware_len: u8,
    
    /// Protocol address length (4 for IPv4)
    pub protocol_len: u8,
    
    /// Operation (1 = request, 2 = reply)
    pub operation: u16,
    
    /// Sender MAC address
    pub sender_mac: [u8; 6],
    
    /// Sender IP address
    pub sender_ip: [u8; 4],
    
    /// Target MAC address
    pub target_mac: [u8; 6],
    
    /// Target IP address
    pub target_ip: [u8; 4],
}

/// ARP operations
pub mod operation {
    pub const REQUEST: u16 = 1;
    pub const REPLY: u16 = 2;
}

impl ArpPacket {
    pub const SIZE: usize = 28;
    
    /// Create ARP request
    pub fn request(sender_mac: [u8; 6], sender_ip: Ipv4Addr, target_ip: Ipv4Addr) -> Self {
        Self {
            hardware_type: 1u16.to_be(),
            protocol_type: 0x0800u16.to_be(),
            hardware_len: 6,
            protocol_len: 4,
            operation: operation::REQUEST.to_be(),
            sender_mac,
            sender_ip: sender_ip.0,
            target_mac: [0, 0, 0, 0, 0, 0],
            target_ip: target_ip.0,
        }
    }
    
    /// Create ARP reply
    pub fn reply(
        sender_mac: [u8; 6],
        sender_ip: Ipv4Addr,
        target_mac: [u8; 6],
        target_ip: Ipv4Addr,
    ) -> Self {
        Self {
            hardware_type: 1u16.to_be(),
            protocol_type: 0x0800u16.to_be(),
            hardware_len: 6,
            protocol_len: 4,
            operation: operation::REPLY.to_be(),
            sender_mac,
            sender_ip: sender_ip.0,
            target_mac,
            target_ip: target_ip.0,
        }
    }
    
    /// Parse from buffer
    pub fn parse(data: &[u8]) -> Result<Self, ArpError> {
        if data.len() < Self::SIZE {
            return Err(ArpError::TooShort);
        }
        
        Ok(Self {
            hardware_type: u16::from_be_bytes([data[0], data[1]]),
            protocol_type: u16::from_be_bytes([data[2], data[3]]),
            hardware_len: data[4],
            protocol_len: data[5],
            operation: u16::from_be_bytes([data[6], data[7]]),
            sender_mac: [data[8], data[9], data[10], data[11], data[12], data[13]],
            sender_ip: [data[14], data[15], data[16], data[17]],
            target_mac: [data[18], data[19], data[20], data[21], data[22], data[23]],
            target_ip: [data[24], data[25], data[26], data[27]],
        })
    }
    
    /// Write to buffer
    pub fn write(&self, buffer: &mut [u8]) -> Result<(), ArpError> {
        if buffer.len() < Self::SIZE {
            return Err(ArpError::BufferTooSmall);
        }
        
        buffer[0..2].copy_from_slice(&self.hardware_type.to_be_bytes());
        buffer[2..4].copy_from_slice(&self.protocol_type.to_be_bytes());
        buffer[4] = self.hardware_len;
        buffer[5] = self.protocol_len;
        buffer[6..8].copy_from_slice(&self.operation.to_be_bytes());
        buffer[8..14].copy_from_slice(&self.sender_mac);
        buffer[14..18].copy_from_slice(&self.sender_ip);
        buffer[18..24].copy_from_slice(&self.target_mac);
        buffer[24..28].copy_from_slice(&self.target_ip);
        
        Ok(())
    }
    
    /// Get operation
    pub fn operation(&self) -> u16 {
        u16::from_be(self.operation)
    }
    
    /// Get sender IP
    pub fn sender_ip(&self) -> Ipv4Addr {
        Ipv4Addr(self.sender_ip)
    }
    
    /// Get target IP
    pub fn target_ip(&self) -> Ipv4Addr {
        Ipv4Addr(self.target_ip)
    }
}

/// ARP cache entry
#[derive(Debug, Clone, Copy)]
pub struct ArpEntry {
    /// IP address
    pub ip: Ipv4Addr,
    
    /// MAC address
    pub mac: [u8; 6],
    
    /// Timestamp (for aging)
    pub timestamp: u64,
}

/// ARP cache
pub struct ArpCache {
    entries: Vec<ArpEntry>,
    max_entries: usize,
}

impl ArpCache {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            max_entries: 256,
        }
    }
    
    /// Initialize cache
    pub fn init(&mut self, capacity: usize) {
        self.max_entries = capacity;
        self.entries = Vec::with_capacity(capacity);
    }
    
    /// Lookup MAC address for IP
    pub fn lookup(&self, ip: Ipv4Addr) -> Option<[u8; 6]> {
        self.entries
            .iter()
            .find(|entry| entry.ip.0 == ip.0)
            .map(|entry| entry.mac)
    }
    
    /// Insert or update entry
    pub fn insert(&mut self, ip: Ipv4Addr, mac: [u8; 6]) {
        // Update if exists
        for entry in self.entries.iter_mut() {
            if entry.ip.0 == ip.0 {
                entry.mac = mac;
                entry.timestamp = current_time();
                return;
            }
        }
        
        // Insert new (evict oldest if full)
        if self.entries.len() >= self.max_entries {
            self.entries.remove(0);
        }
        
        self.entries.push(ArpEntry {
            ip,
            mac,
            timestamp: current_time(),
        });
    }
    
    /// Remove entry
    pub fn remove(&mut self, ip: Ipv4Addr) {
        self.entries.retain(|entry| entry.ip.0 != ip.0);
    }
    
    /// Clear all entries
    pub fn clear(&mut self) {
        self.entries.clear();
    }
    
    /// Get number of entries
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    
    /// Age out old entries (>5 minutes)
    pub fn age_out(&mut self, max_age: u64) {
        let now = current_time();
        self.entries.retain(|entry| now - entry.timestamp < max_age);
    }
}

/// ARP errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArpError {
    TooShort,
    BufferTooSmall,
    InvalidOperation,
    NotFound,
}

/// Global ARP cache
pub static ARP_CACHE: Mutex<ArpCache> = Mutex::new(ArpCache::new());

/// Initialize ARP subsystem
pub fn init() {
    let mut cache = ARP_CACHE.lock();
    cache.init(256);
    
    crate::logger::info("[NET] ARP cache initialized (256 entries)");
}

/// Resolve IP to MAC (blocking)
pub fn resolve(ip: Ipv4Addr) -> Result<[u8; 6], ArpError> {
    // Check cache first
    {
        let cache = ARP_CACHE.lock();
        if let Some(mac) = cache.lookup(ip) {
            return Ok(mac);
        }
    }
    
    // TODO: Send ARP request and wait for reply
    
    Err(ArpError::NotFound)
}

/// Handle incoming ARP packet
pub fn handle_packet(packet: &ArpPacket, our_ip: Ipv4Addr, our_mac: [u8; 6]) {
    match packet.operation() {
        operation::REQUEST => {
            // Update cache with sender info
            {
                let mut cache = ARP_CACHE.lock();
                cache.insert(packet.sender_ip(), packet.sender_mac);
            }
            
            // If request is for us, send reply
            if packet.target_ip().0 == our_ip.0 {
                let reply = ArpPacket::reply(
                    our_mac,
                    our_ip,
                    packet.sender_mac,
                    packet.sender_ip(),
                );
                
                // TODO: Send reply via network device
            }
        }
        
        operation::REPLY => {
            // Update cache with reply info
            let mut cache = ARP_CACHE.lock();
            cache.insert(packet.sender_ip(), packet.sender_mac);
        }
        
        _ => {
            // Unknown operation
        }
    }
}

/// Get current timestamp (stub - replace with actual timer)
fn current_time() -> u64 {
    0 // TODO: Replace with actual timestamp
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_arp_request() {
        let sender_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        let sender_ip = Ipv4Addr::new(192, 168, 1, 1);
        let target_ip = Ipv4Addr::new(192, 168, 1, 2);
        
        let packet = ArpPacket::request(sender_mac, sender_ip, target_ip);
        
        assert_eq!(packet.operation(), operation::REQUEST);
        assert_eq!(packet.sender_ip().0, sender_ip.0);
        assert_eq!(packet.target_ip().0, target_ip.0);
    }
    
    #[test]
    fn test_arp_cache() {
        let mut cache = ArpCache::new();
        cache.init(10);
        
        let ip = Ipv4Addr::new(192, 168, 1, 1);
        let mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
        
        // Insert
        cache.insert(ip, mac);
        assert_eq!(cache.len(), 1);
        
        // Lookup
        let found = cache.lookup(ip);
        assert_eq!(found, Some(mac));
        
        // Remove
        cache.remove(ip);
        assert_eq!(cache.len(), 0);
    }
}
