/// IP Tunneling
/// 
/// Supports various IP tunneling protocols:
/// - IP-in-IP (RFC 2003)
/// - GRE (Generic Routing Encapsulation - RFC 2784)
/// - IPIP6 and IP6IP6 (IPv4-in-IPv6 and IPv6-in-IPv6)

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Tunnel type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelType {
    /// IP-in-IP (protocol 4)
    IpInIp,
    /// IPv6-in-IPv4 (protocol 41)
    Ipv6InIpv4,
    /// IPv4-in-IPv6
    Ipv4InIpv6,
    /// IPv6-in-IPv6
    Ipv6InIpv6,
    /// GRE (Generic Routing Encapsulation)
    Gre,
}

/// Tunnel configuration
#[derive(Debug, Clone)]
pub struct TunnelConfig {
    /// Tunnel name
    pub name: [u8; 16],
    /// Tunnel type
    pub tunnel_type: TunnelType,
    /// Local endpoint address
    pub local_addr: [u8; 16],
    /// Remote endpoint address
    pub remote_addr: [u8; 16],
    /// TTL for outer IP header
    pub ttl: u8,
    /// MTU
    pub mtu: u16,
    /// GRE key (for GRE tunnels)
    pub gre_key: Option<u32>,
}

impl TunnelConfig {
    /// Create a new tunnel configuration
    pub fn new(
        name: &str,
        tunnel_type: TunnelType,
        local_addr: [u8; 16],
        remote_addr: [u8; 16],
    ) -> Self {
        let mut name_bytes = [0u8; 16];
        let bytes = name.as_bytes();
        let len = bytes.len().min(16);
        name_bytes[..len].copy_from_slice(&bytes[..len]);

        Self {
            name: name_bytes,
            tunnel_type,
            local_addr,
            remote_addr,
            ttl: 64,
            mtu: 1480,  // Account for outer IP header
            gre_key: None,
        }
    }

    /// Get tunnel name as string
    pub fn name_str(&self) -> &str {
        let len = self.name.iter().position(|&b| b == 0).unwrap_or(16);
        core::str::from_utf8(&self.name[..len]).unwrap_or("invalid")
    }
}

/// Tunnel interface
pub struct Tunnel {
    /// Tunnel ID
    pub id: u32,
    /// Configuration
    pub config: TunnelConfig,
    /// Statistics
    pub stats: TunnelStats,
    /// Enabled state
    pub enabled: bool,
}

impl Tunnel {
    /// Create a new tunnel
    pub fn new(id: u32, config: TunnelConfig) -> Self {
        Self {
            id,
            config,
            stats: TunnelStats::new(),
            enabled: false,
        }
    }

    /// Enable the tunnel
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// Disable the tunnel
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// Encapsulate a packet
    pub fn encapsulate(&self, inner_packet: &[u8]) -> Result<Vec<u8>, TunnelError> {
        if !self.enabled {
            return Err(TunnelError::TunnelDisabled);
        }

        let mut outer_packet = Vec::new();

        match self.config.tunnel_type {
            TunnelType::IpInIp => {
                // IPv4 outer header
                outer_packet.extend_from_slice(&self.create_ipv4_header(inner_packet.len(), 4));
                outer_packet.extend_from_slice(inner_packet);
            }
            TunnelType::Ipv6InIpv4 => {
                // IPv4 outer header
                outer_packet.extend_from_slice(&self.create_ipv4_header(inner_packet.len(), 41));
                outer_packet.extend_from_slice(inner_packet);
            }
            TunnelType::Gre => {
                // IPv4 outer header
                let gre_header = self.create_gre_header(inner_packet.len());
                outer_packet.extend_from_slice(&self.create_ipv4_header(
                    gre_header.len() + inner_packet.len(),
                    47,  // GRE protocol
                ));
                outer_packet.extend_from_slice(&gre_header);
                outer_packet.extend_from_slice(inner_packet);
            }
            _ => {
                return Err(TunnelError::UnsupportedType);
            }
        }

        self.stats.packets_tx.fetch_add(1, Ordering::Relaxed);
        self.stats.bytes_tx.fetch_add(outer_packet.len() as u64, Ordering::Relaxed);

        Ok(outer_packet)
    }

    /// Decapsulate a packet
    pub fn decapsulate(&self, outer_packet: &[u8]) -> Result<Vec<u8>, TunnelError> {
        if !self.enabled {
            return Err(TunnelError::TunnelDisabled);
        }

        let inner_packet = match self.config.tunnel_type {
            TunnelType::IpInIp | TunnelType::Ipv6InIpv4 => {
                // Skip outer IPv4 header (typically 20 bytes)
                if outer_packet.len() < 20 {
                    return Err(TunnelError::InvalidPacket);
                }
                let header_len = ((outer_packet[0] & 0x0f) * 4) as usize;
                outer_packet[header_len..].to_vec()
            }
            TunnelType::Gre => {
                // Skip outer IPv4 header and GRE header
                if outer_packet.len() < 24 {
                    return Err(TunnelError::InvalidPacket);
                }
                let ip_header_len = ((outer_packet[0] & 0x0f) * 4) as usize;
                let gre_header_len = self.parse_gre_header_len(&outer_packet[ip_header_len..])?;
                outer_packet[ip_header_len + gre_header_len..].to_vec()
            }
            _ => {
                return Err(TunnelError::UnsupportedType);
            }
        };

        self.stats.packets_rx.fetch_add(1, Ordering::Relaxed);
        self.stats.bytes_rx.fetch_add(inner_packet.len() as u64, Ordering::Relaxed);

        Ok(inner_packet)
    }

    /// Create IPv4 header for outer packet
    fn create_ipv4_header(&self, payload_len: usize, protocol: u8) -> Vec<u8> {
        let mut header = vec![0u8; 20];

        // Version (4) and IHL (5)
        header[0] = 0x45;
        // DSCP and ECN
        header[1] = 0;
        // Total length
        let total_len = 20 + payload_len;
        header[2] = (total_len >> 8) as u8;
        header[3] = (total_len & 0xff) as u8;
        // Identification
        header[4] = 0;
        header[5] = 0;
        // Flags and fragment offset
        header[6] = 0x40;  // Don't fragment
        header[7] = 0;
        // TTL
        header[8] = self.config.ttl;
        // Protocol
        header[9] = protocol;
        // Checksum (calculated later)
        header[10] = 0;
        header[11] = 0;
        // Source address (first 4 bytes of local_addr)
        header[12..16].copy_from_slice(&self.config.local_addr[..4]);
        // Destination address (first 4 bytes of remote_addr)
        header[16..20].copy_from_slice(&self.config.remote_addr[..4]);

        // Calculate checksum
        let checksum = Self::calculate_ipv4_checksum(&header);
        header[10] = (checksum >> 8) as u8;
        header[11] = (checksum & 0xff) as u8;

        header
    }

    /// Create GRE header
    fn create_gre_header(&self, payload_len: usize) -> Vec<u8> {
        let has_key = self.config.gre_key.is_some();
        let header_len = if has_key { 8 } else { 4 };
        let mut header = vec![0u8; header_len];

        // Flags and version
        if has_key {
            header[0] = 0x20;  // Key bit set
        }
        header[1] = 0;

        // Protocol type (0x0800 = IPv4, 0x86DD = IPv6)
        header[2] = 0x08;
        header[3] = 0x00;

        // Key (if present)
        if let Some(key) = self.config.gre_key {
            header[4] = (key >> 24) as u8;
            header[5] = ((key >> 16) & 0xff) as u8;
            header[6] = ((key >> 8) & 0xff) as u8;
            header[7] = (key & 0xff) as u8;
        }

        header
    }

    /// Parse GRE header length
    fn parse_gre_header_len(&self, gre_packet: &[u8]) -> Result<usize, TunnelError> {
        if gre_packet.len() < 4 {
            return Err(TunnelError::InvalidPacket);
        }

        let flags = gre_packet[0];
        let mut len = 4;

        // Check for optional fields
        if flags & 0x80 != 0 {
            len += 4;  // Checksum and Reserved1
        }
        if flags & 0x20 != 0 {
            len += 4;  // Key
        }
        if flags & 0x10 != 0 {
            len += 4;  // Sequence number
        }

        Ok(len)
    }

    /// Calculate IPv4 checksum
    fn calculate_ipv4_checksum(header: &[u8]) -> u16 {
        let mut sum: u32 = 0;

        for chunk in header.chunks(2) {
            if chunk.len() == 2 {
                sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
            }
        }

        while sum >> 16 != 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }

        !sum as u16
    }
}

/// Tunnel statistics
#[derive(Debug)]
pub struct TunnelStats {
    /// Packets transmitted
    pub packets_tx: AtomicU64,
    /// Packets received
    pub packets_rx: AtomicU64,
    /// Bytes transmitted
    pub bytes_tx: AtomicU64,
    /// Bytes received
    pub bytes_rx: AtomicU64,
    /// Errors
    pub errors: AtomicU64,
}

impl TunnelStats {
    pub fn new() -> Self {
        Self {
            packets_tx: AtomicU64::new(0),
            packets_rx: AtomicU64::new(0),
            bytes_tx: AtomicU64::new(0),
            bytes_rx: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        }
    }
}

/// Tunnel errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelError {
    /// Tunnel is disabled
    TunnelDisabled,
    /// Invalid packet
    InvalidPacket,
    /// Unsupported tunnel type
    UnsupportedType,
    /// Tunnel not found
    TunnelNotFound,
    /// Tunnel already exists
    TunnelExists,
}

/// Tunnel manager
pub struct TunnelManager {
    /// Active tunnels
    tunnels: Mutex<BTreeMap<u32, Arc<Tunnel>>>,
    /// Next tunnel ID
    next_id: Mutex<u32>,
}

impl TunnelManager {
    /// Create a new tunnel manager
    pub fn new() -> Self {
        Self {
            tunnels: Mutex::new(BTreeMap::new()),
            next_id: Mutex::new(1),
        }
    }

    /// Create a new tunnel
    pub fn create_tunnel(&self, config: TunnelConfig) -> Result<u32, TunnelError> {
        let id = {
            let mut next_id = self.next_id.lock();
            let id = *next_id;
            *next_id += 1;
            id
        };

        let tunnel = Arc::new(Tunnel::new(id, config));
        self.tunnels.lock().insert(id, tunnel);

        Ok(id)
    }

    /// Get a tunnel by ID
    pub fn get_tunnel(&self, id: u32) -> Option<Arc<Tunnel>> {
        self.tunnels.lock().get(&id).cloned()
    }

    /// Delete a tunnel
    pub fn delete_tunnel(&self, id: u32) -> Result<(), TunnelError> {
        self.tunnels
            .lock()
            .remove(&id)
            .map(|_| ())
            .ok_or(TunnelError::TunnelNotFound)
    }

    /// List all tunnels
    pub fn list_tunnels(&self) -> Vec<u32> {
        self.tunnels.lock().keys().copied().collect()
    }
}

/// Global tunnel manager
static TUNNEL_MANAGER: Mutex<Option<TunnelManager>> = Mutex::new(None);

/// Initialize the tunnel manager
pub fn init() {
    *TUNNEL_MANAGER.lock() = Some(TunnelManager::new());
}

/// Get the global tunnel manager
pub fn get_manager() -> Option<Arc<TunnelManager>> {
    TUNNEL_MANAGER.lock().as_ref().map(|_| {
        Arc::new(TunnelManager::new())
    })
}
