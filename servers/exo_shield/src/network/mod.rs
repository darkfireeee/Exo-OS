//! Network security module for the exo_shield security server.
//!
//! Provides firewall rule evaluation, traffic analysis, DNS guard, and
//! intrusion detection — all `no_std` compatible with static arrays.

pub mod dns_guard;
pub mod firewall;
pub mod ids;
pub mod traffic_analysis;

// Re-export primary public types.
pub use firewall::{FirewallAction, FirewallRule, Firewall, FIREWALL_WILDCARD};
pub use traffic_analysis::{FlowEntry, FlowKey, TrafficAnalyzer, BURST_THRESHOLD};
pub use dns_guard::{DnsGuard, DnsQueryLog, DnsExfilDetection};
pub use ids::{
    AlertSeverity, IdsAlert, IdsSignature, IdsSignatureMatcher, IntrusionDetectionSystem,
};
