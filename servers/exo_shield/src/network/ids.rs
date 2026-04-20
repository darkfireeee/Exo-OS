//! Intrusion Detection System for the exo_shield security server.
//!
//! Provides signature-based detection, anomaly detection integration,
//! alert generation, and alert severity classification — all `no_std`
//! compatible.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of IDS signatures.
pub const MAX_IDS_SIGNATURES: usize = 64;

/// Maximum number of active alerts.
pub const MAX_IDS_ALERTS: usize = 64;

/// Maximum length of a signature pattern.
pub const MAX_SIG_PATTERN_LEN: usize = 64;

/// Maximum length of an alert description.
pub const MAX_ALERT_DESC_LEN: usize = 128;

// ---------------------------------------------------------------------------
// Alert severity
// ---------------------------------------------------------------------------

/// Severity level for IDS alerts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(C)]
pub enum AlertSeverity {
    /// Informational — no action required.
    Info = 0,
    /// Low severity — monitor.
    Low = 1,
    /// Medium severity — investigate.
    Medium = 2,
    /// High severity — take action.
    High = 3,
    /// Critical severity — immediate response.
    Critical = 4,
}

impl AlertSeverity {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(AlertSeverity::Info),
            1 => Some(AlertSeverity::Low),
            2 => Some(AlertSeverity::Medium),
            3 => Some(AlertSeverity::High),
            4 => Some(AlertSeverity::Critical),
            _ => None,
        }
    }

    /// Whether this severity requires immediate action.
    pub fn requires_action(&self) -> bool {
        *self >= AlertSeverity::High
    }
}

// ---------------------------------------------------------------------------
// IDS signature
// ---------------------------------------------------------------------------

/// A detection signature — a pattern that, when matched against network
/// or process data, triggers an alert.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct IdsSignature {
    /// Unique signature ID.
    sig_id: u32,
    /// Pattern bytes to match against payload data.
    pattern: [u8; MAX_SIG_PATTERN_LEN],
    /// Pattern length.
    pattern_len: u16,
    /// Protocol the signature applies to (0 = any).
    protocol: u8,
    /// Destination port the signature applies to (0 = any).
    dst_port: u16,
    /// Severity when matched.
    severity: AlertSeverity,
    /// Category tag (user-defined, e.g. 1=exploit, 2=malware, 3=recon).
    category: u8,
    /// Whether the signature is active.
    active: bool,
}

impl IdsSignature {
    /// Create a new signature.
    pub fn new(
        sig_id: u32,
        pattern: &[u8],
        protocol: u8,
        dst_port: u16,
        severity: AlertSeverity,
        category: u8,
    ) -> Option<Self> {
        if pattern.len() >= MAX_SIG_PATTERN_LEN {
            return None;
        }
        let mut buf = [0u8; MAX_SIG_PATTERN_LEN];
        let mut i = 0;
        while i < pattern.len() {
            buf[i] = pattern[i];
            i += 1;
        }
        Some(Self {
            sig_id,
            pattern: buf,
            pattern_len: pattern.len() as u16,
            protocol,
            dst_port,
            severity,
            category,
            active: true,
        })
    }

    /// Create an empty (inactive) signature slot.
    pub const fn empty() -> Self {
        Self {
            sig_id: 0,
            pattern: [0u8; MAX_SIG_PATTERN_LEN],
            pattern_len: 0,
            protocol: 0,
            dst_port: 0,
            severity: AlertSeverity::Info,
            category: 0,
            active: false,
        }
    }

    /// Test whether `payload` matches this signature.
    pub fn matches(&self, payload: &[u8], protocol: u8, dst_port: u16) -> bool {
        if !self.active {
            return false;
        }
        // Protocol filter.
        if self.protocol != 0 && self.protocol != protocol {
            return false;
        }
        // Port filter.
        if self.dst_port != 0 && self.dst_port != dst_port {
            return false;
        }
        // Pattern matching — substring search.
        let pat = &self.pattern[..self.pattern_len as usize];
        if pat.len() > payload.len() {
            return false;
        }
        // Naive substring search (good enough for short patterns).
        let limit = payload.len() - pat.len() + 1;
        let mut i = 0;
        while i < limit {
            let mut j = 0;
            while j < pat.len() && payload[i + j] == pat[j] {
                j += 1;
            }
            if j == pat.len() {
                return true;
            }
            i += 1;
        }
        false
    }

    pub fn sig_id(&self) -> u32 { self.sig_id }
    pub fn severity(&self) -> AlertSeverity { self.severity }
    pub fn category(&self) -> u8 { self.category }
    pub fn is_active(&self) -> bool { self.active }
    pub fn protocol(&self) -> u8 { self.protocol }
    pub fn dst_port(&self) -> u16 { self.dst_port }
}

// ---------------------------------------------------------------------------
// IDS alert
// ---------------------------------------------------------------------------

/// An alert generated by the IDS.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct IdsAlert {
    /// Signature that triggered the alert (0 for anomaly-based).
    sig_id: u32,
    /// Source IP.
    src_ip: u32,
    /// Destination IP.
    dst_ip: u32,
    /// Source port.
    src_port: u16,
    /// Destination port.
    dst_port: u16,
    /// Protocol.
    protocol: u8,
    /// Alert severity.
    severity: AlertSeverity,
    /// Alert category.
    category: u8,
    /// Timestamp.
    timestamp: u64,
    /// Anomaly score if this alert is anomaly-based (Q16.16, 0 = N/A).
    anomaly_score: i32,
    /// Whether the alert is active (not yet resolved).
    resolved: bool,
    /// Whether this entry is in use.
    active: bool,
}

impl IdsAlert {
    pub const fn empty() -> Self {
        Self {
            sig_id: 0,
            src_ip: 0,
            dst_ip: 0,
            src_port: 0,
            dst_port: 0,
            protocol: 0,
            severity: AlertSeverity::Info,
            category: 0,
            timestamp: 0,
            anomaly_score: 0,
            resolved: false,
            active: false,
        }
    }

    pub fn sig_id(&self) -> u32 { self.sig_id }
    pub fn src_ip(&self) -> u32 { self.src_ip }
    pub fn dst_ip(&self) -> u32 { self.dst_ip }
    pub fn src_port(&self) -> u16 { self.src_port }
    pub fn dst_port(&self) -> u16 { self.dst_port }
    pub fn protocol(&self) -> u8 { self.protocol }
    pub fn severity(&self) -> AlertSeverity { self.severity }
    pub fn category(&self) -> u8 { self.category }
    pub fn timestamp(&self) -> u64 { self.timestamp }
    pub fn anomaly_score(&self) -> i32 { self.anomaly_score }
    pub fn is_resolved(&self) -> bool { self.resolved }
    pub fn is_active(&self) -> bool { self.active }

    /// Mark the alert as resolved.
    pub fn resolve(&mut self) {
        self.resolved = true;
    }
}

// ---------------------------------------------------------------------------
// IDS signature matcher
// ---------------------------------------------------------------------------

/// Matches payloads against a set of IDS signatures.
#[derive(Debug)]
#[repr(C)]
pub struct IdsSignatureMatcher {
    signatures: [IdsSignature; MAX_IDS_SIGNATURES],
    sig_count: u32,
}

impl IdsSignatureMatcher {
    pub const fn new() -> Self {
        Self {
            signatures: [IdsSignature::empty(); MAX_IDS_SIGNATURES],
            sig_count: 0,
        }
    }

    /// Add a signature.  Returns `false` if the table is full.
    pub fn add_signature(&mut self, sig: IdsSignature) -> bool {
        if self.sig_count as usize >= MAX_IDS_SIGNATURES {
            return false;
        }
        self.signatures[self.sig_count as usize] = sig;
        self.sig_count += 1;
        true
    }

    /// Remove a signature by ID.
    pub fn remove_signature(&mut self, sig_id: u32) -> bool {
        let count = self.sig_count as usize;
        for i in 0..count {
            if self.signatures[i].is_active() && self.signatures[i].sig_id() == sig_id {
                for j in i..count.saturating_sub(1) {
                    self.signatures[j] = self.signatures[j + 1];
                }
                self.signatures[count - 1] = IdsSignature::empty();
                self.sig_count -= 1;
                return true;
            }
        }
        false
    }

    /// Match a payload against all signatures.  Returns the first matching
    /// signature ID (highest severity first), or `None`.
    pub fn match_payload(
        &self,
        payload: &[u8],
        protocol: u8,
        dst_port: u16,
    ) -> Option<u32> {
        let mut best_sig_id: Option<u32> = None;
        let mut best_severity = AlertSeverity::Info;

        for i in 0..self.sig_count as usize {
            let sig = &self.signatures[i];
            if sig.matches(payload, protocol, dst_port) {
                if sig.severity() > best_severity {
                    best_severity = sig.severity();
                    best_sig_id = Some(sig.sig_id());
                }
            }
        }
        best_sig_id
    }

    /// Get a signature by ID.
    pub fn get_signature(&self, sig_id: u32) -> Option<&IdsSignature> {
        for i in 0..self.sig_count as usize {
            if self.signatures[i].is_active() && self.signatures[i].sig_id() == sig_id {
                return Some(&self.signatures[i]);
            }
        }
        None
    }

    /// Number of active signatures.
    pub fn sig_count(&self) -> u32 { self.sig_count }
}

// ---------------------------------------------------------------------------
// Intrusion Detection System
// ---------------------------------------------------------------------------

/// The main IDS engine: combines signature-based and anomaly-based
/// detection with alert management.
pub struct IntrusionDetectionSystem {
    /// Signature matcher.
    matcher: IdsSignatureMatcher,
    /// Alert table.
    alerts: [IdsAlert; MAX_IDS_ALERTS],
    /// Alert ring-buffer write index.
    alert_head: u32,
    /// Number of active alerts.
    alert_count: u32,
    /// Anomaly threshold (Q16.16) for anomaly-based alert generation.
    anomaly_threshold: i32,
    /// Total alerts generated.
    total_alerts: AtomicU64,
    /// Total alerts by severity.
    alerts_by_severity: [AtomicU32; 5], // Info, Low, Medium, High, Critical
    /// Generation counter.
    generation: AtomicU32,
}

impl IntrusionDetectionSystem {
    /// Create a new IDS with the default anomaly threshold.
    pub const fn new() -> Self {
        Self {
            matcher: IdsSignatureMatcher::new(),
            alerts: [IdsAlert::empty(); MAX_IDS_ALERTS],
            alert_head: 0,
            alert_count: 0,
            anomaly_threshold: 32768, // 0.5 in Q16.16
            total_alerts: AtomicU64::new(0),
            alerts_by_severity: [
                AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0),
                AtomicU32::new(0), AtomicU32::new(0),
            ],
            generation: AtomicU32::new(0),
        }
    }

    // -----------------------------------------------------------------------
    // Signature management
    // -----------------------------------------------------------------------

    /// Add an IDS signature.
    pub fn add_signature(&mut self, sig: IdsSignature) -> bool {
        self.matcher.add_signature(sig)
    }

    /// Remove a signature by ID.
    pub fn remove_signature(&mut self, sig_id: u32) -> bool {
        self.matcher.remove_signature(sig_id)
    }

    /// Number of active signatures.
    pub fn sig_count(&self) -> u32 {
        self.matcher.sig_count()
    }

    // -----------------------------------------------------------------------
    // Signature-based detection
    // -----------------------------------------------------------------------

    /// Inspect a payload for signature matches.  Generates an alert if a
    /// match is found.  Returns the matched signature ID, or `None`.
    pub fn inspect_payload(
        &mut self,
        payload: &[u8],
        src_ip: u32,
        dst_ip: u32,
        src_port: u16,
        dst_port: u16,
        protocol: u8,
        timestamp: u64,
    ) -> Option<u32> {
        let matched = self.matcher.match_payload(payload, protocol, dst_port);
        if let Some(sig_id) = matched {
            let sig = self.matcher.get_signature(sig_id);
            let severity = sig.map(|s| s.severity()).unwrap_or(AlertSeverity::Medium);
            let category = sig.map(|s| s.category()).unwrap_or(0);
            self.generate_alert(IdsAlert {
                sig_id,
                src_ip,
                dst_ip,
                src_port,
                dst_port,
                protocol,
                severity,
                category,
                timestamp,
                anomaly_score: 0,
                resolved: false,
                active: true,
            });
        }
        matched
    }

    // -----------------------------------------------------------------------
    // Anomaly-based detection
    // -----------------------------------------------------------------------

    /// Submit an anomaly score for a source.  If the score exceeds the
    /// threshold, an anomaly alert is generated.
    pub fn report_anomaly(
        &mut self,
        src_ip: u32,
        dst_ip: u32,
        src_port: u16,
        dst_port: u16,
        protocol: u8,
        anomaly_score: i32,
        timestamp: u64,
    ) -> bool {
        if anomaly_score < self.anomaly_threshold {
            return false;
        }
        // Determine severity from anomaly score.
        let severity = self.classify_anomaly_severity(anomaly_score);
        self.generate_alert(IdsAlert {
            sig_id: 0, // 0 = anomaly-based
            src_ip,
            dst_ip,
            src_port,
            dst_port,
            protocol,
            severity,
            category: 99, // anomaly category
            timestamp,
            anomaly_score,
            resolved: false,
            active: true,
        });
        true
    }

    /// Set the anomaly threshold.
    pub fn set_anomaly_threshold(&mut self, threshold: i32) {
        self.anomaly_threshold = threshold;
    }

    /// Get the anomaly threshold.
    pub fn anomaly_threshold(&self) -> i32 {
        self.anomaly_threshold
    }

    /// Classify anomaly severity based on score.
    fn classify_anomaly_severity(&self, score: i32) -> AlertSeverity {
        // Thresholds in Q16.16:
        // 0.5  = 32768 → Medium
        // 0.7  = 45875 → High
        // 0.85 = 55705 → Critical
        if score >= 55705 {
            AlertSeverity::Critical
        } else if score >= 45875 {
            AlertSeverity::High
        } else if score >= 32768 {
            AlertSeverity::Medium
        } else if score >= 16384 {
            AlertSeverity::Low
        } else {
            AlertSeverity::Info
        }
    }

    // -----------------------------------------------------------------------
    // Alert management
    // -----------------------------------------------------------------------

    /// Generate a new alert.
    fn generate_alert(&mut self, alert: IdsAlert) {
        let severity_idx = alert.severity as usize;
        self.total_alerts.fetch_add(1, Ordering::Relaxed);
        if severity_idx < 5 {
            self.alerts_by_severity[severity_idx].fetch_add(1, Ordering::Relaxed);
        }

        // Add to ring buffer.
        let idx = (self.alert_head % MAX_IDS_ALERTS as u32) as usize;
        self.alerts[idx] = alert;
        self.alert_head += 1;
        if self.alert_count < MAX_IDS_ALERTS as u32 {
            self.alert_count += 1;
        }
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Get an alert by recency (0 = most recent).
    pub fn get_alert(&self, recency: usize) -> Option<&IdsAlert> {
        if recency >= self.alert_count as usize {
            return None;
        }
        let idx = if self.alert_head as usize <= MAX_IDS_ALERTS {
            if recency >= self.alert_head as usize {
                return None;
            }
            self.alert_head as usize - 1 - recency
        } else {
            let head = self.alert_head as usize % MAX_IDS_ALERTS;
            let idx = if recency <= head {
                head - recency
            } else {
                MAX_IDS_ALERTS - (recency - head)
            };
            idx % MAX_IDS_ALERTS
        };
        let a = &self.alerts[idx];
        if a.is_active() {
            Some(a)
        } else {
            None
        }
    }

    /// Resolve (acknowledge) the most recent alert for a given signature.
    pub fn resolve_alert_by_sig(&mut self, sig_id: u32) -> bool {
        for i in 0..self.alert_count as usize {
            // Walk from most recent.
            let idx = if self.alert_head as usize <= MAX_IDS_ALERTS {
                self.alert_head as usize - 1 - i
            } else {
                let head = self.alert_head as usize % MAX_IDS_ALERTS;
                if i <= head {
                    head - i
                } else {
                    MAX_IDS_ALERTS - (i - head)
                }
            };
            let a = &mut self.alerts[idx % MAX_IDS_ALERTS];
            if a.is_active() && !a.is_resolved() && a.sig_id() == sig_id {
                a.resolve();
                return true;
            }
        }
        false
    }

    /// Count unresolved alerts.
    pub fn unresolved_alert_count(&self) -> u32 {
        let mut count = 0u32;
        for i in 0..MAX_IDS_ALERTS {
            if self.alerts[i].is_active() && !self.alerts[i].is_resolved() {
                count += 1;
            }
        }
        count
    }

    /// Count unresolved alerts at or above a severity level.
    pub fn unresolved_alerts_at_severity(&self, severity: AlertSeverity) -> u32 {
        let mut count = 0u32;
        for i in 0..MAX_IDS_ALERTS {
            let a = &self.alerts[i];
            if a.is_active() && !a.is_resolved() && a.severity() >= severity {
                count += 1;
            }
        }
        count
    }

    // -----------------------------------------------------------------------
    // Statistics
    // -----------------------------------------------------------------------

    pub fn total_alerts(&self) -> u64 { self.total_alerts.load(Ordering::Relaxed) }
    pub fn alerts_info(&self) -> u32 { self.alerts_by_severity[0].load(Ordering::Relaxed) }
    pub fn alerts_low(&self) -> u32 { self.alerts_by_severity[1].load(Ordering::Relaxed) }
    pub fn alerts_medium(&self) -> u32 { self.alerts_by_severity[2].load(Ordering::Relaxed) }
    pub fn alerts_high(&self) -> u32 { self.alerts_by_severity[3].load(Ordering::Relaxed) }
    pub fn alerts_critical(&self) -> u32 { self.alerts_by_severity[4].load(Ordering::Relaxed) }
    pub fn generation(&self) -> u32 { self.generation.load(Ordering::Acquire) }
    pub fn alert_count(&self) -> u32 { self.alert_count }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_match() {
        let sig = IdsSignature::new(
            1001,
            b"EXPLOIT",
            6, // TCP
            80,
            AlertSeverity::High,
            1,
        ).unwrap();
        // Should match
        assert!(sig.matches(b"GET /EXPLOIT HTTP/1.1", 6, 80));
        // Wrong protocol
        assert!(!sig.matches(b"GET /EXPLOIT HTTP/1.1", 17, 80));
        // Wrong port
        assert!(!sig.matches(b"GET /EXPLOIT HTTP/1.1", 6, 443));
        // No match
        assert!(!sig.matches(b"GET /index.html HTTP/1.1", 6, 80));
    }

    #[test]
    fn signature_match_any_protocol() {
        let sig = IdsSignature::new(
            1002,
            b"TUNNEL",
            0, // any protocol
            0, // any port
            AlertSeverity::Medium,
            2,
        ).unwrap();
        assert!(sig.matches(b"data TUNNEL data", 6, 12345));
        assert!(sig.matches(b"data TUNNEL data", 17, 53));
    }

    #[test]
    fn ids_inspect_payload() {
        let mut ids = IntrusionDetectionSystem::new();
        let sig = IdsSignature::new(
            2001,
            b"MALWARE",
            6,
            0,
            AlertSeverity::Critical,
            2,
        ).unwrap();
        assert!(ids.add_signature(sig));
        let result = ids.inspect_payload(
            b"MALWARE_PAYLOAD", 0x0A000001, 0x0A000002, 12345, 80, 6, 1000,
        );
        assert_eq!(result, Some(2001));
        assert_eq!(ids.total_alerts(), 1);
        assert_eq!(ids.alerts_critical(), 1);
    }

    #[test]
    fn ids_anomaly_detection() {
        let mut ids = IntrusionDetectionSystem::new();
        // Anomaly score above threshold (0.5 = 32768)
        let triggered = ids.report_anomaly(
            0x0A000001, 0x0A000002, 12345, 80, 6, 40000, 1000,
        );
        assert!(triggered);
        assert_eq!(ids.total_alerts(), 1);

        // Below threshold
        let triggered2 = ids.report_anomaly(
            0x0A000001, 0x0A000002, 12345, 80, 6, 10000, 2000,
        );
        assert!(!triggered2);
    }

    #[test]
    fn ids_alert_severity_classification() {
        let ids = IntrusionDetectionSystem::new();
        assert_eq!(ids.classify_anomaly_severity(10000), AlertSeverity::Info);
        assert_eq!(ids.classify_anomaly_severity(20000), AlertSeverity::Low);
        assert_eq!(ids.classify_anomaly_severity(35000), AlertSeverity::Medium);
        assert_eq!(ids.classify_anomaly_severity(50000), AlertSeverity::High);
        assert_eq!(ids.classify_anomaly_severity(60000), AlertSeverity::Critical);
    }

    #[test]
    fn ids_resolve_alert() {
        let mut ids = IntrusionDetectionSystem::new();
        let sig = IdsSignature::new(
            3001, b"SIG", 6, 0, AlertSeverity::High, 1,
        ).unwrap();
        ids.add_signature(sig);
        ids.inspect_payload(b"SIG_DATA", 1, 2, 3, 4, 6, 100);
        assert_eq!(ids.unresolved_alert_count(), 1);
        assert!(ids.resolve_alert_by_sig(3001));
        assert_eq!(ids.unresolved_alert_count(), 0);
    }

    #[test]
    fn ids_multiple_alerts() {
        let mut ids = IntrusionDetectionSystem::new();
        let sig1 = IdsSignature::new(1, b"AAA", 6, 0, AlertSeverity::Low, 1).unwrap();
        let sig2 = IdsSignature::new(2, b"BBB", 6, 0, AlertSeverity::Critical, 2).unwrap();
        ids.add_signature(sig1);
        ids.add_signature(sig2);
        ids.inspect_payload(b"AAA", 1, 2, 3, 4, 6, 100);
        ids.inspect_payload(b"BBB", 1, 2, 3, 4, 6, 200);
        assert_eq!(ids.total_alerts(), 2);
        assert_eq!(ids.alerts_low(), 1);
        assert_eq!(ids.alerts_critical(), 1);
    }

    #[test]
    fn severity_requires_action() {
        assert!(!AlertSeverity::Info.requires_action());
        assert!(!AlertSeverity::Low.requires_action());
        assert!(!AlertSeverity::Medium.requires_action());
        assert!(AlertSeverity::High.requires_action());
        assert!(AlertSeverity::Critical.requires_action());
    }
}
