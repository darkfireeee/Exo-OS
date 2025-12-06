// kernel/src/net/dns.rs - DNS Client Implementation
// Domain Name System (RFC 1035)

use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use spin::RwLock;

// ============================================================================
// DNS Constants
// ============================================================================

const DNS_PORT: u16 = 53;
const DNS_MAX_NAME_LEN: usize = 255;
const DNS_MAX_LABEL_LEN: usize = 63;

// DNS Record Types
pub const DNS_TYPE_A: u16 = 1;      // IPv4 address
pub const DNS_TYPE_NS: u16 = 2;     // Name server
pub const DNS_TYPE_CNAME: u16 = 5;  // Canonical name
pub const DNS_TYPE_SOA: u16 = 6;    // Start of authority
pub const DNS_TYPE_PTR: u16 = 12;   // Pointer record
pub const DNS_TYPE_MX: u16 = 15;    // Mail exchange
pub const DNS_TYPE_TXT: u16 = 16;   // Text record
pub const DNS_TYPE_AAAA: u16 = 28;  // IPv6 address

// DNS Classes
pub const DNS_CLASS_IN: u16 = 1;    // Internet

// DNS Response Codes
const DNS_RCODE_NOERROR: u16 = 0;
const DNS_RCODE_FORMATERR: u16 = 1;
const DNS_RCODE_SERVFAIL: u16 = 2;
const DNS_RCODE_NXDOMAIN: u16 = 3;
const DNS_RCODE_NOTIMP: u16 = 4;
const DNS_RCODE_REFUSED: u16 = 5;

// Cache timeout
const DNS_CACHE_TIMEOUT: u64 = 300_000_000; // 5 minutes in microseconds

// ============================================================================
// DNS Header
// ============================================================================

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DnsHeader {
    pub id: u16,
    pub flags: u16,
    pub qdcount: u16,  // Question count
    pub ancount: u16,  // Answer count
    pub nscount: u16,  // Authority count
    pub arcount: u16,  // Additional count
}

impl DnsHeader {
    pub fn new_query(id: u16) -> Self {
        Self {
            id: id.to_be(),
            flags: 0x0100u16.to_be(), // Standard query, recursion desired
            qdcount: 1u16.to_be(),
            ancount: 0,
            nscount: 0,
            arcount: 0,
        }
    }

    pub fn is_response(&self) -> bool {
        (u16::from_be(self.flags) & 0x8000) != 0
    }

    pub fn rcode(&self) -> u16 {
        u16::from_be(self.flags) & 0x000F
    }

    pub fn question_count(&self) -> u16 {
        u16::from_be(self.qdcount)
    }

    pub fn answer_count(&self) -> u16 {
        u16::from_be(self.ancount)
    }
}

// ============================================================================
// DNS Question
// ============================================================================

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DnsQuestion {
    // Name is encoded separately
    pub qtype: u16,
    pub qclass: u16,
}

// ============================================================================
// DNS Resource Record
// ============================================================================

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DnsResourceRecord {
    // Name is encoded separately
    pub rtype: u16,
    pub rclass: u16,
    pub ttl: u32,
    pub rdlength: u16,
    // RDATA follows
}

// ============================================================================
// DNS Cache Entry
// ============================================================================

#[derive(Debug, Clone)]
struct DnsCacheEntry {
    ip: [u8; 4],
    timestamp: u64,
    ttl: u32,
}

// ============================================================================
// DNS Client
// ============================================================================

pub struct DnsClient {
    dns_servers: RwLock<Vec<[u8; 4]>>,
    query_id: AtomicU16,
    cache: RwLock<BTreeMap<String, DnsCacheEntry>>,
    stats: DnsStats,
}

#[derive(Debug, Default)]
struct DnsStats {
    queries_sent: AtomicU64,
    responses_received: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    errors: AtomicU64,
}

impl DnsClient {
    pub const fn new() -> Self {
        Self {
            dns_servers: RwLock::new(Vec::new()),
            query_id: AtomicU16::new(1),
            cache: RwLock::new(BTreeMap::new()),
            stats: DnsStats {
                queries_sent: AtomicU64::new(0),
                responses_received: AtomicU64::new(0),
                cache_hits: AtomicU64::new(0),
                cache_misses: AtomicU64::new(0),
                errors: AtomicU64::new(0),
            },
        }
    }

    // ========================================================================
    // Configuration
    // ========================================================================

    pub fn add_dns_server(&self, server: [u8; 4]) {
        let mut servers = self.dns_servers.write();
        if !servers.contains(&server) {
            servers.push(server);
            log::info!("[DNS] Added server: {}.{}.{}.{}", server[0], server[1], server[2], server[3]);
        }
    }

    pub fn set_dns_servers(&self, servers: Vec<[u8; 4]>) {
        *self.dns_servers.write() = servers;
        log::info!("[DNS] Set {} DNS servers", servers.len());
    }

    // ========================================================================
    // Query - Resolve hostname to IP
    // ========================================================================

    pub fn resolve(&self, hostname: &str) -> Result<[u8; 4], DnsError> {
        // Check cache first
        if let Some(entry) = self.cache_lookup(hostname) {
            self.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(entry.ip);
        }

        self.stats.cache_misses.fetch_add(1, Ordering::Relaxed);

        // Build query
        let query = self.build_query(hostname, DNS_TYPE_A)?;

        // Send to DNS servers
        let servers = self.dns_servers.read();
        if servers.is_empty() {
            return Err(DnsError::NoServers);
        }

        for &server in servers.iter() {
            // TODO: Send UDP packet to server:53
            // TODO: Wait for response with timeout
            // Pour l'instant, on simule une erreur
            log::debug!("[DNS] Would query {} to resolve {}", format_ip(server), hostname);
        }

        self.stats.queries_sent.fetch_add(1, Ordering::Relaxed);
        Err(DnsError::NotImplemented)
    }

    // ========================================================================
    // Build DNS Query
    // ========================================================================

    fn build_query(&self, hostname: &str, qtype: u16) -> Result<Vec<u8>, DnsError> {
        let id = self.query_id.fetch_add(1, Ordering::Relaxed);
        let header = DnsHeader::new_query(id);

        let mut query = Vec::new();

        // Header
        query.extend_from_slice(unsafe {
            core::slice::from_raw_parts(
                &header as *const _ as *const u8,
                core::mem::size_of::<DnsHeader>()
            )
        });

        // Question: encode domain name
        self.encode_name(&mut query, hostname)?;

        // Question type and class
        query.extend_from_slice(&qtype.to_be_bytes());
        query.extend_from_slice(&DNS_CLASS_IN.to_be_bytes());

        Ok(query)
    }

    // ========================================================================
    // Handle DNS Response
    // ========================================================================

    pub fn handle_response(&self, data: &[u8]) -> Result<Vec<[u8; 4]>, DnsError> {
        if data.len() < core::mem::size_of::<DnsHeader>() {
            return Err(DnsError::PacketTooSmall);
        }

        let header = unsafe {
            &*(data.as_ptr() as *const DnsHeader)
        };

        if !header.is_response() {
            return Err(DnsError::NotResponse);
        }

        let rcode = header.rcode();
        if rcode != DNS_RCODE_NOERROR {
            self.stats.errors.fetch_add(1, Ordering::Relaxed);
            return Err(match rcode {
                DNS_RCODE_NXDOMAIN => DnsError::NxDomain,
                DNS_RCODE_SERVFAIL => DnsError::ServerFailure,
                DNS_RCODE_REFUSED => DnsError::Refused,
                _ => DnsError::OtherError,
            });
        }

        let answer_count = header.answer_count();
        if answer_count == 0 {
            return Err(DnsError::NoAnswers);
        }

        // Skip questions
        let mut offset = core::mem::size_of::<DnsHeader>();
        for _ in 0..header.question_count() {
            offset = self.skip_name(data, offset)?;
            offset += 4; // qtype + qclass
        }

        // Parse answers
        let mut results = Vec::new();
        for _ in 0..answer_count {
            let (name_offset, rr) = self.parse_resource_record(data, offset)?;
            offset = name_offset;

            let rtype = u16::from_be(rr.rtype);
            let rdlength = u16::from_be(rr.rdlength) as usize;

            if rtype == DNS_TYPE_A && rdlength == 4 {
                if offset + 4 <= data.len() {
                    let ip = [data[offset], data[offset + 1], data[offset + 2], data[offset + 3]];
                    results.push(ip);
                    offset += 4;
                }
            } else {
                offset += rdlength;
            }
        }

        self.stats.responses_received.fetch_add(1, Ordering::Relaxed);
        Ok(results)
    }

    // ========================================================================
    // Domain Name Encoding (DNS label format)
    // ========================================================================

    fn encode_name(&self, buffer: &mut Vec<u8>, name: &str) -> Result<(), DnsError> {
        if name.len() > DNS_MAX_NAME_LEN {
            return Err(DnsError::NameTooLong);
        }

        for label in name.split('.') {
            if label.is_empty() || label.len() > DNS_MAX_LABEL_LEN {
                return Err(DnsError::InvalidLabel);
            }

            buffer.push(label.len() as u8);
            buffer.extend_from_slice(label.as_bytes());
        }

        buffer.push(0); // Null terminator
        Ok(())
    }

    fn skip_name(&self, data: &[u8], mut offset: usize) -> Result<usize, DnsError> {
        loop {
            if offset >= data.len() {
                return Err(DnsError::InvalidPacket);
            }

            let len = data[offset];
            if len == 0 {
                return Ok(offset + 1);
            }

            // Compression pointer
            if (len & 0xC0) == 0xC0 {
                return Ok(offset + 2);
            }

            offset += 1 + len as usize;
        }
    }

    fn parse_resource_record(&self, data: &[u8], offset: usize) -> Result<(usize, DnsResourceRecord), DnsError> {
        let offset = self.skip_name(data, offset)?;

        if offset + core::mem::size_of::<DnsResourceRecord>() > data.len() {
            return Err(DnsError::InvalidPacket);
        }

        let rr = unsafe {
            *(data.as_ptr().add(offset) as *const DnsResourceRecord)
        };

        let new_offset = offset + core::mem::size_of::<DnsResourceRecord>();
        Ok((new_offset, rr))
    }

    // ========================================================================
    // DNS Cache
    // ========================================================================

    fn cache_lookup(&self, hostname: &str) -> Option<DnsCacheEntry> {
        let cache = self.cache.read();
        if let Some(entry) = cache.get(hostname) {
            let now = crate::time::monotonic_time();
            let ttl_us = entry.ttl as u64 * 1_000_000;
            
            if now - entry.timestamp < ttl_us.min(DNS_CACHE_TIMEOUT) {
                return Some(entry.clone());
            }
        }
        None
    }

    pub fn cache_insert(&self, hostname: &str, ip: [u8; 4], ttl: u32) {
        let entry = DnsCacheEntry {
            ip,
            timestamp: crate::time::monotonic_time(),
            ttl,
        };

        self.cache.write().insert(hostname.to_string(), entry);
        log::debug!("[DNS] Cached {} -> {}.{}.{}.{} (TTL: {}s)",
            hostname, ip[0], ip[1], ip[2], ip[3], ttl);
    }

    pub fn cache_clear(&self) {
        self.cache.write().clear();
        log::info!("[DNS] Cache cleared");
    }

    // ========================================================================
    // Statistics
    // ========================================================================

    pub fn stats(&self) -> (u64, u64, u64, u64, u64, usize) {
        (
            self.stats.queries_sent.load(Ordering::Relaxed),
            self.stats.responses_received.load(Ordering::Relaxed),
            self.stats.cache_hits.load(Ordering::Relaxed),
            self.stats.cache_misses.load(Ordering::Relaxed),
            self.stats.errors.load(Ordering::Relaxed),
            self.cache.read().len(),
        )
    }
}

// ============================================================================
// Global DNS Client
// ============================================================================

pub static DNS_CLIENT: DnsClient = DnsClient::new();

// ============================================================================
// Convenience Functions
// ============================================================================

pub fn resolve(hostname: &str) -> Result<[u8; 4], DnsError> {
    DNS_CLIENT.resolve(hostname)
}

pub fn add_dns_server(server: [u8; 4]) {
    DNS_CLIENT.add_dns_server(server);
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
pub enum DnsError {
    PacketTooSmall,
    InvalidPacket,
    NameTooLong,
    InvalidLabel,
    NotResponse,
    NxDomain,
    ServerFailure,
    Refused,
    NoAnswers,
    NoServers,
    OtherError,
    NotImplemented,
}
