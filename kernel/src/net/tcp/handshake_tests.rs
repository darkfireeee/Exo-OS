//! TCP 3-Way Handshake Validation Tests
//!
//! Phase 2d: Verify TCP connection establishment

use crate::net::tcp::state::{TcpState, TcpStateMachine, TcpEvent};

/// Test TCP 3-way handshake (Client side)
///
/// Client:  CLOSED -> SYN_SENT -> ESTABLISHED
/// Server:  LISTEN -> SYN_RECEIVED -> ESTABLISHED
#[test]
fn test_tcp_3way_handshake_client() {
    // Client side
    let client = TcpStateMachine::new(TcpState::Closed);
    
    // 1. Client calls connect() -> sends SYN
    assert_eq!(client.current(), TcpState::Closed);
    client.handle_event(TcpEvent::Connect).unwrap();
    assert_eq!(client.current(), TcpState::SynSent);
    
    // 2. Client receives SYN+ACK from server
    client.handle_event(TcpEvent::SynAckReceived).unwrap();
    assert_eq!(client.current(), TcpState::Established);
    
    // Connection established!
    assert!(client.current().is_established());
    assert!(client.current().can_send());
    assert!(client.current().can_recv());
}

/// Test TCP 3-way handshake (Server side)
#[test]
fn test_tcp_3way_handshake_server() {
    // Server side
    let server = TcpStateMachine::new(TcpState::Closed);
    
    // 1. Server calls listen()
    assert_eq!(server.current(), TcpState::Closed);
    server.handle_event(TcpEvent::Listen).unwrap();
    assert_eq!(server.current(), TcpState::Listen);
    
    // 2. Server receives SYN from client -> sends SYN+ACK
    server.handle_event(TcpEvent::SynReceived).unwrap();
    assert_eq!(server.current(), TcpState::SynReceived);
    
    // 3. Server receives ACK from client
    server.handle_event(TcpEvent::AckReceived).unwrap();
    assert_eq!(server.current(), TcpState::Established);
    
    // Connection established!
    assert!(server.current().is_established());
    assert!(server.current().can_send());
    assert!(server.current().can_recv());
}

/// Test full 3-way handshake simulation
#[test]
fn test_tcp_3way_handshake_full() {
    // Client and Server
    let client = TcpStateMachine::new(TcpState::Closed);
    let server = TcpStateMachine::new(TcpState::Closed);
    
    // Server: listen()
    server.handle_event(TcpEvent::Listen).unwrap();
    
    // Client: connect() -> SYN sent
    client.handle_event(TcpEvent::Connect).unwrap();
    assert_eq!(client.current(), TcpState::SynSent);
    
    // Server: receives SYN -> SYN+ACK sent
    server.handle_event(TcpEvent::SynReceived).unwrap();
    assert_eq!(server.current(), TcpState::SynReceived);
    
    // Client: receives SYN+ACK -> ACK sent
    client.handle_event(TcpEvent::SynAckReceived).unwrap();
    assert_eq!(client.current(), TcpState::Established);
    
    // Server: receives ACK
    server.handle_event(TcpEvent::AckReceived).unwrap();
    assert_eq!(server.current(), TcpState::Established);
    
    // Both sides established!
    assert!(client.current().is_established());
    assert!(server.current().is_established());
}

/// Test TCP connection close (4-way teardown)
#[test]
fn test_tcp_4way_close() {
    // Start with established connection
    let client = TcpStateMachine::new(TcpState::Established);
    let server = TcpStateMachine::new(TcpState::Established);
    
    // Client initiates close -> FIN sent
    client.handle_event(TcpEvent::Close).unwrap();
    assert_eq!(client.current(), TcpState::FinWait1);
    
    // Server receives FIN -> ACK sent
    server.handle_event(TcpEvent::FinReceived).unwrap();
    assert_eq!(server.current(), TcpState::CloseWait);
    
    // Client receives ACK
    client.handle_event(TcpEvent::AckReceived).unwrap();
    assert_eq!(client.current(), TcpState::FinWait2);
    
    // Server closes -> FIN sent
    server.handle_event(TcpEvent::Close).unwrap();
    assert_eq!(server.current(), TcpState::LastAck);
    
    // Client receives FIN -> ACK sent
    client.handle_event(TcpEvent::FinReceived).unwrap();
    assert_eq!(client.current(), TcpState::TimeWait);
    
    // Server receives ACK -> closed
    server.handle_event(TcpEvent::FinAckReceived).unwrap();
    assert_eq!(server.current(), TcpState::Closed);
    
    // After TIME_WAIT, client closes
    // (In real system, this happens after 2*MSL timeout)
    client.transition(TcpState::Closed).unwrap();
    assert_eq!(client.current(), TcpState::Closed);
}

/// Test invalid state transitions
#[test]
fn test_tcp_invalid_transitions() {
    let state = TcpStateMachine::new(TcpState::Closed);
    
    // Cannot go directly from CLOSED to ESTABLISHED
    assert!(state.transition(TcpState::Established).is_err());
    
    // Cannot receive data in CLOSED state
    assert!(!state.current().can_recv());
    assert!(!state.current().can_send());
}

/// Test simultaneous open
#[test]
fn test_tcp_simultaneous_open() {
    let peer1 = TcpStateMachine::new(TcpState::Closed);
    let peer2 = TcpStateMachine::new(TcpState::Closed);
    
    // Both call connect() simultaneously
    peer1.handle_event(TcpEvent::Connect).unwrap();
    peer2.handle_event(TcpEvent::Connect).unwrap();
    
    assert_eq!(peer1.current(), TcpState::SynSent);
    assert_eq!(peer2.current(), TcpState::SynSent);
    
    // Both receive SYN (cross SYNs)
    peer1.handle_event(TcpEvent::SynReceived).unwrap();
    peer2.handle_event(TcpEvent::SynReceived).unwrap();
    
    assert_eq!(peer1.current(), TcpState::SynReceived);
    assert_eq!(peer2.current(), TcpState::SynReceived);
    
    // Both receive ACK
    peer1.handle_event(TcpEvent::AckReceived).unwrap();
    peer2.handle_event(TcpEvent::AckReceived).unwrap();
    
    // Both established
    assert!(peer1.current().is_established());
    assert!(peer2.current().is_established());
}

/// Test TCP reset
#[test]
fn test_tcp_reset() {
    let conn = TcpStateMachine::new(TcpState::Established);
    
    // Reset from any state goes to CLOSED
    conn.handle_event(TcpEvent::Reset).unwrap();
    assert_eq!(conn.current(), TcpState::Closed);
}

/// Run all TCP handshake tests
pub fn run_all_tests() -> (usize, usize) {
    let mut passed = 0;
    let mut failed = 0;
    
    // Run tests
    let tests = [
        ("3-way handshake (client)", test_tcp_3way_handshake_client as fn()),
        ("3-way handshake (server)", test_tcp_3way_handshake_server as fn()),
        ("3-way handshake (full)", test_tcp_3way_handshake_full as fn()),
        ("4-way close", test_tcp_4way_close as fn()),
        ("Invalid transitions", test_tcp_invalid_transitions as fn()),
        ("Simultaneous open", test_tcp_simultaneous_open as fn()),
        ("Reset", test_tcp_reset as fn()),
    ];
    
    crate::logger::info("\n╔══════════════════════════════════════════════════════════╗");
    crate::logger::info("║         TCP 3-Way Handshake Validation Tests            ║");
    crate::logger::info("╠══════════════════════════════════════════════════════════╣");
    
    for (name, test_fn) in tests.iter() {
        let result = core::panic::catch_unwind(core::panic::AssertUnwindSafe(|| {
            test_fn();
        }));
        
        match result {
            Ok(_) => {
                crate::logger::info(&alloc::format!("║ ✅ {:<52} ║", name));
                passed += 1;
            }
            Err(_) => {
                crate::logger::error(&alloc::format!("║ ❌ {:<52} ║", name));
                failed += 1;
            }
        }
    }
    
    crate::logger::info("╠══════════════════════════════════════════════════════════╣");
    crate::logger::info(&alloc::format!(
        "║ Results: {}/{} tests passed                              ║",
        passed,
        passed + failed
    ));
    crate::logger::info("╚══════════════════════════════════════════════════════════╝");
    
    (passed, failed)
}
