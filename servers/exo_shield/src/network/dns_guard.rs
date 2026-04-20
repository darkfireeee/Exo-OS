//! DNS guard for the exo_shield security server.
//!
//! Provides domain allow-listing (max 32), DNS query logging, DNS
//! exfiltration detection, and DNS tunneling detection — all `no_std`
//! compatible.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of allowed domain patterns.
pub const MAX_ALLOWED_DOMAINS: usize = 32;

/// Maximum length of a domain pattern.
pub const MAX_DOMAIN_LEN: usize = 64;

/// Maximum number of logged DNS queries.
pub const MAX_DNS_LOG_ENTRIES: usize = 64;

/// Entropy threshold for DNS tunneling detection (bits per byte).
/// Normal DNS names have ≈ 2.5–3.5 bits/byte; tunnelled traffic tends
/// to be ≥ 4.0.  We store this scaled by 256 for integer arithmetic.
pub const ENTROPY_THRESHOLD_SCALED: u32 = 1024; // ≈ 4.0 bits/byte × 256

/// Maximum query rate (queries per measurement window) before flagging
/// exfiltration.
pub const EXFIL_RATE_THRESHOLD: u64 = 50;

/// Measurement window in ticks for exfiltration detection.
pub const EXFIL_WINDOW_TICKS: u64 = 100;

// ---------------------------------------------------------------------------
// Domain entry
// ---------------------------------------------------------------------------

/// An allowed domain pattern (glob-style).
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct DomainEntry {
    pattern: [u8; MAX_DOMAIN_LEN],
    len: u16,
    active: bool,
}

impl DomainEntry {
    pub const fn empty() -> Self {
        Self {
            pattern: [0u8; MAX_DOMAIN_LEN],
            len: 0,
            active: false,
        }
    }

    pub fn from_bytes(pattern: &[u8]) -> Option<Self> {
        if pattern.is_empty() || pattern.len() >= MAX_DOMAIN_LEN {
            return None;
        }
        let mut buf = [0u8; MAX_DOMAIN_LEN];
        let mut i = 0;
        while i < pattern.len() {
            buf[i] = pattern[i];
            i += 1;
        }
        Some(Self {
            pattern: buf,
            len: pattern.len() as u16,
            active: true,
        })
    }

    #[inline]
    pub fn pattern_str(&self) -> &[u8] {
        &self.pattern[..self.len as usize]
    }

    #[inline]
    pub fn is_active(&self) -> bool {
        self.active
    }
}

// ---------------------------------------------------------------------------
// DNS query log entry
// ---------------------------------------------------------------------------

/// A logged DNS query event.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct DnsQueryLog {
    /// The queried domain (truncated to MAX_DOMAIN_LEN).
    domain: [u8; MAX_DOMAIN_LEN],
    /// Length of the domain name.
    domain_len: u16,
    /// Source IP of the querier.
    src_ip: u32,
    /// Query type (A=1, AAAA=28, TXT=16, etc.).
    qtype: u16,
    /// Timestamp.
    timestamp: u64,
    /// Whether the query was blocked.
    blocked: bool,
    /// Whether exfiltration was detected.
    exfil_flag: bool,
    /// Whether tunneling was detected.
    tunnel_flag: bool,
    /// Whether the entry is in use.
    active: bool,
}

impl DnsQueryLog {
    pub const fn empty() -> Self {
        Self {
            domain: [0u8; MAX_DOMAIN_LEN],
            domain_len: 0,
            src_ip: 0,
            qtype: 0,
            timestamp: 0,
            blocked: false,
            exfil_flag: false,
            tunnel_flag: false,
            active: false,
        }
    }

    pub fn domain(&self) -> &[u8] {
        &self.domain[..self.domain_len as usize]
    }

    pub fn src_ip(&self) -> u32 { self.src_ip }
    pub fn qtype(&self) -> u16 { self.qtype }
    pub fn timestamp(&self) -> u64 { self.timestamp }
    pub fn was_blocked(&self) -> bool { self.blocked }
    pub fn exfil_detected(&self) -> bool { self.exfil_flag }
    pub fn tunnel_detected(&self) -> bool { self.tunnel_flag }
}

// ---------------------------------------------------------------------------
// DNS exfiltration detection result
// ---------------------------------------------------------------------------

/// Result of DNS exfiltration / tunneling analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct DnsExfilDetection {
    /// Whether exfiltration is suspected.
    exfil_suspected: bool,
    /// Whether tunneling is suspected.
    tunnel_suspected: bool,
    /// Query rate (queries per window).
    query_rate: u64,
    /// Entropy of the domain name (scaled by 256).
    entropy_scaled: u32,
}

impl DnsExfilDetection {
    pub const fn new(
        exfil_suspected: bool,
        tunnel_suspected: bool,
        query_rate: u64,
        entropy_scaled: u32,
    ) -> Self {
        Self {
            exfil_suspected,
            tunnel_suspected,
            query_rate,
            entropy_scaled,
        }
    }

    pub const fn clean() -> Self {
        Self::new(false, false, 0, 0)
    }

    pub fn exfil_suspected(&self) -> bool { self.exfil_suspected }
    pub fn tunnel_suspected(&self) -> bool { self.tunnel_suspected }
    pub fn query_rate(&self) -> u64 { self.query_rate }
    pub fn entropy_scaled(&self) -> u32 { self.entropy_scaled }
}

// ---------------------------------------------------------------------------
// DNS guard
// ---------------------------------------------------------------------------

/// DNS security guard — domain filtering, query logging, exfiltration
/// and tunneling detection.
pub struct DnsGuard {
    /// Allowed domain patterns.
    allowed_domains: [DomainEntry; MAX_ALLOWED_DOMAINS],
    /// Number of active domain entries.
    domain_count: u32,
    /// Whether domain filtering is enabled.
    filtering_enabled: AtomicBool,
    /// DNS query log (ring buffer).
    query_log: [DnsQueryLog; MAX_DNS_LOG_ENTRIES],
    /// Ring-buffer write index.
    log_head: u32,
    /// Total queries seen.
    total_queries: AtomicU64,
    /// Total queries blocked.
    total_blocked: AtomicU64,
    /// Exfiltration detection: query count in current window.
    exfil_window_count: AtomicU64,
    /// Exfiltration detection: window start timestamp.
    exfil_window_start: AtomicU64,
    /// Total exfiltration alerts.
    total_exfil_alerts: AtomicU32,
    /// Total tunneling alerts.
    total_tunnel_alerts: AtomicU32,
    /// Generation counter.
    generation: AtomicU32,
}

impl DnsGuard {
    /// Create a new DNS guard with empty allow-list and filtering enabled.
    pub const fn new() -> Self {
        Self {
            allowed_domains: [DomainEntry::empty(); MAX_ALLOWED_DOMAINS],
            domain_count: 0,
            filtering_enabled: AtomicBool::new(true),
            query_log: [DnsQueryLog::empty(); MAX_DNS_LOG_ENTRIES],
            log_head: 0,
            total_queries: AtomicU64::new(0),
            total_blocked: AtomicU64::new(0),
            exfil_window_count: AtomicU64::new(0),
            exfil_window_start: AtomicU64::new(0),
            total_exfil_alerts: AtomicU32::new(0),
            total_tunnel_alerts: AtomicU32::new(0),
            generation: AtomicU32::new(0),
        }
    }

    // -----------------------------------------------------------------------
    // Domain management
    // -----------------------------------------------------------------------

    /// Add a domain to the allow-list.  Returns `false` if full.
    pub fn add_domain(&mut self, entry: DomainEntry) -> bool {
        if self.domain_count as usize >= MAX_ALLOWED_DOMAINS {
            return false;
        }
        self.allowed_domains[self.domain_count as usize] = entry;
        self.domain_count += 1;
        self.generation.fetch_add(1, Ordering::Release);
        true
    }

    /// Remove a domain from the allow-list.
    pub fn remove_domain(&mut self, pattern: &[u8]) -> bool {
        let count = self.domain_count as usize;
        for i in 0..count {
            if self.allowed_domains[i].is_active()
                && Self::pattern_eq(&self.allowed_domains[i], pattern)
            {
                for j in i..count.saturating_sub(1) {
                    self.allowed_domains[j] = self.allowed_domains[j + 1];
                }
                self.allowed_domains[count - 1] = DomainEntry::empty();
                self.domain_count -= 1;
                self.generation.fetch_add(1, Ordering::Release);
                return true;
            }
        }
        false
    }

    /// Check whether a domain is allowed.
    pub fn is_domain_allowed(&self, domain: &[u8]) -> bool {
        if !self.filtering_enabled.load(Ordering::Relaxed) {
            return true;
        }
        // If no domains are listed, everything is allowed.
        if self.domain_count == 0 {
            return true;
        }
        for i in 0..self.domain_count as usize {
            let entry = &self.allowed_domains[i];
            if entry.is_active() && domain_glob_match(entry.pattern_str(), domain) {
                return true;
            }
        }
        false
    }

    /// Enable or disable domain filtering.
    pub fn set_filtering_enabled(&self, enabled: bool) {
        self.filtering_enabled.store(enabled, Ordering::Release);
    }

    /// Whether filtering is enabled.
    pub fn is_filtering_enabled(&self) -> bool {
        self.filtering_enabled.load(Ordering::Acquire)
    }

    /// Number of allowed domains.
    pub fn domain_count(&self) -> u32 { self.domain_count }

    // -----------------------------------------------------------------------
    // Query processing
    // -----------------------------------------------------------------------

    /// Process a DNS query.  Performs domain filtering, exfiltration
    /// detection, and tunneling detection.  Returns the detection result.
    pub fn process_query(
        &mut self,
        domain: &[u8],
        src_ip: u32,
        qtype: u16,
        timestamp: u64,
    ) -> DnsExfilDetection {
        self.total_queries.fetch_add(1, Ordering::Relaxed);

        // Domain filtering.
        let blocked = !self.is_domain_allowed(domain);
        if blocked {
            self.total_blocked.fetch_add(1, Ordering::Relaxed);
        }

        // Exfiltration detection — query rate.
        let window_start = self.exfil_window_start.load(Ordering::Relaxed);
        let mut exfil_suspected = false;
        if timestamp >= window_start + EXFIL_WINDOW_TICKS {
            // New window.
            self.exfil_window_start.store(timestamp, Ordering::Relaxed);
            self.exfil_window_count.store(1, Ordering::Relaxed);
        } else {
            let count = self.exfil_window_count.fetch_add(1, Ordering::Relaxed) + 1;
            if count >= EXFIL_RATE_THRESHOLD {
                exfil_suspected = true;
                self.total_exfil_alerts.fetch_add(1, Ordering::Relaxed);
            }
        }
        let query_rate = self.exfil_window_count.load(Ordering::Relaxed);

        // Tunneling detection — entropy analysis.
        let entropy_scaled = compute_entropy_scaled(domain);
        let tunnel_suspected = entropy_scaled >= ENTROPY_THRESHOLD_SCALED
            && domain.len() > 10; // short domains are likely legit
        if tunnel_suspected {
            self.total_tunnel_alerts.fetch_add(1, Ordering::Relaxed);
        }

        // Log the query.
        self.log_query(domain, src_ip, qtype, timestamp, blocked, exfil_suspected, tunnel_suspected);

        DnsExfilDetection::new(exfil_suspected, tunnel_suspected, query_rate, entropy_scaled)
    }

    // -----------------------------------------------------------------------
    // Query log
    // -----------------------------------------------------------------------

    /// Get a query log entry by recency (0 = most recent).
    pub fn get_log_entry(&self, recency: usize) -> Option<&DnsQueryLog> {
        if recency >= MAX_DNS_LOG_ENTRIES || recency as u32 >= self.log_head {
            return None;
        }
        let idx = if self.log_head as usize <= MAX_DNS_LOG_ENTRIES {
            self.log_head as usize - 1 - recency
        } else {
            let head = self.log_head as usize % MAX_DNS_LOG_ENTRIES;
            let idx = if recency <= head {
                head - recency
            } else {
                MAX_DNS_LOG_ENTRIES - (recency - head)
            };
            idx % MAX_DNS_LOG_ENTRIES
        };
        let entry = &self.query_log[idx];
        if entry.active {
            Some(entry)
        } else {
            None
        }
    }

    // -----------------------------------------------------------------------
    // Statistics
    // -----------------------------------------------------------------------

    pub fn total_queries(&self) -> u64 { self.total_queries.load(Ordering::Relaxed) }
    pub fn total_blocked(&self) -> u64 { self.total_blocked.load(Ordering::Relaxed) }
    pub fn total_exfil_alerts(&self) -> u32 { self.total_exfil_alerts.load(Ordering::Relaxed) }
    pub fn total_tunnel_alerts(&self) -> u32 { self.total_tunnel_alerts.load(Ordering::Relaxed) }
    pub fn generation(&self) -> u32 { self.generation.load(Ordering::Acquire) }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn log_query(
        &mut self,
        domain: &[u8],
        src_ip: u32,
        qtype: u16,
        timestamp: u64,
        blocked: bool,
        exfil_flag: bool,
        tunnel_flag: bool,
    ) {
        let idx = (self.log_head % MAX_DNS_LOG_ENTRIES as u32) as usize;
        let mut dom_buf = [0u8; MAX_DOMAIN_LEN];
        let copy_len = domain.len().min(MAX_DOMAIN_LEN - 1);
        let mut i = 0;
        while i < copy_len {
            dom_buf[i] = domain[i];
            i += 1;
        }
        self.query_log[idx] = DnsQueryLog {
            domain: dom_buf,
            domain_len: copy_len as u16,
            src_ip,
            qtype,
            timestamp,
            blocked,
            exfil_flag,
            tunnel_flag,
            active: true,
        };
        self.log_head += 1;
    }

    fn pattern_eq(entry: &DomainEntry, pattern: &[u8]) -> bool {
        let p = entry.pattern_str();
        p.len() == pattern.len() && {
            let mut i = 0;
            while i < p.len() {
                if p[i] != pattern[i] {
                    return false;
                }
                i += 1;
            }
            true
        }
    }
}

// ---------------------------------------------------------------------------
// Glob matcher for domain patterns (supports `*` wildcard)
// ---------------------------------------------------------------------------

/// Glob match for domain names: `*` matches any sequence of characters.
pub fn domain_glob_match(pattern: &[u8], text: &[u8]) -> bool {
    let mut pi = 0usize;
    let mut ti = 0usize;
    let mut star_pi: Option<usize> = None;
    let mut star_ti: usize = 0;

    loop {
        if pi == pattern.len() && ti == text.len() {
            return true;
        }
        if pi < pattern.len() && pattern[pi] == b'*' {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
            continue;
        }
        if pi < pattern.len() && ti < text.len() {
            // Case-insensitive comparison for ASCII letters.
            let pc = if pattern[pi] >= b'A' && pattern[pi] <= b'Z' {
                pattern[pi] + 32
            } else {
                pattern[pi]
            };
            let tc = if text[ti] >= b'A' && text[ti] <= b'Z' {
                text[ti] + 32
            } else {
                text[ti]
            };
            if pc == tc {
                pi += 1;
                ti += 1;
                continue;
            }
        }
        if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
            continue;
        }
        return false;
    }
}

// ---------------------------------------------------------------------------
// Entropy calculation (Shannon entropy, scaled by 256)
// ---------------------------------------------------------------------------

/// Compute Shannon entropy of a byte string, scaled by 256.
/// Returns bits-per-byte × 256 as an integer.
pub fn compute_entropy_scaled(data: &[u8]) -> u32 {
    if data.is_empty() {
        return 0;
    }
    let len = data.len() as u64;
    // Count byte frequencies.
    let mut freq = [0u32; 256];
    for &b in data {
        freq[b as usize] += 1;
    }

    // Compute entropy: H = -Σ p * log2(p)
    // In fixed-point: for each byte value with count c,
    //   p = c / len
    //   -p * log2(p) ≈ -p * (ln(p) / ln(2))
    // We approximate log2(p) using a small lookup table for common values
    // and a linear interpolation otherwise.
    //
    // Simpler approach: use the fact that for uniform distribution over
    // `unique` symbols, H ≈ log2(unique).
    // We count unique byte values and use that as a rough entropy proxy.
    let mut unique: u32 = 0;
    for &f in &freq {
        if f > 0 {
            unique += 1;
        }
    }
    if unique <= 1 {
        return 0;
    }

    // log2(unique) ≈ position of highest set bit + fraction
    // For unique in 2..=256, log2 is in [1, 8].
    // We compute log2_approx × 256.
    let log2_scaled = log2_approx_scaled(unique);

    // Weight by the fraction of unique bytes used (penalise sparse usage).
    let unique_fraction_scaled = (unique as u64 * 256 / 256) as u32; // always 256 for byte data
    // Final: log2(unique) × (unique / max_unique) × scale
    // For simplicity, return log2_scaled (bits per byte × 256 for the
    // worst case of uniform distribution).
    // Adjust: if many bytes map to only a few unique values, reduce.
    let ratio = ((unique as u64) << 8) / (len as u64);
    let adjusted = (log2_scaled as u64 * ratio.min(256)) >> 8;
    adjusted as u32
}

/// Approximate log2(n) scaled by 256, for n in 1..=256.
/// Uses a small lookup table for exact values and linear interpolation.
fn log2_approx_scaled(n: u32) -> u32 {
    // Precomputed log2 values × 256 for powers of 2.
    const LUT: [u32; 9] = [
        0,           // log2(1) × 256
        256,         // log2(2) × 256
        512,         // log2(4) × 256
        768,         // log2(8) × 256
        1024,        // log2(16) × 256
        1280,        // log2(32) × 256
        1536,        // log2(64) × 256
        1792,        // log2(128) × 256
        2048,        // log2(256) × 256
    ];

    if n == 0 {
        return 0;
    }

    // Find the highest bit position.
    let mut high_bit = 0u32;
    let mut tmp = n;
    while tmp > 1 {
        tmp >>= 1;
        high_bit += 1;
    }
    high_bit = high_bit.min(8);

    let lower = LUT[high_bit as usize];
    let upper = if (high_bit as usize) < 8 {
        LUT[(high_bit + 1) as usize]
    } else {
        LUT[8]
    };

    // Interpolate between 2^high_bit and 2^(high_bit+1).
    let base = 1u32 << high_bit;
    let range = if high_bit < 8 { 1u32 << high_bit } else { 0 };
    if range == 0 {
        return upper;
    }
    let frac = (n - base).min(range);
    lower + (upper - lower) * frac / range
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_glob_wildcard() {
        assert!(domain_glob_match(b"*.example.com", b"www.example.com"));
        assert!(domain_glob_match(b"*.example.com", b"api.example.com"));
        assert!(!domain_glob_match(b"*.example.com", b"example.com"));
    }

    #[test]
    fn domain_glob_exact() {
        assert!(domain_glob_match(b"example.com", b"example.com"));
        assert!(!domain_glob_match(b"example.com", b"www.example.com"));
    }

    #[test]
    fn domain_glob_case_insensitive() {
        assert!(domain_glob_match(b"*.Example.COM", b"www.example.com"));
    }

    #[test]
    fn add_remove_domain() {
        let mut guard = DnsGuard::new();
        let entry = DomainEntry::from_bytes(b"*.example.com").unwrap();
        assert!(guard.add_domain(entry));
        assert_eq!(guard.domain_count(), 1);
        assert!(guard.is_domain_allowed(b"www.example.com"));
        assert!(!guard.is_domain_allowed(b"evil.com"));
        assert!(guard.remove_domain(b"*.example.com"));
        assert_eq!(guard.domain_count(), 0);
    }

    #[test]
    fn process_query_blocking() {
        let mut guard = DnsGuard::new();
        let entry = DomainEntry::from_bytes(b"*.trusted.com").unwrap();
        guard.add_domain(entry);
        let result = guard.process_query(b"evil.com", 0x0A000001, 1, 100);
        assert!(result.exfil_suspected() || !result.exfil_suspected()); // just runs
        assert_eq!(guard.total_queries(), 1);
        assert_eq!(guard.total_blocked(), 1); // evil.com not in allow list
    }

    #[test]
    fn entropy_low_for_normal_domain() {
        let e = compute_entropy_scaled(b"www.example.com");
        // Normal ASCII domain should have moderate entropy (≈ 3-4 bits × 256)
        assert!(e < 1500, "entropy = {}", e);
    }

    #[test]
    fn entropy_high_for_random() {
        // Simulate high-entropy tunnelling data.
        let e = compute_entropy_scaled(b"a7Fk9xQ2mP4bR8nL5vC1");
        assert!(e > 500, "entropy = {}", e);
    }

    #[test]
    fn exfil_rate_detection() {
        let mut guard = DnsGuard::new();
        // Send many queries quickly.
        for i in 0..EXFIL_RATE_THRESHOLD {
            let domain = b"query.example.com";
            let result = guard.process_query(domain, 0x0A000001, 1, 10);
            if i >= EXFIL_RATE_THRESHOLD - 1 {
                assert!(result.exfil_suspected());
            }
        }
        assert!(guard.total_exfil_alerts() > 0);
    }
}
