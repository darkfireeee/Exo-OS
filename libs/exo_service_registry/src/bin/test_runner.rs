//! Test Runner - Programme de test standalone pour exo_service_registry
//!
//! Ce programme teste toutes les fonctionnalités en conditions réelles sans
//! dépendre du framework de test standard Rust (qui nécessite std).

#![no_std]
#![no_main]
#![allow(internal_features)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::string::{String, ToString};
use alloc::format;

// Allocateur global minimal
use core::alloc::{GlobalAlloc, Layout};

struct DummyAlloc;

unsafe impl GlobalAlloc for DummyAlloc {
    unsafe fn alloc(&self, _layout: Layout) -> *mut u8 {
        core::ptr::null_mut()
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
static ALLOCATOR: DummyAlloc = DummyAlloc;

use exo_service_registry::{
    Registry, RegistryConfig,
    ServiceName, ServiceInfo, ServiceStatus,
    metrics::{MetricsExporter, MetricsFormat},
    versioning::{VersionManager, VersionedService, ServiceVersion},
    config::{SystemConfig, TomlParser},
    signals::{Signal, signal_flags, simulate_signal},
    threading::{ThreadSafeRegistry, RegistryPool},
    loadbalancer::{LoadBalancer, LoadBalancingStrategy, RegistryInstance},
};

#[cfg(feature = "ipc")]
use exo_service_registry::{
    daemon::RegistryDaemon,
    protocol::RegistryRequest,
};

use core::sync::atomic::Ordering;

/// Compteur de tests
static mut TESTS_RUN: usize = 0;
static mut TESTS_PASSED: usize = 0;
static mut TESTS_FAILED: usize = 0;

/// Macro assert custom pour no_std
macro_rules! test_assert {
    ($cond:expr, $msg:expr) => {
        unsafe {
            TESTS_RUN += 1;
        }
        if !($cond) {
            log_error(&alloc::format!("FAILED: {} - {}", stringify!($cond), $msg));
            unsafe {
                TESTS_FAILED += 1;
            }
        } else {
            unsafe {
                TESTS_PASSED += 1;
            }
        }
    };
}

macro_rules! test_assert_eq {
    ($left:expr, $right:expr) => {
        unsafe {
            TESTS_RUN += 1;
        }
        let left_val = $left;
        let right_val = $right;
        if left_val != right_val {
            log_error(&alloc::format!("FAILED: {} == {} (got {:?} != {:?})",
                stringify!($left), stringify!($right), left_val, right_val));
            unsafe {
                TESTS_FAILED += 1;
            }
        } else {
            unsafe {
                TESTS_PASSED += 1;
            }
        }
    };
}

/// Point d'entrée principal
#[no_mangle]
pub extern "C" fn main(_argc: isize, _argv: *const *const u8) -> isize {
    log_info("========================================");
    log_info("  exo_service_registry v0.4.0");
    log_info("  TEST RUNNER - Conditions Réelles");
    log_info("========================================");
    log_info("");

    // Test 1: Core Registry
    test_core_registry();

    // Test 2: Configuration System
    test_config_system();

    // Test 3: Signal Handlers
    test_signal_handlers();

    // Test 4: Multi-threading
    test_threading();

    // Test 5: Load Balancing
    test_load_balancing();

    // Test 6: Metrics Export
    test_metrics_export();

    // Test 7: Service Versioning
    test_service_versioning();

    #[cfg(feature = "ipc")]
    {
        // Test 8: IPC Communication
        test_ipc_communication();
    }

    // Résumé final
    print_summary();

    unsafe {
        if TESTS_FAILED == 0 {
            0 // Success
        } else {
            1 // Failure
        }
    }
}

/// Test 1: Core Registry avec Cache et Bloom
fn test_core_registry() {
    log_section("TEST 1: Core Registry");

    let config = RegistryConfig::new()
        .with_cache_size(100)
        .with_bloom_size(10_000);

    let mut registry = Registry::with_config(config);

    // Test registration
    for i in 0..10 {
        let name = ServiceName::new(&alloc::format!("test_service_{}", i)).unwrap();
        let info = ServiceInfo::new(&alloc::format!("/tmp/test_{}.sock", i));
        registry.register(name, info).unwrap();
    }

    let stats = registry.stats();
    test_assert_eq!(stats.active_services.load(Ordering::Relaxed), 10);
    test_assert_eq!(stats.total_registrations.load(Ordering::Relaxed), 10);

    // Test lookup
    for i in 0..10 {
        let name = ServiceName::new(&alloc::format!("test_service_{}", i)).unwrap();
        let info = registry.lookup(&name);
        test_assert!(info.is_some(), "Lookup should find registered service");
    }

    log_success("Core Registry: 10 services registered and looked up");
}

/// Test 2: Configuration System
fn test_config_system() {
    log_section("TEST 2: Configuration System");

    // Test default config
    let config = SystemConfig::default();
    test_assert_eq!(config.registry.cache_size, 100);

    // Test TOML parsing
    let toml = "[registry]\ncache_size = 500\n";
    let mut parser = TomlParser::new(toml);
    let parsed = parser.parse();
    test_assert!(parsed.is_ok(), "TOML parsing should succeed");

    if let Ok(config) = parsed {
        test_assert_eq!(config.registry.cache_size, 500);
    }

    log_success("Configuration: TOML parsing and defaults OK");
}

/// Test 3: Signal Handlers
fn test_signal_handlers() {
    log_section("TEST 3: Signal Handlers");

    let flags = signal_flags();

    // Test initial state
    test_assert!(!flags.should_shutdown(), "Should not shutdown initially");
    test_assert!(!flags.should_reload_config(), "Should not reload initially");

    // Simulate SIGTERM
    simulate_signal(Signal::SIGTERM);
    test_assert!(flags.should_shutdown(), "SIGTERM should set shutdown flag");

    // Simulate SIGHUP
    simulate_signal(Signal::SIGHUP);
    test_assert!(flags.should_reload_config(), "SIGHUP should set reload flag");

    // Clear flags
    flags.clear_shutdown();
    flags.clear_reload_config();
    test_assert!(!flags.should_shutdown(), "Shutdown flag should clear");

    log_success("Signal Handlers: SIGTERM, SIGHUP, flags OK");
}

/// Test 4: Multi-threading
fn test_threading() {
    log_section("TEST 4: Multi-threading");

    // ThreadSafeRegistry
    let registry = ThreadSafeRegistry::new();
    let name = ServiceName::new("thread_test").unwrap();
    let info = ServiceInfo::new("/tmp/thread_test.sock");

    registry.register(name.clone(), info).unwrap();
    let found = registry.lookup(&name);
    test_assert!(found.is_some(), "ThreadSafeRegistry lookup should work");

    // RegistryPool
    let pool = RegistryPool::new(4, RegistryConfig::new());
    for i in 0..20 {
        let name = ServiceName::new(&alloc::format!("pool_service_{}", i)).unwrap();
        let info = ServiceInfo::new(&alloc::format!("/tmp/pool_{}.sock", i));
        pool.register(name, info).unwrap();
    }

    test_assert_eq!(pool.size(), 4);
    test_assert_eq!(pool.total_services(), 20);

    log_success("Threading: ThreadSafeRegistry and RegistryPool OK");
}

/// Test 5: Load Balancing
fn test_load_balancing() {
    log_section("TEST 5: Load Balancing");

    // Test Round-Robin
    let mut lb_rr = LoadBalancer::new(LoadBalancingStrategy::RoundRobin);
    lb_rr.add_instance(RegistryInstance::new("instance1".into(), 1));
    lb_rr.add_instance(RegistryInstance::new("instance2".into(), 1));

    for i in 0..10 {
        let name = ServiceName::new(&alloc::format!("lb_service_{}", i)).unwrap();
        let info = ServiceInfo::new(&alloc::format!("/tmp/lb_{}.sock", i));
        lb_rr.register(name, info).unwrap();
    }

    test_assert_eq!(lb_rr.total_instances(), 2);
    test_assert_eq!(lb_rr.healthy_instances(), 2);

    // Test Weighted
    let mut lb_weighted = LoadBalancer::new(LoadBalancingStrategy::WeightedRoundRobin);
    lb_weighted.add_instance(RegistryInstance::new("heavy".into(), 80));
    lb_weighted.add_instance(RegistryInstance::new("light".into(), 20));

    test_assert_eq!(lb_weighted.total_instances(), 2);

    log_success("Load Balancing: RoundRobin and Weighted OK");
}

/// Test 6: Metrics Export
fn test_metrics_export() {
    log_section("TEST 6: Metrics Export");

    let mut registry = Registry::new();
    for i in 0..50 {
        let name = ServiceName::new(&alloc::format!("metric_{}", i)).unwrap();
        let info = ServiceInfo::new(&alloc::format!("/tmp/metric_{}.sock", i));
        registry.register(name.clone(), info).unwrap();
        let _ = registry.lookup(&name);
    }

    // Prometheus export
    let exporter = MetricsExporter::new(MetricsFormat::Prometheus);
    let prometheus = exporter.export(&registry.stats());
    test_assert!(prometheus.contains("exo_registry_lookups_total"),
        "Prometheus export should contain lookups metric");
    test_assert!(prometheus.contains("exo_registry_active_services 50"),
        "Prometheus should show 50 active services");

    // JSON export
    let json_exporter = MetricsExporter::new(MetricsFormat::Json);
    let json = json_exporter.export(&registry.stats());
    test_assert!(json.contains("\"total_lookups\""),
        "JSON export should contain total_lookups");

    log_success("Metrics: Prometheus and JSON export OK");
}

/// Test 7: Service Versioning
fn test_service_versioning() {
    log_section("TEST 7: Service Versioning");

    let mut version_mgr = VersionManager::new();
    let service_name = ServiceName::new("versioned_service").unwrap();

    // Register multiple versions
    let v1_0 = ServiceVersion::new(1, 0, 0);
    let v1_1 = ServiceVersion::new(1, 1, 0);
    let v2_0 = ServiceVersion::new(2, 0, 0);

    version_mgr.register(VersionedService::new(
        service_name.clone(),
        v1_0,
        ServiceInfo::new("/tmp/v1.0.sock"),
    )).unwrap();

    version_mgr.register(VersionedService::new(
        service_name.clone(),
        v1_1,
        ServiceInfo::new("/tmp/v1.1.sock"),
    )).unwrap();

    version_mgr.register(VersionedService::new(
        service_name.clone(),
        v2_0,
        ServiceInfo::new("/tmp/v2.0.sock"),
    )).unwrap();

    // Find compatible
    let required = ServiceVersion::new(1, 0, 0);
    let found = version_mgr.find_compatible(&service_name, &required);
    test_assert!(found.is_some(), "Should find compatible version");
    test_assert_eq!(found.unwrap().version, v1_1); // v1.1 is best match

    // Deprecate
    version_mgr.deprecate_version(&service_name, &v1_0).unwrap();
    test_assert_eq!(version_mgr.count_active_versions(&service_name), 2);

    log_success("Versioning: SemVer compatibility and deprecation OK");
}

/// Test 8: IPC Communication
#[cfg(feature = "ipc")]
fn test_ipc_communication() {
    log_section("TEST 8: IPC Communication");

    let registry = Box::new(Registry::new());
    let mut daemon = RegistryDaemon::with_registry(registry);

    // Test register via daemon
    let name = ServiceName::new("ipc_test").unwrap();
    let info = ServiceInfo::new("/tmp/ipc_test.sock");
    let req = RegistryRequest::register(name.clone(), info);
    let resp = daemon.handle_request(req);

    test_assert_eq!(resp.response_type,
        exo_service_registry::protocol::ResponseType::Ok);

    // Test lookup
    let lookup_req = RegistryRequest::lookup(name);
    let lookup_resp = daemon.handle_request(lookup_req);
    test_assert_eq!(lookup_resp.response_type,
        exo_service_registry::protocol::ResponseType::Found);

    // Test ping
    let ping = RegistryRequest::ping();
    let pong = daemon.handle_request(ping);
    test_assert_eq!(pong.response_type,
        exo_service_registry::protocol::ResponseType::Pong);

    log_success("IPC: Daemon request handling OK");
}

/// Affiche le résumé final
fn print_summary() {
    log_info("");
    log_info("========================================");
    log_info("  RÉSUMÉ DES TESTS");
    log_info("========================================");

    unsafe {
        let total = TESTS_RUN;
        let passed = TESTS_PASSED;
        let failed = TESTS_FAILED;

        log_info(&alloc::format!("Total:  {} tests", total));

        if failed == 0 {
            log_success(&alloc::format!("Passed: {} tests ✅", passed));
            log_info("Failed: 0 tests");
            log_info("");
            log_success("🎉 TOUS LES TESTS RÉUSSIS!");
        } else {
            log_info(&alloc::format!("Passed: {} tests", passed));
            log_error(&alloc::format!("Failed: {} tests ❌", failed));
            log_info("");
            log_error("❌ CERTAINS TESTS ONT ÉCHOUÉ");
        }
    }

    log_info("");
}

// Helpers de logging

fn log_section(msg: &str) {
    log_info("");
    log_info(&alloc::format!("━━━ {} ━━━", msg));
}

fn log_info(msg: &str) {
    #[cfg(feature = "std")]
    println!("{}", msg);

    #[cfg(not(feature = "std"))]
    {
        // Dans Exo-OS, utiliser exo_logger ou syscall write
        let _ = msg;
    }
}

fn log_success(msg: &str) {
    #[cfg(feature = "std")]
    println!("✅ {}", msg);

    #[cfg(not(feature = "std"))]
    {
        let _ = msg;
    }
}

fn log_error(msg: &str) {
    #[cfg(feature = "std")]
    eprintln!("❌ {}", msg);

    #[cfg(not(feature = "std"))]
    {
        let _ = msg;
    }
}

/// Panic handler pour no_std
#[cfg(not(feature = "std"))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

/// Entry point stub pour no_std
#[cfg(not(feature = "std"))]
#[lang = "eh_personality"]
extern "C" fn eh_personality() {}
