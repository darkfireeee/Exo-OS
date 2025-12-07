//! # Firewall Rules Engine
//! 
//! High-performance packet filtering with:
//! - Stateful inspection
//! - Connection tracking
//! - Rate limiting
//! - DDoS protection

use alloc::vec::Vec;
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};
use super::nat::{IpRange, PortRange, Protocol, Packet};

/// Firewall action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Accept,
    Drop,
    Reject,
    Log,
    RateLimit,
}

/// Rule match state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchState {
    New,
    Established,
    Related,
    Invalid,
}

/// Firewall rule
#[derive(Debug, Clone)]
pub struct FirewallRule {
    pub id: u32,
    pub chain: Chain,
    pub priority: i32,
    pub enabled: bool,
    
    // Match criteria
    pub src_addr: Option<IpRange>,
    pub dst_addr: Option<IpRange>,
    pub src_port: Option<PortRange>,
    pub dst_port: Option<PortRange>,
    pub protocol: Option<Protocol>,
    pub interface_in: Option<String>,
    pub interface_out: Option<String>,
    pub state: Option<Vec<MatchState>>,
    
    // Action
    pub action: Action,
    pub rate_limit: Option<RateLimit>,
    pub log_prefix: Option<String>,
    
    // Statistics
    pub packets: AtomicU64,
    pub bytes: AtomicU64,
}

impl FirewallRule {
    pub fn new(id: u32, chain: Chain, action: Action) -> Self {
        Self {
            id,
            chain,
            priority: 0,
            enabled: true,
            src_addr: None,
            dst_addr: None,
            src_port: None,
            dst_port: None,
            protocol: None,
            interface_in: None,
            interface_out: None,
            state: None,
            action,
            rate_limit: None,
            log_prefix: None,
            packets: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
        }
    }
    
    pub fn matches(&self, packet: &Packet, state: MatchState) -> bool {
        if !self.enabled {
            return false;
        }
        
        // Check source address
        if let Some(ref src) = self.src_addr {
            if !src.contains(packet.src_addr) {
                return false;
            }
        }
        
        // Check destination address
        if let Some(ref dst) = self.dst_addr {
            if !dst.contains(packet.dst_addr) {
                return false;
            }
        }
        
        // Check protocol
        if let Some(proto) = self.protocol {
            if packet.protocol != proto {
                return false;
            }
        }
        
        // Check ports
        if let Some(ref src_port) = self.src_port {
            if !src_port.contains(packet.src_port) {
                return false;
            }
        }
        
        if let Some(ref dst_port) = self.dst_port {
            if !dst_port.contains(packet.dst_port) {
                return false;
            }
        }
        
        // Check state
        if let Some(ref states) = self.state {
            if !states.contains(&state) {
                return false;
            }
        }
        
        true
    }
    
    pub fn check_rate_limit(&self) -> bool {
        if let Some(ref limit) = self.rate_limit {
            return limit.check();
        }
        true
    }
}

/// Chain type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Chain {
    Input,
    Forward,
    Output,
    Prerouting,
    Postrouting,
}

/// Rate limiting
#[derive(Debug, Clone)]
pub struct RateLimit {
    pub rate: u32,        // packets per second
    pub burst: u32,       // burst size
    last_check: AtomicU64,
    tokens: AtomicU64,
}

impl RateLimit {
    pub fn new(rate: u32, burst: u32) -> Self {
        Self {
            rate,
            burst,
            last_check: AtomicU64::new(current_time()),
            tokens: AtomicU64::new(burst as u64),
        }
    }
    
    pub fn check(&self) -> bool {
        let now = current_time();
        let last = self.last_check.load(Ordering::Relaxed);
        let elapsed = now - last;
        
        // Refill tokens
        let new_tokens = (elapsed * self.rate as u64 / 1000).min(self.burst as u64);
        let tokens = self.tokens.fetch_add(new_tokens, Ordering::Relaxed);
        
        if tokens > 0 {
            self.tokens.fetch_sub(1, Ordering::Relaxed);
            self.last_check.store(now, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

/// Rules engine
pub struct RulesEngine {
    input_rules: Vec<FirewallRule>,
    forward_rules: Vec<FirewallRule>,
    output_rules: Vec<FirewallRule>,
    
    // Default policies
    input_policy: Action,
    forward_policy: Action,
    output_policy: Action,
    
    // Statistics
    input_packets: AtomicU64,
    forward_packets: AtomicU64,
    output_packets: AtomicU64,
    dropped: AtomicU64,
}

impl RulesEngine {
    pub fn new() -> Self {
        Self {
            input_rules: Vec::new(),
            forward_rules: Vec::new(),
            output_rules: Vec::new(),
            input_policy: Action::Accept,
            forward_policy: Action::Accept,
            output_policy: Action::Accept,
            input_packets: AtomicU64::new(0),
            forward_packets: AtomicU64::new(0),
            output_packets: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        }
    }
    
    pub fn add_rule(&mut self, rule: FirewallRule) {
        let chain_rules = match rule.chain {
            Chain::Input => &mut self.input_rules,
            Chain::Forward => &mut self.forward_rules,
            Chain::Output => &mut self.output_rules,
            _ => return,
        };
        
        // Insert by priority
        let pos = chain_rules.iter().position(|r| r.priority > rule.priority).unwrap_or(chain_rules.len());
        chain_rules.insert(pos, rule);
    }
    
    pub fn process_input(&self, packet: &Packet, state: MatchState) -> Action {
        self.input_packets.fetch_add(1, Ordering::Relaxed);
        self.process_chain(&self.input_rules, packet, state, self.input_policy)
    }
    
    pub fn process_forward(&self, packet: &Packet, state: MatchState) -> Action {
        self.forward_packets.fetch_add(1, Ordering::Relaxed);
        self.process_chain(&self.forward_rules, packet, state, self.forward_policy)
    }
    
    pub fn process_output(&self, packet: &Packet, state: MatchState) -> Action {
        self.output_packets.fetch_add(1, Ordering::Relaxed);
        self.process_chain(&self.output_rules, packet, state, self.output_policy)
    }
    
    fn process_chain(&self, rules: &[FirewallRule], packet: &Packet, state: MatchState, default_policy: Action) -> Action {
        for rule in rules {
            if rule.matches(packet, state) {
                // Check rate limit
                if !rule.check_rate_limit() {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    return Action::Drop;
                }
                
                // Log if requested
                if let Some(ref prefix) = rule.log_prefix {
                    log_packet(prefix, packet);
                }
                
                // Update stats
                rule.packets.fetch_add(1, Ordering::Relaxed);
                rule.bytes.fetch_add(packet.len as u64, Ordering::Relaxed);
                
                // Return action
                if rule.action != Action::Log {
                    if rule.action == Action::Drop || rule.action == Action::Reject {
                        self.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                    return rule.action;
                }
            }
        }
        
        // Apply default policy
        if default_policy == Action::Drop || default_policy == Action::Reject {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        default_policy
    }
    
    pub fn set_policy(&mut self, chain: Chain, policy: Action) {
        match chain {
            Chain::Input => self.input_policy = policy,
            Chain::Forward => self.forward_policy = policy,
            Chain::Output => self.output_policy = policy,
            _ => {}
        }
    }
}

fn current_time() -> u64 {
    0 // TODO: Real time
}

fn log_packet(prefix: &str, packet: &Packet) {
    // TODO: Actual logging
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_rule_matching() {
        let mut rule = FirewallRule::new(1, Chain::Input, Action::Accept);
        rule.src_addr = Some(IpRange::cidr([192, 168, 1, 0], 24));
        rule.protocol = Some(Protocol::Tcp);
        rule.dst_port = Some(PortRange::single(80));
        
        let packet = Packet {
            src_addr: [192, 168, 1, 100],
            src_port: 12345,
            dst_addr: [10, 0, 0, 1],
            dst_port: 80,
            protocol: Protocol::Tcp,
            len: 1000,
        };
        
        assert!(rule.matches(&packet, MatchState::New));
    }
    
    #[test]
    fn test_rate_limit() {
        let limit = RateLimit::new(100, 10);
        
        // Should allow burst
        for _ in 0..10 {
            assert!(limit.check());
        }
        
        // Should block after burst
        assert!(!limit.check());
    }
}
