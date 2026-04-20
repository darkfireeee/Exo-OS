//! # net_hooks — Network interception & anomaly detection
//!
//! Monitors network connections, detects port scanning, data exfiltration,
//! and tracks DNS queries. All detection operates on static arrays with
//! no heap allocation.
//!
//! ## Detection algorithms
//! - **Port scan**: tracks unique destination ports per source IP within a
//!   sliding time window; flags when count exceeds threshold.
//! - **Exfiltration**: accumulates outbound byte counts per PID; flags when
//!   volume exceeds threshold within the tracking window.
//! - **DNS**: maintains a ring buffer of recent DNS queries for pattern
//!   analysis (e.g., DNS tunneling via high query rate or long subdomains).

use core::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum concurrent connection tracking entries.
const MAX_CONN_ENTRIES: usize = 256;

/// Maximum per-IP port scan tracking entries.
const MAX_PORT_SCAN_ENTRIES: usize = 64;

/// Maximum unique ports tracked per source IP for port-scan detection.
const MAX_PORTS_PER_SOURCE: usize = 32;

/// Maximum exfiltration tracking entries (per PID).
const MAX_EXFIL_ENTRIES: usize = 64;

/// Maximum DNS query ring buffer entries.
const MAX_DNS_ENTRIES: usize = 256;

/// Maximum recent net events stored.
const MAX_NET_EVENTS: usize = 512;

/// Port scan threshold: unique ports per source within window.
const PORT_SCAN_THRESHOLD: u32 = 16;

/// Exfiltration threshold: bytes per PID within window (~10 MB).
const EXFIL_BYTE_THRESHOLD: u64 = 10_000_000;

/// DNS query rate threshold per PID within window.
const DNS_RATE_THRESHOLD: u32 = 30;

/// Tracking window in TSC ticks (~1 second at 3 GHz).
const TRACKING_WINDOW_TSC: u64 = 3_000_000_000;

// ── TSC read ──────────────────────────────────────────────────────────────────

#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

// ── Types ─────────────────────────────────────────────────────────────────────

/// Network event type discriminator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NetEventType {
    /// Outbound TCP connection attempt.
    ConnectOut = 0,
    /// Inbound TCP connection accepted.
    ConnectIn = 1,
    /// UDP packet sent.
    UdpSend = 2,
    /// UDP packet received.
    UdpRecv = 3,
    /// DNS query issued.
    DnsQuery = 4,
    /// Connection closed.
    Close = 5,
    /// Data transfer (bulk).
    DataTransfer = 6,
}

impl NetEventType {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::ConnectOut,
            1 => Self::ConnectIn,
            2 => Self::UdpSend,
            3 => Self::UdpRecv,
            4 => Self::DnsQuery,
            5 => Self::Close,
            6 => Self::DataTransfer,
            _ => Self::ConnectOut,
        }
    }
}

/// Network event recorded for each relevant network operation.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NetEvent {
    /// PID owning the socket.
    pub pid: u32,
    /// Event type.
    pub event_type: u8,
    /// Protocol: 6=TCP, 17=UDP, 0=other.
    pub protocol: u8,
    /// Padding.
    pub _pad: u16,
    /// Source IP (network byte order).
    pub src_ip: u32,
    /// Destination IP (network byte order).
    pub dst_ip: u32,
    /// Source port.
    pub src_port: u16,
    /// Destination port.
    pub dst_port: u16,
    /// Number of bytes transferred (0 for connect/close).
    pub byte_count: u32,
    /// TSC timestamp.
    pub timestamp: u64,
}

impl Default for NetEvent {
    fn default() -> Self {
        Self {
            pid: 0,
            event_type: 0,
            protocol: 0,
            _pad: 0,
            src_ip: 0,
            dst_ip: 0,
            src_port: 0,
            dst_port: 0,
            byte_count: 0,
            timestamp: 0,
        }
    }
}

/// DNS query tracking entry.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DnsQueryEntry {
    /// PID that issued the query.
    pub pid: u32,
    /// FNV-1a hash of the queried domain name.
    pub domain_hash: u64,
    /// DNS query type (A=1, AAAA=28, MX=15, etc.).
    pub qtype: u16,
    /// Flags: bit 0 = response received, bit 1 = NXDOMAIN.
    pub flags: u8,
    /// Padding.
    pub _pad: u8,
    /// TSC timestamp of the query.
    pub timestamp: u64,
}

impl Default for DnsQueryEntry {
    fn default() -> Self {
        Self {
            pid: 0,
            domain_hash: 0,
            qtype: 0,
            flags: 0,
            _pad: 0,
            timestamp: 0,
        }
    }
}

/// Port scan tracking entry — per source IP.
struct PortScanEntry {
    /// Source IP being tracked.
    src_ip: AtomicU32,
    /// Unique destination ports observed.
    ports: [AtomicU16; MAX_PORTS_PER_SOURCE],
    /// Number of ports currently tracked.
    port_count: AtomicU32,
    /// TSC of the window start.
    window_start: AtomicU64,
    /// Whether this entry has been flagged as a port scan.
    flagged: AtomicU8,
}

impl PortScanEntry {
    const fn new() -> Self {
        Self {
            src_ip: AtomicU32::new(0),
            ports: [const { AtomicU16::new(0) }; MAX_PORTS_PER_SOURCE],
            port_count: AtomicU32::new(0),
            window_start: AtomicU64::new(0),
            flagged: AtomicU8::new(0),
        }
    }
}

/// Exfiltration tracking entry — per PID.
struct ExfilEntry {
    pid: AtomicU32,
    byte_total: AtomicU64,
    window_start: AtomicU64,
    flagged: AtomicU8,
}

impl ExfilEntry {
    const fn new() -> Self {
        Self {
            pid: AtomicU32::new(0),
            byte_total: AtomicU64::new(0),
            window_start: AtomicU64::new(0),
            flagged: AtomicU8::new(0),
        }
    }
}

/// DNS rate tracking entry — per PID.
struct DnsRateEntry {
    pid: AtomicU32,
    count: AtomicU32,
    window_start: AtomicU64,
    flagged: AtomicU8,
}

impl DnsRateEntry {
    const fn new() -> Self {
        Self {
            pid: AtomicU32::new(0),
            count: AtomicU32::new(0),
            window_start: AtomicU64::new(0),
            flagged: AtomicU8::new(0),
        }
    }
}

// ── FNV-1a hash ───────────────────────────────────────────────────────────────

#[inline]
fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01B3);
    }
    hash
}

// ── Static storage ────────────────────────────────────────────────────────────

/// Ring buffer of recent net events.
static NET_EVENTS: Mutex<[NetEvent; MAX_NET_EVENTS]> = Mutex::new(
    [NetEvent::default(); MAX_NET_EVENTS],
);
static NET_EVENT_IDX: AtomicU32 = AtomicU32::new(0);

/// Port scan tracking table.
static PORT_SCAN_TABLE: Mutex<[PortScanEntry; MAX_PORT_SCAN_ENTRIES]> = Mutex::new(
    [PortScanEntry::new(); MAX_PORT_SCAN_ENTRIES],
);

/// Exfiltration tracking table.
static EXFIL_TABLE: Mutex<[ExfilEntry; MAX_EXFIL_ENTRIES]> = Mutex::new(
    [ExfilEntry::new(); MAX_EXFIL_ENTRIES],
);

/// DNS query ring buffer.
static DNS_BUFFER: Mutex<[DnsQueryEntry; MAX_DNS_ENTRIES]> = Mutex::new(
    [DnsQueryEntry::default(); MAX_DNS_ENTRIES],
);
static DNS_BUFFER_IDX: AtomicU32 = AtomicU32::new(0);

/// DNS rate tracking table.
static DNS_RATE_TABLE: Mutex<[DnsRateEntry; MAX_EXFIL_ENTRIES]> = Mutex::new(
    [DnsRateEntry::new(); MAX_EXFIL_ENTRIES],
);

/// Connection tracking table (active connections).
static CONN_TABLE: Mutex<[NetEvent; MAX_CONN_ENTRIES]> = Mutex::new(
    [NetEvent::default(); MAX_CONN_ENTRIES],
);
static CONN_COUNT: AtomicU32 = AtomicU32::new(0);

/// Statistics counters.
static TOTAL_NET_EVENTS: AtomicU64 = AtomicU64::new(0);
static PORT_SCAN_DETECTIONS: AtomicU64 = AtomicU64::new(0);
static EXFIL_DETECTIONS: AtomicU64 = AtomicU64::new(0);
static DNS_ANOMALIES: AtomicU64 = AtomicU64::new(0);
static BLOCKED_CONNECTIONS: AtomicU64 = AtomicU64::new(0);

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Record a net event into the ring buffer.
fn store_net_event(event: NetEvent) {
    let mut events = NET_EVENTS.lock();
    let idx = NET_EVENT_IDX.fetch_add(1, Ordering::AcqRel) as usize % MAX_NET_EVENTS;
    events[idx] = event;
}

/// Track a destination port for a given source IP in the port-scan detector.
/// Returns `true` if a port scan is detected.
fn track_port_for_scan(src_ip: u32, dst_port: u16) -> bool {
    let mut table = PORT_SCAN_TABLE.lock();
    let now = read_tsc();

    // Find existing entry for this source IP
    for i in 0..MAX_PORT_SCAN_ENTRIES {
        let entry_ip = table[i].src_ip.load(Ordering::Acquire);
        if entry_ip == src_ip {
            let start = table[i].window_start.load(Ordering::Acquire);
            // Reset window if expired
            if now.wrapping_sub(start) > TRACKING_WINDOW_TSC {
                table[i].port_count.store(0, Ordering::Release);
                table[i].window_start.store(now, Ordering::Release);
                table[i].flagged.store(0, Ordering::Release);
            }

            // Check if port already recorded
            let count = table[i].port_count.load(Ordering::Acquire) as usize;
            for j in 0..count.min(MAX_PORTS_PER_SOURCE) {
                if table[i].ports[j].load(Ordering::Acquire) == dst_port {
                    // Already seen this port — no new detection
                    return table[i].flagged.load(Ordering::Acquire) != 0;
                }
            }

            // Add new port
            if count < MAX_PORTS_PER_SOURCE {
                table[i].ports[count].store(dst_port, Ordering::Release);
                let new_count = table[i].port_count.fetch_add(1, Ordering::AcqRel) + 1;

                if new_count >= PORT_SCAN_THRESHOLD && table[i].flagged.load(Ordering::Acquire) == 0 {
                    table[i].flagged.store(1, Ordering::Release);
                    PORT_SCAN_DETECTIONS.fetch_add(1, Ordering::Relaxed);
                    return true;
                }
            }
            return table[i].flagged.load(Ordering::Acquire) != 0;
        }
    }

    // No entry found — create one
    for i in 0..MAX_PORT_SCAN_ENTRIES {
        let entry_ip = table[i].src_ip.load(Ordering::Acquire);
        if entry_ip == 0 {
            table[i].src_ip.store(src_ip, Ordering::Release);
            table[i].ports[0].store(dst_port, Ordering::Release);
            table[i].port_count.store(1, Ordering::Release);
            table[i].window_start.store(now, Ordering::Release);
            table[i].flagged.store(0, Ordering::Release);
            return false;
        }
    }

    // Table full — evict the oldest
    let mut oldest_idx = 0usize;
    let mut oldest_tsc = u64::MAX;
    for i in 0..MAX_PORT_SCAN_ENTRIES {
        let start = table[i].window_start.load(Ordering::Acquire);
        if start < oldest_tsc {
            oldest_tsc = start;
            oldest_idx = i;
        }
    }
    table[oldest_idx].src_ip.store(src_ip, Ordering::Release);
    table[oldest_idx].ports[0].store(dst_port, Ordering::Release);
    table[oldest_idx].port_count.store(1, Ordering::Release);
    table[oldest_idx].window_start.store(now, Ordering::Release);
    table[oldest_idx].flagged.store(0, Ordering::Release);
    false
}

/// Track outbound bytes for a PID in the exfiltration detector.
/// Returns `true` if exfiltration is detected.
fn track_exfil_bytes(pid: u32, bytes: u64) -> bool {
    let mut table = EXFIL_TABLE.lock();
    let now = read_tsc();

    for i in 0..MAX_EXFIL_ENTRIES {
        let entry_pid = table[i].pid.load(Ordering::Acquire);
        if entry_pid == pid {
            let start = table[i].window_start.load(Ordering::Acquire);
            if now.wrapping_sub(start) > TRACKING_WINDOW_TSC {
                table[i].byte_total.store(bytes, Ordering::Release);
                table[i].window_start.store(now, Ordering::Release);
                table[i].flagged.store(0, Ordering::Release);
                return false;
            }
            let new_total = table[i].byte_total.fetch_add(bytes, Ordering::AcqRel) + bytes;
            if new_total >= EXFIL_BYTE_THRESHOLD && table[i].flagged.load(Ordering::Acquire) == 0 {
                table[i].flagged.store(1, Ordering::Release);
                EXFIL_DETECTIONS.fetch_add(1, Ordering::Relaxed);
                return true;
            }
            return table[i].flagged.load(Ordering::Acquire) != 0;
        }
    }

    // New entry
    for i in 0..MAX_EXFIL_ENTRIES {
        if table[i].pid.load(Ordering::Acquire) == 0 {
            table[i].pid.store(pid, Ordering::Release);
            table[i].byte_total.store(bytes, Ordering::Release);
            table[i].window_start.store(now, Ordering::Release);
            table[i].flagged.store(0, Ordering::Release);
            return false;
        }
    }

    // Evict oldest
    let mut oldest_idx = 0usize;
    let mut oldest_tsc = u64::MAX;
    for i in 0..MAX_EXFIL_ENTRIES {
        let start = table[i].window_start.load(Ordering::Acquire);
        if start < oldest_tsc {
            oldest_tsc = start;
            oldest_idx = i;
        }
    }
    table[oldest_idx].pid.store(pid, Ordering::Release);
    table[oldest_idx].byte_total.store(bytes, Ordering::Release);
    table[oldest_idx].window_start.store(now, Ordering::Release);
    table[oldest_idx].flagged.store(0, Ordering::Release);
    false
}

/// Track DNS query rate per PID.
/// Returns `true` if the rate exceeds the threshold.
fn track_dns_rate(pid: u32) -> bool {
    let mut table = DNS_RATE_TABLE.lock();
    let now = read_tsc();

    for i in 0..MAX_EXFIL_ENTRIES {
        let entry_pid = table[i].pid.load(Ordering::Acquire);
        if entry_pid == pid {
            let start = table[i].window_start.load(Ordering::Acquire);
            if now.wrapping_sub(start) > TRACKING_WINDOW_TSC {
                table[i].count.store(1, Ordering::Release);
                table[i].window_start.store(now, Ordering::Release);
                table[i].flagged.store(0, Ordering::Release);
                return false;
            }
            let new_count = table[i].count.fetch_add(1, Ordering::AcqRel) + 1;
            if new_count >= DNS_RATE_THRESHOLD && table[i].flagged.load(Ordering::Acquire) == 0 {
                table[i].flagged.store(1, Ordering::Release);
                DNS_ANOMALIES.fetch_add(1, Ordering::Relaxed);
                return true;
            }
            return table[i].flagged.load(Ordering::Acquire) != 0;
        }
    }

    for i in 0..MAX_EXFIL_ENTRIES {
        if table[i].pid.load(Ordering::Acquire) == 0 {
            table[i].pid.store(pid, Ordering::Release);
            table[i].count.store(1, Ordering::Release);
            table[i].window_start.store(now, Ordering::Release);
            table[i].flagged.store(0, Ordering::Release);
            return false;
        }
    }

    let mut oldest_idx = 0usize;
    let mut oldest_tsc = u64::MAX;
    for i in 0..MAX_EXFIL_ENTRIES {
        let start = table[i].window_start.load(Ordering::Acquire);
        if start < oldest_tsc {
            oldest_tsc = start;
            oldest_idx = i;
        }
    }
    table[oldest_idx].pid.store(pid, Ordering::Release);
    table[oldest_idx].count.store(1, Ordering::Release);
    table[oldest_idx].window_start.store(now, Ordering::Release);
    table[oldest_idx].flagged.store(0, Ordering::Release);
    false
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Validates an outbound connection attempt before it is established.
///
/// Checks:
/// 1. Port scan detection for the source IP
/// 2. Exfiltration volume for the PID
/// 3. DNS anomaly state for the PID
///
/// Returns `true` if the connection should be blocked.
pub fn pre_connect_check(
    pid: u32,
    src_ip: u32,
    dst_ip: u32,
    src_port: u16,
    dst_port: u16,
    protocol: u8,
) -> bool {
    let _ = (dst_ip, src_port, protocol);

    // Check port scan
    let scan_detected = track_port_for_scan(src_ip, dst_port);
    if scan_detected {
        BLOCKED_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
        return true;
    }

    // Check exfiltration flag
    {
        let table = EXFIL_TABLE.lock();
        for i in 0..MAX_EXFIL_ENTRIES {
            if table[i].pid.load(Ordering::Acquire) == pid
                && table[i].flagged.load(Ordering::Acquire) != 0
            {
                BLOCKED_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }
    }

    // Check DNS anomaly flag
    {
        let table = DNS_RATE_TABLE.lock();
        for i in 0..MAX_EXFIL_ENTRIES {
            if table[i].pid.load(Ordering::Acquire) == pid
                && table[i].flagged.load(Ordering::Acquire) != 0
            {
                BLOCKED_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }
    }

    false
}

/// Monitors a connection after it has been established.
///
/// Records the connection event, tracks exfiltration volume for outbound
/// data, and maintains the active connection table.
pub fn post_connect_monitor(
    pid: u32,
    src_ip: u32,
    dst_ip: u32,
    src_port: u16,
    dst_port: u16,
    protocol: u8,
    byte_count: u32,
) {
    TOTAL_NET_EVENTS.fetch_add(1, Ordering::Relaxed);

    let event = NetEvent {
        pid,
        event_type: NetEventType::ConnectOut as u8,
        protocol,
        _pad: 0,
        src_ip,
        dst_ip,
        src_port,
        dst_port,
        byte_count,
        timestamp: read_tsc(),
    };

    store_net_event(event);

    // Track exfiltration for outbound data
    if byte_count > 0 {
        track_exfil_bytes(pid, byte_count as u64);
    }

    // Add to active connection table
    {
        let mut conns = CONN_TABLE.lock();
        let count = CONN_COUNT.load(Ordering::Acquire) as usize;
        if count < MAX_CONN_ENTRIES {
            conns[count] = event;
            CONN_COUNT.fetch_add(1, Ordering::Release);
        } else {
            // Find a closed/evictable slot (oldest timestamp)
            let mut oldest_idx = 0usize;
            let mut oldest_tsc = u64::MAX;
            for i in 0..MAX_CONN_ENTRIES {
                if conns[i].timestamp < oldest_tsc {
                    oldest_tsc = conns[i].timestamp;
                    oldest_idx = i;
                }
            }
            conns[oldest_idx] = event;
        }
    }
}

/// Detects whether a source IP is performing a port scan.
///
/// Returns `Some(count)` with the number of unique ports hit if a scan
/// is detected, `None` otherwise.
pub fn detect_port_scan(src_ip: u32) -> Option<u32> {
    let table = PORT_SCAN_TABLE.lock();
    let now = read_tsc();

    for i in 0..MAX_PORT_SCAN_ENTRIES {
        if table[i].src_ip.load(Ordering::Acquire) == src_ip {
            let start = table[i].window_start.load(Ordering::Acquire);
            if now.wrapping_sub(start) > TRACKING_WINDOW_TSC {
                return None;
            }
            let count = table[i].port_count.load(Ordering::Acquire);
            if count >= PORT_SCAN_THRESHOLD {
                return Some(count);
            }
            return None;
        }
    }
    None
}

/// Detects whether a PID is exfiltrating data.
///
/// Returns `Some(byte_total)` if exfiltration is detected, `None` otherwise.
pub fn detect_exfiltration(pid: u32) -> Option<u64> {
    let table = EXFIL_TABLE.lock();
    let now = read_tsc();

    for i in 0..MAX_EXFIL_ENTRIES {
        if table[i].pid.load(Ordering::Acquire) == pid {
            let start = table[i].window_start.load(Ordering::Acquire);
            if now.wrapping_sub(start) > TRACKING_WINDOW_TSC {
                return None;
            }
            let total = table[i].byte_total.load(Ordering::Acquire);
            if total >= EXFIL_BYTE_THRESHOLD {
                return Some(total);
            }
            return None;
        }
    }
    None
}

/// Records a DNS query for tracking and rate analysis.
///
/// Returns `true` if the query rate is anomalous.
pub fn record_dns_query(pid: u32, domain: &[u8], qtype: u16) -> bool {
    let domain_hash = fnv1a_hash(domain);
    let now = read_tsc();

    let entry = DnsQueryEntry {
        pid,
        domain_hash,
        qtype,
        flags: 0,
        _pad: 0,
        timestamp: now,
    };

    {
        let mut dns = DNS_BUFFER.lock();
        let idx = DNS_BUFFER_IDX.fetch_add(1, Ordering::AcqRel) as usize % MAX_DNS_ENTRIES;
        dns[idx] = entry;
    }

    track_dns_rate(pid)
}

/// Queries recent DNS queries for a specific PID.
///
/// Fills `out` with matching entries (most recent first) and returns
/// the number of entries written.
pub fn query_dns_for_pid(pid: u32, out: &mut [DnsQueryEntry]) -> usize {
    let dns = DNS_BUFFER.lock();
    let head = DNS_BUFFER_IDX.load(Ordering::Acquire) as usize;
    let mut written = 0usize;

    for offset in 0..MAX_DNS_ENTRIES {
        let i = (head + MAX_DNS_ENTRIES - 1 - offset) % MAX_DNS_ENTRIES;
        if dns[i].pid == pid && dns[i].timestamp != 0 {
            if written < out.len() {
                out[written] = dns[i];
                written += 1;
            } else {
                break;
            }
        }
    }
    written
}

/// Network subsystem statistics.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NetStats {
    pub total_events: u64,
    pub port_scan_detections: u64,
    pub exfil_detections: u64,
    pub dns_anomalies: u64,
    pub blocked_connections: u64,
    pub active_connections: u32,
}

/// Collects network hook statistics.
pub fn get_net_stats() -> NetStats {
    NetStats {
        total_events: TOTAL_NET_EVENTS.load(Ordering::Relaxed),
        port_scan_detections: PORT_SCAN_DETECTIONS.load(Ordering::Relaxed),
        exfil_detections: EXFIL_DETECTIONS.load(Ordering::Relaxed),
        dns_anomalies: DNS_ANOMALIES.load(Ordering::Relaxed),
        blocked_connections: BLOCKED_CONNECTIONS.load(Ordering::Relaxed),
        active_connections: CONN_COUNT.load(Ordering::Relaxed),
    }
}

/// Removes a connection from the active connection table (on close).
pub fn close_connection(pid: u32, src_ip: u32, dst_ip: u32, src_port: u16, dst_port: u16) {
    let mut conns = CONN_TABLE.lock();
    let count = CONN_COUNT.load(Ordering::Acquire) as usize;
    for i in 0..count.min(MAX_CONN_ENTRIES) {
        if conns[i].pid == pid
            && conns[i].src_ip == src_ip
            && conns[i].dst_ip == dst_ip
            && conns[i].src_port == src_port
            && conns[i].dst_port == dst_port
        {
            // Shift remaining entries down
            for j in i..(count - 1).min(MAX_CONN_ENTRIES - 1) {
                conns[j] = conns[j + 1];
            }
            conns[count.min(MAX_CONN_ENTRIES) - 1] = NetEvent::default();
            CONN_COUNT.fetch_sub(1, Ordering::Release);
            break;
        }
    }

    // Record close event
    TOTAL_NET_EVENTS.fetch_add(1, Ordering::Relaxed);
    let event = NetEvent {
        pid,
        event_type: NetEventType::Close as u8,
        protocol: 6,
        _pad: 0,
        src_ip,
        dst_ip,
        src_port,
        dst_port,
        byte_count: 0,
        timestamp: read_tsc(),
    };
    store_net_event(event);
}

/// Resets the net hook subsystem.
pub fn net_hooks_init() {
    NET_EVENT_IDX.store(0, Ordering::Release);
    DNS_BUFFER_IDX.store(0, Ordering::Release);
    CONN_COUNT.store(0, Ordering::Release);
    TOTAL_NET_EVENTS.store(0, Ordering::Release);
    PORT_SCAN_DETECTIONS.store(0, Ordering::Release);
    EXFIL_DETECTIONS.store(0, Ordering::Release);
    DNS_ANOMALIES.store(0, Ordering::Release);
    BLOCKED_CONNECTIONS.store(0, Ordering::Release);
}
