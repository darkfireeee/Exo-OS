//! Network security module for the exo_shield security server.
//!
//! Provides firewall rule evaluation, traffic analysis, DNS guard, and
//! intrusion detection — all `no_std` compatible with static arrays.

pub mod dns_guard;
pub mod firewall;
pub mod ids;
pub mod traffic_analysis;

// Re-export primary public types.
pub use dns_guard::{DnsExfilDetection, DnsGuard, DnsQueryLog};
pub use firewall::{
    block_pid, firewall_init, is_pid_blocked, unblock_pid, Firewall, FirewallAction, FirewallRule,
    FIREWALL_WILDCARD,
};
pub use ids::{
    AlertSeverity, IdsAlert, IdsSignature, IdsSignatureMatcher, IntrusionDetectionSystem,
};
pub use traffic_analysis::{FlowEntry, FlowKey, TrafficAnalyzer, BURST_THRESHOLD};
