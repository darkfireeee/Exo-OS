//! Firewall rule engine for the exo_shield security server.
//!
//! Provides rule-based packet filtering with source/destination IP and port,
//! protocol matching, and priority-ordered evaluation — up to 64 rules
//! stored in a static array.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of firewall rules.
pub const MAX_FIREWALL_RULES: usize = 64;

/// Wildcard value for IP (0 = match any).
pub const FIREWALL_WILDCARD_IP: u32 = 0;

/// Wildcard value for port (0 = match any).
pub const FIREWALL_WILDCARD_PORT: u16 = 0;

/// Wildcard value for protocol (0 = match any).
pub const FIREWALL_WILDCARD_PROTO: u8 = 0;

// Legacy name alias.
pub const FIREWALL_WILDCARD: u32 = FIREWALL_WILDCARD_IP;

// ---------------------------------------------------------------------------
// Firewall action
// ---------------------------------------------------------------------------

/// Action to take when a rule matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum FirewallAction {
    /// Allow the packet.
    Allow = 0,
    /// Drop the packet silently.
    Drop = 1,
    /// Reject the packet (send ICMP/port-unreachable).
    Reject = 2,
    /// Log and allow (for auditing).
    LogAllow = 3,
    /// Rate-limit the packet.
    RateLimit = 4,
}

impl FirewallAction {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(FirewallAction::Allow),
            1 => Some(FirewallAction::Drop),
            2 => Some(FirewallAction::Reject),
            3 => Some(FirewallAction::LogAllow),
            4 => Some(FirewallAction::RateLimit),
            _ => None,
        }
    }

    /// Whether this action permits the packet to pass.
    pub fn is_allowed(&self) -> bool {
        matches!(self, FirewallAction::Allow | FirewallAction::LogAllow)
    }
}

// ---------------------------------------------------------------------------
// Firewall rule
// ---------------------------------------------------------------------------

/// A single firewall rule.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct FirewallRule {
    /// Source IP address (0 = wildcard).
    src_ip: u32,
    /// Destination IP address (0 = wildcard).
    dst_ip: u32,
    /// Source port (0 = wildcard).
    src_port: u16,
    /// Destination port (0 = wildcard).
    dst_port: u16,
    /// IP protocol number (0 = wildcard, 6 = TCP, 17 = UDP, 1 = ICMP).
    protocol: u8,
    /// Action when matched.
    action: FirewallAction,
    /// Rule priority (lower = higher priority).
    priority: u16,
    /// Whether the rule is active.
    active: bool,
}

impl FirewallRule {
    /// Create a new firewall rule.
    pub const fn new(
        src_ip: u32,
        dst_ip: u32,
        src_port: u16,
        dst_port: u16,
        protocol: u8,
        action: FirewallAction,
        priority: u16,
    ) -> Self {
        Self {
            src_ip,
            dst_ip,
            src_port,
            dst_port,
            protocol,
            action,
            priority,
            active: true,
        }
    }

    /// Create an empty (inactive) rule slot.
    pub const fn empty() -> Self {
        Self {
            src_ip: 0,
            dst_ip: 0,
            src_port: 0,
            dst_port: 0,
            protocol: 0,
            action: FirewallAction::Drop,
            priority: u16::MAX,
            active: false,
        }
    }

    /// Check whether a packet matches this rule.
    pub fn matches(
        &self,
        src_ip: u32,
        dst_ip: u32,
        src_port: u16,
        dst_port: u16,
        protocol: u8,
    ) -> bool {
        if !self.active {
            return false;
        }
        // Wildcard = 0 means "match any".
        if self.src_ip != FIREWALL_WILDCARD_IP && self.src_ip != src_ip {
            return false;
        }
        if self.dst_ip != FIREWALL_WILDCARD_IP && self.dst_ip != dst_ip {
            return false;
        }
        if self.src_port != FIREWALL_WILDCARD_PORT && self.src_port != src_port {
            return false;
        }
        if self.dst_port != FIREWALL_WILDCARD_PORT && self.dst_port != dst_port {
            return false;
        }
        if self.protocol != FIREWALL_WILDCARD_PROTO && self.protocol != protocol {
            return false;
        }
        true
    }

    // Accessors
    pub fn src_ip(&self) -> u32 { self.src_ip }
    pub fn dst_ip(&self) -> u32 { self.dst_ip }
    pub fn src_port(&self) -> u16 { self.src_port }
    pub fn dst_port(&self) -> u16 { self.dst_port }
    pub fn protocol(&self) -> u8 { self.protocol }
    pub fn action(&self) -> FirewallAction { self.action }
    pub fn priority(&self) -> u16 { self.priority }
    pub fn is_active(&self) -> bool { self.active }
}

// ---------------------------------------------------------------------------
// Firewall
// ---------------------------------------------------------------------------

/// The firewall engine: evaluates rules in priority order and returns the
/// action of the first matching rule.
pub struct Firewall {
    rules: [FirewallRule; MAX_FIREWALL_RULES],
    /// Number of active rules.
    rule_count: u32,
    /// Default action when no rule matches.
    default_action: FirewallAction,
    /// Packets allowed counter.
    packets_allowed: AtomicU64,
    /// Packets denied counter.
    packets_denied: AtomicU64,
    /// Generation counter.
    generation: AtomicU32,
}

impl Firewall {
    /// Create a new firewall with a deny-all default.
    pub const fn new_deny_default() -> Self {
        Self {
            rules: [FirewallRule::empty(); MAX_FIREWALL_RULES],
            rule_count: 0,
            default_action: FirewallAction::Drop,
            packets_allowed: AtomicU64::new(0),
            packets_denied: AtomicU64::new(0),
            generation: AtomicU32::new(0),
        }
    }

    /// Create a new firewall with an allow-all default.
    pub const fn new_allow_default() -> Self {
        Self {
            rules: [FirewallRule::empty(); MAX_FIREWALL_RULES],
            rule_count: 0,
            default_action: FirewallAction::Allow,
            packets_allowed: AtomicU64::new(0),
            packets_denied: AtomicU64::new(0),
            generation: AtomicU32::new(0),
        }
    }

    // -----------------------------------------------------------------------
    // Rule management
    // -----------------------------------------------------------------------

    /// Add a rule.  Returns `false` if the table is full.
    /// Rules are stored in insertion order; evaluation walks them and
    /// picks the highest-priority match.
    pub fn add_rule(&mut self, rule: FirewallRule) -> bool {
        if self.rule_count as usize >= MAX_FIREWALL_RULES {
            return false;
        }
        self.rules[self.rule_count as usize] = rule;
        self.rule_count += 1;
        self.generation.fetch_add(1, Ordering::Release);
        true
    }

    /// Remove a rule by index.  Returns `false` if the index is invalid.
    pub fn remove_rule(&mut self, idx: usize) -> bool {
        if idx >= self.rule_count as usize {
            return false;
        }
        // Shift remaining rules down.
        let count = self.rule_count as usize;
        for j in idx..count.saturating_sub(1) {
            self.rules[j] = self.rules[j + 1];
        }
        self.rules[count - 1] = FirewallRule::empty();
        self.rule_count -= 1;
        self.generation.fetch_add(1, Ordering::Release);
        true
    }

    /// Insert a rule at a specific position (for explicit ordering).
    pub fn insert_rule(&mut self, idx: usize, rule: FirewallRule) -> bool {
        if self.rule_count as usize >= MAX_FIREWALL_RULES {
            return false;
        }
        let count = self.rule_count as usize;
        let pos = idx.min(count);
        // Shift rules up to make room.
        for j in (pos..count).rev() {
            self.rules[j + 1] = self.rules[j];
        }
        self.rules[pos] = rule;
        self.rule_count += 1;
        self.generation.fetch_add(1, Ordering::Release);
        true
    }

    /// Number of active rules.
    pub fn rule_count(&self) -> u32 {
        self.rule_count
    }

    /// Get a rule by index.
    pub fn get_rule(&self, idx: usize) -> Option<&FirewallRule> {
        if idx < self.rule_count as usize {
            Some(&self.rules[idx])
        } else {
            None
        }
    }

    // -----------------------------------------------------------------------
    // Evaluation
    // -----------------------------------------------------------------------

    /// Evaluate a packet against the rule set.  Returns the action of the
    /// highest-priority matching rule, or the default action if none match.
    pub fn evaluate(
        &self,
        src_ip: u32,
        dst_ip: u32,
        src_port: u16,
        dst_port: u16,
        protocol: u8,
    ) -> FirewallAction {
        let mut best_action: Option<FirewallAction> = None;
        let mut best_priority: u16 = u16::MAX;

        for i in 0..self.rule_count as usize {
            let rule = &self.rules[i];
            if !rule.is_active() {
                continue;
            }
            if rule.matches(src_ip, dst_ip, src_port, dst_port, protocol) {
                if rule.priority < best_priority {
                    best_priority = rule.priority;
                    best_action = Some(rule.action());
                }
            }
        }

        let action = best_action.unwrap_or(self.default_action);
        if action.is_allowed() {
            self.packets_allowed.fetch_add(1, Ordering::Relaxed);
        } else {
            self.packets_denied.fetch_add(1, Ordering::Relaxed);
        }
        action
    }

    /// Check whether a packet would be allowed (convenience wrapper).
    pub fn is_allowed(
        &self,
        src_ip: u32,
        dst_ip: u32,
        src_port: u16,
        dst_port: u16,
        protocol: u8,
    ) -> bool {
        self.evaluate(src_ip, dst_ip, src_port, dst_port, protocol).is_allowed()
    }

    // -----------------------------------------------------------------------
    // Statistics
    // -----------------------------------------------------------------------

    /// Packets allowed counter.
    pub fn packets_allowed(&self) -> u64 {
        self.packets_allowed.load(Ordering::Relaxed)
    }

    /// Packets denied counter.
    pub fn packets_denied(&self) -> u64 {
        self.packets_denied.load(Ordering::Relaxed)
    }

    /// Total packets evaluated.
    pub fn total_packets(&self) -> u64 {
        self.packets_allowed() + self.packets_denied()
    }

    /// Generation counter.
    pub fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }

    /// Default action.
    pub fn default_action(&self) -> FirewallAction {
        self.default_action
    }

    /// Set the default action.
    pub fn set_default_action(&mut self, action: FirewallAction) {
        self.default_action = action;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_match_wildcard() {
        let rule = FirewallRule::new(
            FIREWALL_WILDCARD_IP,
            FIREWALL_WILDCARD_IP,
            FIREWALL_WILDCARD_PORT,
            80,
            6,
            FirewallAction::Allow,
            10,
        );
        // Should match any src, dst=any, any src_port, dst_port=80, TCP
        assert!(rule.matches(0x0A000001, 0x0A000002, 12345, 80, 6));
        assert!(!rule.matches(0x0A000001, 0x0A000002, 12345, 443, 6));
        assert!(!rule.matches(0x0A000001, 0x0A000002, 12345, 80, 17));
    }

    #[test]
    fn rule_match_specific_ip() {
        let rule = FirewallRule::new(
            0x0A000001,
            0x0A000002,
            FIREWALL_WILDCARD_PORT,
            22,
            6,
            FirewallAction::Allow,
            5,
        );
        assert!(rule.matches(0x0A000001, 0x0A000002, 54321, 22, 6));
        assert!(!rule.matches(0x0A000003, 0x0A000002, 54321, 22, 6));
    }

    #[test]
    fn firewall_priority() {
        let mut fw = Firewall::new_deny_default();
        // Low-priority deny rule.
        fw.add_rule(FirewallRule::new(
            FIREWALL_WILDCARD_IP, FIREWALL_WILDCARD_IP,
            FIREWALL_WILDCARD_PORT, 80,
            FIREWALL_WILDCARD_PROTO, FirewallAction::Drop, 100,
        ));
        // High-priority allow rule.
        fw.add_rule(FirewallRule::new(
            0x0A000001, FIREWALL_WILDCARD_IP,
            FIREWALL_WILDCARD_PORT, 80,
            FIREWALL_WILDCARD_PROTO, FirewallAction::Allow, 10,
        ));
        // From the allowed IP → Allow (higher priority)
        assert_eq!(fw.evaluate(0x0A000001, 0x0A000002, 12345, 80, 6), FirewallAction::Allow);
        // From another IP → Drop (lower priority deny)
        assert_eq!(fw.evaluate(0x0A000003, 0x0A000002, 12345, 80, 6), FirewallAction::Drop);
    }

    #[test]
    fn firewall_default_action() {
        let fw = Firewall::new_deny_default();
        assert_eq!(fw.evaluate(1, 2, 3, 4, 6), FirewallAction::Drop);
    }

    #[test]
    fn firewall_counters() {
        let mut fw = Firewall::new_allow_default();
        fw.add_rule(FirewallRule::new(
            FIREWALL_WILDCARD_IP, FIREWALL_WILDCARD_IP,
            FIREWALL_WILDCARD_PORT, 22,
            FIREWALL_WILDCARD_PROTO, FirewallAction::Drop, 1,
        ));
        fw.evaluate(0, 0, 0, 80, 6);  // allowed (no match → default)
        fw.evaluate(0, 0, 0, 22, 6);  // denied
        assert_eq!(fw.packets_allowed(), 1);
        assert_eq!(fw.packets_denied(), 1);
    }

    #[test]
    fn firewall_remove_rule() {
        let mut fw = Firewall::new_deny_default();
        fw.add_rule(FirewallRule::new(
            FIREWALL_WILDCARD_IP, FIREWALL_WILDCARD_IP,
            FIREWALL_WILDCARD_PORT, 80,
            FIREWALL_WILDCARD_PROTO, FirewallAction::Allow, 10,
        ));
        assert_eq!(fw.rule_count(), 1);
        assert!(fw.remove_rule(0));
        assert_eq!(fw.rule_count(), 0);
    }
}
