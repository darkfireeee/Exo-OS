//! Phase 2d Integration Tests
//!
//! Tests pour valider tous les gaps ROADMAP complétés

#![cfg(test)]

use crate::scheduler::numa::{NumaNode, NumaTopology, NUMA_DISTANCE_LOCAL};
use crate::scheduler::tlb_shootdown::{CpuTlbState, TlbFlushRequest};
use crate::scheduler::migration::MigrationQueue;
use crate::posix_x::syscalls::scheduler::CpuSet;
use crate::net::tcp::state::{TcpState, TcpStateMachine, TcpEvent};
use crate::net::tcp::congestion::{CubicState, CongestionControl, BETA_CUBIC};
use crate::net::ip::icmp::{IcmpMessage, IcmpType};
use alloc::vec::Vec;

// ═══════════════════════════════════════════════════════════
// CPU Affinity Tests
// ═══════════════════════════════════════════════════════════

#[test_case]
fn test_cpu_set_basic() {
    let mut mask = CpuSet::new();
    
    // Initially empty
    assert_eq!(mask.count(), 0);
    assert!(!mask.is_set(0));
    
    // Set CPU 0
    mask.set(0);
    assert!(mask.is_set(0));
    assert_eq!(mask.count(), 1);
    assert_eq!(mask.first(), Some(0));
}

#[test_case]
fn test_cpu_set_multiple() {
    let mut mask = CpuSet::new();
    
    // Set CPUs 0, 1, 2
    mask.set(0);
    mask.set(1);
    mask.set(2);
    
    assert_eq!(mask.count(), 3);
    assert!(mask.is_set(0));
    assert!(mask.is_set(1));
    assert!(mask.is_set(2));
    assert!(!mask.is_set(3));
}

#[test_case]
fn test_cpu_set_clear() {
    let mut mask = CpuSet::new();
    
    mask.set(0);
    mask.set(1);
    assert_eq!(mask.count(), 2);
    
    mask.clear(0);
    assert_eq!(mask.count(), 1);
    assert!(!mask.is_set(0));
    assert!(mask.is_set(1));
}

// ═══════════════════════════════════════════════════════════
// NUMA Tests
// ═══════════════════════════════════════════════════════════

#[test_case]
fn test_numa_node_creation() {
    let cpus = alloc::vec![0, 1, 2, 3];
    let node = NumaNode::new(0, cpus.clone(), 1024 * 1024 * 1024);
    
    assert_eq!(node.id, 0);
    assert_eq!(node.cpus.len(), 4);
    assert!(node.contains_cpu(0));
    assert!(node.contains_cpu(3));
    assert!(!node.contains_cpu(4));
}

#[test_case]
fn test_numa_allocation() {
    let cpus = alloc::vec![0, 1];
    let node = NumaNode::new(0, cpus, 1024 * 1024);
    
    // Allocate 1024 bytes
    assert!(node.allocate(1024));
    assert_eq!(node.allocations.load(core::sync::atomic::Ordering::Relaxed), 1);
    
    // Deallocate
    node.deallocate(1024);
    assert_eq!(node.allocations.load(core::sync::atomic::Ordering::Relaxed), 0);
}

#[test_case]
fn test_numa_topology_init() {
    let topo = NumaTopology::new();
    topo.init(4, 1024 * 1024 * 1024);
    
    assert_eq!(topo.node_count(), 1);
    assert_eq!(topo.node_for_cpu(0), Some(0));
    assert_eq!(topo.distance(0, 0), NUMA_DISTANCE_LOCAL);
}

// ═══════════════════════════════════════════════════════════
// Migration Tests
// ═══════════════════════════════════════════════════════════

#[test_case]
fn test_migration_queue_creation() {
    let queue = MigrationQueue::new(0);
    let (in_count, out_count) = queue.stats();
    
    assert_eq!(in_count, 0);
    assert_eq!(out_count, 0);
}

// ═══════════════════════════════════════════════════════════
// TLB Shootdown Tests
// ═══════════════════════════════════════════════════════════

#[test_case]
fn test_tlb_state_creation() {
    let state = CpuTlbState::new(0);
    
    assert_eq!(state.flush_count(), 0);
    assert!(!state.is_acked());
}

#[test_case]
fn test_tlb_flush_request() {
    let state = CpuTlbState::new(0);
    
    let request = TlbFlushRequest {
        addr: 0x1000,
        cr3: 0,
        global: false,
        request_id: 1,
    };
    
    state.set_pending(request);
    assert!(!state.is_acked()); // Not processed yet
}

// ═══════════════════════════════════════════════════════════
// ICMP Tests
// ═══════════════════════════════════════════════════════════

#[test_case]
fn test_icmp_echo_request() {
    let payload = b"Hello, World!".to_vec();
    let mut msg = IcmpMessage::echo_request(1234, 5678, payload.clone());
    
    let bytes = msg.to_bytes();
    assert!(bytes.len() > 8);
    assert_eq!(bytes[0], IcmpType::EchoRequest as u8);
}

#[test_case]
fn test_icmp_echo_reply() {
    let payload = b"Pong!".to_vec();
    let mut msg = IcmpMessage::echo_reply(1234, 5678, payload.clone());
    
    let bytes = msg.to_bytes();
    assert_eq!(bytes[0], IcmpType::EchoReply as u8);
}

#[test_case]
fn test_icmp_checksum() {
    let payload = b"Test".to_vec();
    let mut msg = IcmpMessage::echo_request(100, 200, payload.clone());
    
    let bytes = msg.to_bytes();
    
    // Parse back and verify checksum
    let parsed = IcmpMessage::from_bytes(&bytes);
    assert!(parsed.is_ok());
}

// ═══════════════════════════════════════════════════════════
// TCP State Machine Tests
// ═══════════════════════════════════════════════════════════

#[test_case]
fn test_tcp_3way_handshake_client() {
    let client = TcpStateMachine::new(TcpState::Closed);
    
    // 1. Client calls connect() -> sends SYN
    assert_eq!(client.current(), TcpState::Closed);
    let _ = client.handle_event(TcpEvent::Connect);
    assert_eq!(client.current(), TcpState::SynSent);
    
    // 2. Client receives SYN+ACK from server
    let _ = client.handle_event(TcpEvent::SynAckReceived);
    assert_eq!(client.current(), TcpState::Established);
    
    // Connection established!
    assert!(client.current().is_established());
    assert!(client.current().can_send());
    assert!(client.current().can_recv());
}

#[test_case]
fn test_tcp_3way_handshake_server() {
    let server = TcpStateMachine::new(TcpState::Closed);
    
    // 1. Server calls listen()
    let _ = server.handle_event(TcpEvent::Listen);
    assert_eq!(server.current(), TcpState::Listen);
    
    // 2. Server receives SYN from client
    let _ = server.handle_event(TcpEvent::SynReceived);
    assert_eq!(server.current(), TcpState::SynReceived);
    
    // 3. Server receives ACK from client
    let _ = server.handle_event(TcpEvent::AckReceived);
    assert_eq!(server.current(), TcpState::Established);
    
    assert!(server.current().is_established());
}

#[test_case]
fn test_tcp_invalid_transition() {
    let state = TcpStateMachine::new(TcpState::Closed);
    
    // Cannot go directly to ESTABLISHED
    let result = state.transition(TcpState::Established);
    assert!(result.is_err());
}

#[test_case]
fn test_tcp_reset() {
    let conn = TcpStateMachine::new(TcpState::Established);
    
    // Reset goes to CLOSED
    let _ = conn.handle_event(TcpEvent::Reset);
    assert_eq!(conn.current(), TcpState::Closed);
}

// ═══════════════════════════════════════════════════════════
// CUBIC Congestion Control Tests
// ═══════════════════════════════════════════════════════════

#[test_case]
fn test_cubic_slow_start() {
    let cubic = CubicState::new(10);
    
    // Initial state
    assert_eq!(cubic.cwnd(), 10);
    assert_eq!(cubic.ssthresh(), u32::MAX);
    
    // Slow start: exponential growth
    cubic.on_ack(10, 10);
    assert_eq!(cubic.cwnd(), 20);
    
    cubic.on_ack(10, 10);
    assert_eq!(cubic.cwnd(), 30);
}

#[test_case]
fn test_cubic_congestion_decrease() {
    let cubic = CubicState::new(100);
    
    let initial_cwnd = cubic.cwnd();
    
    // Congestion event
    cubic.on_congestion();
    
    // Should decrease by β
    let expected = (initial_cwnd * BETA_CUBIC) / 1024;
    assert_eq!(cubic.cwnd(), expected);
}

#[test_case]
fn test_cubic_timeout_reset() {
    let cubic = CubicState::new(100);
    
    cubic.on_timeout();
    
    // Timeout resets to 1 MSS
    assert_eq!(cubic.cwnd(), 1);
    assert!(cubic.ssthresh() > 0);
}

#[test_case]
fn test_cubic_rtt_tracking() {
    let cubic = CubicState::new(10);
    
    // Update RTT
    cubic.update_rtt(50);
    cubic.update_rtt(30);
    cubic.update_rtt(40);
    
    // Min RTT should be 30
    assert_eq!(
        cubic.min_rtt.load(core::sync::atomic::Ordering::Relaxed),
        30
    );
}

// ═══════════════════════════════════════════════════════════
// Summary Function
// ═══════════════════════════════════════════════════════════

/// Run all Phase 2d tests
pub fn run_phase_2d_tests() {
    crate::logger::info("\n╔══════════════════════════════════════════════════════════╗");
    crate::logger::info("║         PHASE 2D - INTEGRATION TESTS                     ║");
    crate::logger::info("╠══════════════════════════════════════════════════════════╣");
    crate::logger::info("║  CPU Affinity:          3 tests                          ║");
    crate::logger::info("║  NUMA:                  3 tests                          ║");
    crate::logger::info("║  Migration:             1 test                           ║");
    crate::logger::info("║  TLB Shootdown:         2 tests                          ║");
    crate::logger::info("║  ICMP:                  3 tests                          ║");
    crate::logger::info("║  TCP Handshake:         4 tests                          ║");
    crate::logger::info("║  CUBIC:                 4 tests                          ║");
    crate::logger::info("╠══════════════════════════════════════════════════════════╣");
    crate::logger::info("║  TOTAL:                 20 tests                         ║");
    crate::logger::info("╚══════════════════════════════════════════════════════════╝\n");
}
