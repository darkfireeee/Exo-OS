//! Traffic analysis for the exo_shield security server.
//!
//! Provides packet counting, flow tracking (max 64 flows), statistical
//! analysis, and burst detection — all `no_std` compatible.

use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of tracked flows.
pub const MAX_FLOWS: usize = 64;

/// Burst detection threshold (packets per measurement window).
pub const BURST_THRESHOLD: u64 = 1000;

/// Measurement window size in ticks.
pub const BURST_WINDOW_TICKS: u64 = 100;

// ---------------------------------------------------------------------------
// Flow key — 5-tuple
// ---------------------------------------------------------------------------

/// A network flow identified by the standard 5-tuple.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct FlowKey {
    src_ip: u32,
    dst_ip: u32,
    src_port: u16,
    dst_port: u16,
    protocol: u8,
}

impl FlowKey {
    pub const fn new(
        src_ip: u32,
        dst_ip: u32,
        src_port: u16,
        dst_port: u16,
        protocol: u8,
    ) -> Self {
        Self {
            src_ip,
            dst_ip,
            src_port,
            dst_port,
            protocol,
        }
    }

    /// Simple hash for indexing into the flow table.
    pub fn hash(&self) -> u64 {
        let mut h: u64 = 0x5175_6E65_7400_0000; // magic seed
        h ^= self.src_ip as u64;
        h = h.wrapping_mul(0x5BD1_E995);
        h ^= self.dst_ip as u64;
        h = h.wrapping_mul(0x5BD1_E995);
        h ^= ((self.src_port as u64) << 16) | self.dst_port as u64;
        h = h.wrapping_mul(0x5BD1_E995);
        h ^= self.protocol as u64;
        h
    }

    pub fn src_ip(&self) -> u32 { self.src_ip }
    pub fn dst_ip(&self) -> u32 { self.dst_ip }
    pub fn src_port(&self) -> u16 { self.src_port }
    pub fn dst_port(&self) -> u16 { self.dst_port }
    pub fn protocol(&self) -> u8 { self.protocol }
}

// ---------------------------------------------------------------------------
// Flow entry
// ---------------------------------------------------------------------------

/// Tracked state for a single network flow.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct FlowEntry {
    /// Flow identification key.
    key: FlowKey,
    /// Total packets in this flow.
    packet_count: u64,
    /// Total bytes in this flow.
    byte_count: u64,
    /// First-seen timestamp.
    first_seen: u64,
    /// Last-seen timestamp.
    last_seen: u64,
    /// Packets in the current burst window.
    burst_window_count: u64,
    /// Start of the current burst window.
    burst_window_start: u64,
    /// Whether a burst has been detected for this flow.
    burst_detected: bool,
    /// Whether this entry is in use.
    active: bool,
}

impl FlowEntry {
    pub const fn empty() -> Self {
        Self {
            key: FlowKey::new(0, 0, 0, 0, 0),
            packet_count: 0,
            byte_count: 0,
            first_seen: 0,
            last_seen: 0,
            burst_window_count: 0,
            burst_window_start: 0,
            burst_detected: false,
            active: false,
        }
    }

    pub fn key(&self) -> &FlowKey { &self.key }
    pub fn packet_count(&self) -> u64 { self.packet_count }
    pub fn byte_count(&self) -> u64 { self.byte_count }
    pub fn first_seen(&self) -> u64 { self.first_seen }
    pub fn last_seen(&self) -> u64 { self.last_seen }
    pub fn burst_detected(&self) -> bool { self.burst_detected }
    pub fn is_active(&self) -> bool { self.active }

    /// Flow duration in ticks.
    pub fn duration(&self) -> u64 {
        self.last_seen.saturating_sub(self.first_seen)
    }

    /// Average packet size (0 if no packets).
    pub fn avg_packet_size(&self) -> u64 {
        if self.packet_count == 0 {
            0
        } else {
            self.byte_count / self.packet_count
        }
    }

    /// Packets per tick (0 if duration is 0).
    pub fn packets_per_tick(&self) -> u64 {
        let dur = self.duration();
        if dur == 0 {
            0
        } else {
            self.packet_count / dur
        }
    }
}

// ---------------------------------------------------------------------------
// Traffic analyzer
// ---------------------------------------------------------------------------

/// Global traffic statistics.
#[derive(Debug)]
#[repr(C)]
pub struct TrafficStats {
    /// Total packets seen.
    total_packets: AtomicU64,
    /// Total bytes seen.
    total_bytes: AtomicU64,
    /// Packets per protocol (index = protocol number, max 256).
    proto_packets: [AtomicU64; 8], // TCP, UDP, ICMP, + 5 others
    /// Bytes per protocol.
    proto_bytes: [AtomicU64; 8],
}

impl TrafficStats {
    pub const fn new() -> Self {
        Self {
            total_packets: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            proto_packets: [
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0),
            ],
            proto_bytes: [
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0),
            ],
        }
    }

    /// Map a protocol number to a slot index (0–7).
    fn proto_slot(proto: u8) -> usize {
        match proto {
            6 => 0,  // TCP
            17 => 1, // UDP
            1 => 2,  // ICMP
            _ => 3,  // Other
        }
    }

    fn record(&self, bytes: u64, proto: u8) {
        self.total_packets.fetch_add(1, Ordering::Relaxed);
        self.total_bytes.fetch_add(bytes, Ordering::Relaxed);
        let slot = Self::proto_slot(proto);
        self.proto_packets[slot].fetch_add(1, Ordering::Relaxed);
        self.proto_bytes[slot].fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn total_packets(&self) -> u64 { self.total_packets.load(Ordering::Relaxed) }
    pub fn total_bytes(&self) -> u64 { self.total_bytes.load(Ordering::Relaxed) }
    pub fn tcp_packets(&self) -> u64 { self.proto_packets[0].load(Ordering::Relaxed) }
    pub fn udp_packets(&self) -> u64 { self.proto_packets[1].load(Ordering::Relaxed) }
    pub fn icmp_packets(&self) -> u64 { self.proto_packets[2].load(Ordering::Relaxed) }
}

/// The main traffic analysis engine.
pub struct TrafficAnalyzer {
    /// Flow table.
    flows: [FlowEntry; MAX_FLOWS],
    /// Number of active flows.
    flow_count: u32,
    /// Global traffic statistics.
    stats: TrafficStats,
    /// Burst detection counter (current window).
    burst_window_packets: AtomicU64,
    /// Burst window start.
    burst_window_start: AtomicU64,
    /// Whether a global burst is detected.
    burst_flag: bool,
    /// Generation counter.
    generation: AtomicU32,
}

impl TrafficAnalyzer {
    /// Create a new traffic analyser.
    pub const fn new() -> Self {
        Self {
            flows: [FlowEntry::empty(); MAX_FLOWS],
            flow_count: 0,
            stats: TrafficStats::new(),
            burst_window_packets: AtomicU64::new(0),
            burst_window_start: AtomicU64::new(0),
            burst_flag: false,
            generation: AtomicU32::new(0),
        }
    }

    // -----------------------------------------------------------------------
    // Packet processing
    // -----------------------------------------------------------------------

    /// Process an incoming packet.  Updates flow tracking, statistics,
    /// and burst detection.
    pub fn process_packet(
        &mut self,
        key: FlowKey,
        bytes: u64,
        timestamp: u64,
    ) {
        // Update global stats.
        self.stats.record(bytes, key.protocol());

        // Find or create the flow entry.
        let flow_idx = self.find_or_create_flow(key, timestamp);

        // Update flow entry.
        if let Some(entry) = self.get_flow_mut(flow_idx) {
            entry.packet_count += 1;
            entry.byte_count += bytes;
            entry.last_seen = timestamp;

            // Burst detection per-flow.
            if timestamp >= entry.burst_window_start + BURST_WINDOW_TICKS {
                // New window.
                entry.burst_window_start = timestamp;
                entry.burst_window_count = 1;
                entry.burst_detected = false;
            } else {
                entry.burst_window_count += 1;
                if entry.burst_window_count >= BURST_THRESHOLD {
                    entry.burst_detected = true;
                }
            }
        }

        // Global burst detection.
        let window_start = self.burst_window_start.load(Ordering::Relaxed);
        if timestamp >= window_start + BURST_WINDOW_TICKS {
            self.burst_window_start.store(timestamp, Ordering::Relaxed);
            self.burst_window_packets.store(1, Ordering::Relaxed);
            self.burst_flag = false;
        } else {
            let count = self.burst_window_packets.fetch_add(1, Ordering::Relaxed) + 1;
            if count >= BURST_THRESHOLD {
                self.burst_flag = true;
            }
        }

        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    // -----------------------------------------------------------------------
    // Flow management
    // -----------------------------------------------------------------------

    /// Find a flow by key, or create a new one.  Returns the index.
    fn find_or_create_flow(&mut self, key: FlowKey, timestamp: u64) -> usize {
        // Search existing.
        for i in 0..self.flow_count as usize {
            if self.flows[i].is_active()
                && self.flows[i].key() == &key
            {
                return i;
            }
        }
        // Create new.
        if self.flow_count as usize < MAX_FLOWS {
            let idx = self.flow_count as usize;
            self.flows[idx] = FlowEntry {
                key,
                packet_count: 0,
                byte_count: 0,
                first_seen: timestamp,
                last_seen: timestamp,
                burst_window_count: 0,
                burst_window_start: timestamp,
                burst_detected: false,
                active: true,
            };
            self.flow_count += 1;
            return idx;
        }
        // Table full — evict the oldest (least-recently-seen) flow.
        let mut oldest_idx = 0usize;
        let mut oldest_ts = u64::MAX;
        for i in 0..MAX_FLOWS {
            if self.flows[i].is_active() && self.flows[i].last_seen < oldest_ts {
                oldest_ts = self.flows[i].last_seen;
                oldest_idx = i;
            }
        }
        self.flows[oldest_idx] = FlowEntry {
            key,
            packet_count: 0,
            byte_count: 0,
            first_seen: timestamp,
            last_seen: timestamp,
            burst_window_count: 0,
            burst_window_start: timestamp,
            burst_detected: false,
            active: true,
        };
        oldest_idx
    }

    /// Get a flow by index.
    pub fn get_flow(&self, idx: usize) -> Option<&FlowEntry> {
        if idx < MAX_FLOWS && self.flows[idx].is_active() {
            Some(&self.flows[idx])
        } else {
            None
        }
    }

    /// Get a mutable flow by index.
    fn get_flow_mut(&mut self, idx: usize) -> Option<&mut FlowEntry> {
        if idx < MAX_FLOWS && self.flows[idx].is_active() {
            Some(&mut self.flows[idx])
        } else {
            None
        }
    }

    /// Find a flow by key.
    pub fn find_flow(&self, key: &FlowKey) -> Option<&FlowEntry> {
        for i in 0..self.flow_count as usize {
            if self.flows[i].is_active() && self.flows[i].key() == key {
                return Some(&self.flows[i]);
            }
        }
        None
    }

    /// Remove a flow by key.
    pub fn remove_flow(&mut self, key: &FlowKey) -> bool {
        for i in 0..self.flow_count as usize {
            if self.flows[i].is_active() && self.flows[i].key() == key {
                // Shift down
                let count = self.flow_count as usize;
                for j in i..count.saturating_sub(1) {
                    self.flows[j] = self.flows[j + 1];
                }
                self.flows[count - 1] = FlowEntry::empty();
                self.flow_count -= 1;
                self.generation.fetch_add(1, Ordering::Release);
                return true;
            }
        }
        false
    }

    /// Expire flows older than `max_age` ticks.
    /// Returns the number of flows evicted.
    pub fn expire_flows(&mut self, now: u64, max_age: u64) -> u32 {
        let mut evicted = 0u32;
        let mut write = 0usize;
        let count = self.flow_count as usize;
        for read in 0..count {
            if self.flows[read].is_active() && now.saturating_sub(self.flows[read].last_seen) > max_age {
                evicted += 1;
            } else {
                if write != read {
                    self.flows[write] = self.flows[read];
                }
                write += 1;
            }
        }
        // Clear remaining slots.
        for j in write..count {
            self.flows[j] = FlowEntry::empty();
        }
        self.flow_count = write as u32;
        if evicted > 0 {
            self.generation.fetch_add(1, Ordering::Release);
        }
        evicted
    }

    // -----------------------------------------------------------------------
    // Statistics / queries
    // -----------------------------------------------------------------------

    /// Number of active flows.
    pub fn flow_count(&self) -> u32 { self.flow_count }

    /// Whether a global burst is currently detected.
    pub fn is_burst_detected(&self) -> bool { self.burst_flag }

    /// Find flows from a specific source IP.
    pub fn flows_by_src_ip(&self, src_ip: u32, results: &mut [usize; MAX_FLOWS]) -> usize {
        let mut n = 0usize;
        for i in 0..self.flow_count as usize {
            if self.flows[i].is_active() && self.flows[i].key().src_ip() == src_ip {
                if n < results.len() {
                    results[n] = i;
                    n += 1;
                }
            }
        }
        n
    }

    /// Get a reference to the traffic statistics.
    pub fn stats(&self) -> &TrafficStats { &self.stats }

    /// Generation counter.
    pub fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }

    // -----------------------------------------------------------------------
    // Statistical analysis
    // -----------------------------------------------------------------------

    /// Compute the average packet size across all flows.
    /// Returns 0 if no packets have been seen.
    pub fn avg_global_packet_size(&self) -> u64 {
        let total_pkts = self.stats.total_packets();
        if total_pkts == 0 {
            return 0;
        }
        self.stats.total_bytes() / total_pkts
    }

    /// Compute the maximum packets-per-tick rate among all flows.
    pub fn max_flow_rate(&self) -> u64 {
        let mut max_rate = 0u64;
        for i in 0..self.flow_count as usize {
            if self.flows[i].is_active() {
                let rate = self.flows[i].packets_per_tick();
                if rate > max_rate {
                    max_rate = rate;
                }
            }
        }
        max_rate
    }

    /// Count flows where burst_detected is true.
    pub fn burst_flow_count(&self) -> u32 {
        let mut count = 0u32;
        for i in 0..self.flow_count as usize {
            if self.flows[i].is_active() && self.flows[i].burst_detected() {
                count += 1;
            }
        }
        count
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_key_hash_deterministic() {
        let k = FlowKey::new(0x0A000001, 0x0A000002, 12345, 80, 6);
        assert_eq!(k.hash(), k.hash());
    }

    #[test]
    fn process_packet_creates_flow() {
        let mut ta = TrafficAnalyzer::new();
        let key = FlowKey::new(0x0A000001, 0x0A000002, 12345, 80, 6);
        ta.process_packet(key, 1500, 100);
        assert_eq!(ta.flow_count(), 1);
        let f = ta.find_flow(&key).unwrap();
        assert_eq!(f.packet_count(), 1);
        assert_eq!(f.byte_count(), 1500);
    }

    #[test]
    fn process_packet_updates_existing_flow() {
        let mut ta = TrafficAnalyzer::new();
        let key = FlowKey::new(0x0A000001, 0x0A000002, 12345, 80, 6);
        ta.process_packet(key, 1500, 100);
        ta.process_packet(key, 500, 200);
        let f = ta.find_flow(&key).unwrap();
        assert_eq!(f.packet_count(), 2);
        assert_eq!(f.byte_count(), 2000);
    }

    #[test]
    fn expire_flows() {
        let mut ta = TrafficAnalyzer::new();
        let key1 = FlowKey::new(1, 2, 3, 4, 6);
        let key2 = FlowKey::new(5, 6, 7, 8, 17);
        ta.process_packet(key1, 100, 100);
        ta.process_packet(key2, 200, 100);
        assert_eq!(ta.flow_count(), 2);

        // Expire flows older than 50 ticks from now=200.
        let evicted = ta.expire_flows(200, 50);
        assert_eq!(evicted, 2); // both last_seen=100, age=100 > 50
        assert_eq!(ta.flow_count(), 0);
    }

    #[test]
    fn burst_detection() {
        let mut ta = TrafficAnalyzer::new();
        let key = FlowKey::new(1, 2, 3, 80, 6);
        // Send BURST_THRESHOLD packets within one window.
        for _ in 0..BURST_THRESHOLD {
            ta.process_packet(key, 100, 10); // same timestamp → same window
        }
        assert!(ta.is_burst_detected());
        assert!(ta.find_flow(&key).unwrap().burst_detected());
    }

    #[test]
    fn max_flow_rate() {
        let mut ta = TrafficAnalyzer::new();
        let key = FlowKey::new(1, 2, 3, 80, 6);
        ta.process_packet(key, 100, 100);
        ta.process_packet(key, 100, 200);
        // 2 packets over 100 ticks = 0 pps (integer division)
        // With duration = 100: 2/100 = 0.
        let rate = ta.max_flow_rate();
        assert_eq!(rate, 0);
    }

    #[test]
    fn stats_counters() {
        let mut ta = TrafficAnalyzer::new();
        let key = FlowKey::new(1, 2, 3, 80, 6);
        ta.process_packet(key, 100, 10);
        assert_eq!(ta.stats().total_packets(), 1);
        assert_eq!(ta.stats().total_bytes(), 100);
        assert_eq!(ta.stats().tcp_packets(), 1);
    }
}
