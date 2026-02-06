//! IPC Runtime Tests
//!
//! Production-ready IPC tests that can be executed at runtime

use super::*;
use super::core::{MpmcRing, Endpoint, EndpointFlags};
use super::named::{create_channel, open_channel, ChannelType, ChannelPermissions, ChannelFlags};
use alloc::vec::Vec;

/// Test result
pub struct TestResult {
    pub name: &'static str,
    pub passed: bool,
    pub message: &'static str,
}

impl TestResult {
    pub fn pass(name: &'static str) -> Self {
        Self { name, passed: true, message: "OK" }
    }

    pub fn fail(name: &'static str, message: &'static str) -> Self {
        Self { name, passed: false, message }
    }
}

/// Run all IPC tests and return results
pub fn run_all_ipc_tests() -> Vec<TestResult> {
    let mut results = Vec::new();

    log::info!("========================================");
    log::info!("  IPC RUNTIME TEST SUITE");
    log::info!("========================================");

    // Test 1: Basic inline send/recv
    results.push(test_basic_inline());

    // Test 2: Multiple messages
    results.push(test_multiple_messages());

    // Test 3: Ring full handling
    results.push(test_ring_full());

    // Test 4: Endpoint bidirectional
    results.push(test_endpoint_bidir());

    // Test 5: Named channels
    results.push(test_named_channels());

    // Test 6: Performance benchmark
    results.push(test_performance());

    // Test 7: Max inline size
    results.push(test_max_inline());

    // Print summary
    let passed = results.iter().filter(|r| r.passed).count();
    let failed = results.iter().filter(|r| !r.passed).count();

    log::info!("========================================");
    log::info!("  TEST SUMMARY");
    log::info!("  Total: {}, Passed: {}, Failed: {}", results.len(), passed, failed);
    log::info!("========================================");

    for result in &results {
        if result.passed {
            log::info!("✅ {}: {}", result.name, result.message);
        } else {
            log::error!("❌ {}: {}", result.name, result.message);
        }
    }

    results
}

// =============================================================================
// INDIVIDUAL TESTS
// =============================================================================

fn test_basic_inline() -> TestResult {
    let ring = MpmcRing::new(64);

    // Send
    let data = b"Hello IPC!";
    if let Err(_) = ring.try_send_inline(data) {
        return TestResult::fail("basic_inline", "Send failed");
    }

    // Receive
    let mut buffer = [0u8; 64];
    match ring.try_recv(&mut buffer) {
        Ok(size) => {
            if size != data.len() {
                return TestResult::fail("basic_inline", "Size mismatch");
            }
            if &buffer[..size] != data {
                return TestResult::fail("basic_inline", "Data mismatch");
            }
            TestResult::pass("basic_inline")
        }
        Err(_) => TestResult::fail("basic_inline", "Receive failed"),
    }
}

fn test_multiple_messages() -> TestResult {
    let ring = MpmcRing::new(64);

    let messages = [b"Msg1", b"Msg2", b"Msg3"];

    // Send all
    for msg in &messages {
        if let Err(_) = ring.try_send_inline(*msg) {
            return TestResult::fail("multiple_messages", "Send failed");
        }
    }

    // Receive all
    for expected in &messages {
        let mut buffer = [0u8; 64];
        match ring.try_recv(&mut buffer) {
            Ok(size) => {
                if &buffer[..size] != *expected {
                    return TestResult::fail("multiple_messages", "Data mismatch");
                }
            }
            Err(_) => return TestResult::fail("multiple_messages", "Receive failed"),
        }
    }

    TestResult::pass("multiple_messages")
}

fn test_ring_full() -> TestResult {
    let ring = MpmcRing::new(4);

    // Fill ring (capacity-1 because of MPMC design)
    for _ in 0..4 {
        if let Err(_) = ring.try_send_inline(b"fill") {
            // Expected to fill
            break;
        }
    }

    // Next send should fail
    match ring.try_send_inline(b"overflow") {
        Err(_) => TestResult::pass("ring_full"),
        Ok(_) => TestResult::fail("ring_full", "Should fail when full"),
    }
}

fn test_endpoint_bidir() -> TestResult {
    let ring = alloc::sync::Arc::new(MpmcRing::new(64));
    let wait_queue = alloc::sync::Arc::new(WaitQueue::new());
    let channel = super::core::ChannelHandle(1);

    let ep1 = Endpoint::new(channel, ring.clone(), wait_queue.clone(), EndpointFlags::BIDIRECTIONAL);
    let ep2 = Endpoint::new(channel, ring.clone(), wait_queue.clone(), EndpointFlags::BIDIRECTIONAL);

    // Send from ep1
    if let Err(_) = ep1.try_send(b"test") {
        return TestResult::fail("endpoint_bidir", "Send failed");
    }

    // Receive on ep2
    let mut buffer = [0u8; 64];
    match ep2.try_recv(&mut buffer) {
        Ok(size) => {
            if &buffer[..size] != b"test" {
                return TestResult::fail("endpoint_bidir", "Data mismatch");
            }
            TestResult::pass("endpoint_bidir")
        }
        Err(_) => TestResult::fail("endpoint_bidir", "Receive failed"),
    }
}

fn test_named_channels() -> TestResult {
    let name = "/test/runtime_channel";

    // Create
    let sender = match create_channel(
        name,
        ChannelType::Pipe,
        ChannelPermissions::public(),
        ChannelFlags::new(0),
    ) {
        Ok(ch) => ch,
        Err(_) => return TestResult::fail("named_channels", "Create failed"),
    };

    // Open
    let receiver = match open_channel(name, true, false) {
        Ok(ch) => ch,
        Err(_) => return TestResult::fail("named_channels", "Open failed"),
    };

    // Send
    if let Err(_) = sender.send(b"named test") {
        return TestResult::fail("named_channels", "Send failed");
    }

    // Receive
    match receiver.recv() {
        Ok(data) => {
            if &data[..] != b"named test" {
                return TestResult::fail("named_channels", "Data mismatch");
            }
            TestResult::pass("named_channels")
        }
        Err(_) => TestResult::fail("named_channels", "Receive failed"),
    }
}

fn test_performance() -> TestResult {
    use crate::time::tsc;

    let ring = MpmcRing::new(1024);
    let msg = b"Perf test message";
    let iterations = 1000;

    let start = tsc::read_tsc();

    for _ in 0..iterations {
        if let Err(_) = ring.try_send_inline(msg) {
            return TestResult::fail("performance", "Send failed");
        }
        let mut buffer = [0u8; 64];
        if let Err(_) = ring.try_recv(&mut buffer) {
            return TestResult::fail("performance", "Receive failed");
        }
    }

    let end = tsc::read_tsc();
    let avg_cycles = (end - start) / iterations;

    log::info!("IPC Performance: {} cycles/operation (target: <100)", avg_cycles);

    if avg_cycles < 200 {
        TestResult::pass("performance")
    } else {
        TestResult::fail("performance", "Too slow")
    }
}

fn test_max_inline() -> TestResult {
    let ring = MpmcRing::new(64);

    // Max inline is 56 bytes
    let msg: Vec<u8> = (0..56).map(|i| i as u8).collect();

    if let Err(_) = ring.try_send_inline(&msg) {
        return TestResult::fail("max_inline", "Send failed");
    }

    let mut buffer = [0u8; 128];
    match ring.try_recv(&mut buffer) {
        Ok(size) => {
            if size != 56 {
                return TestResult::fail("max_inline", "Size incorrect");
            }
            if &buffer[..size] != &msg[..] {
                return TestResult::fail("max_inline", "Data mismatch");
            }
            TestResult::pass("max_inline")
        }
        Err(_) => TestResult::fail("max_inline", "Receive failed"),
    }
}
