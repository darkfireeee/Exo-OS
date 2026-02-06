//! Test Runner avec std - Tests en conditions réelles
//!
//! Programme de test complet qui utilise std pour pouvoir s'exécuter
//! normalement et tester toutes les fonctionnalités.

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

/// Statistiques de tests
struct TestStats {
    total: usize,
    passed: usize,
    failed: usize,
}

impl TestStats {
    fn new() -> Self {
        Self {
            total: 0,
            passed: 0,
            failed: 0,
        }
    }

    fn run_test(&mut self, name: &str, test_fn: fn() -> Result<(), String>) {
        self.total += 1;
        print!("  Testing: {} ... ", name);
        match test_fn() {
            Ok(_) => {
                self.passed += 1;
                println!("✅ OK");
            }
            Err(msg) => {
                self.failed += 1;
                println!("❌ FAILED: {}", msg);
            }
        }
    }
}

fn main() {
    println!("========================================");
    println!("  exo_service_registry v0.4.0");
    println!("  TEST RUNNER - Conditions Réelles");
    println!("========================================");
    println!();

    let mut stats = TestStats::new();

    // Test 1: Core Registry
    println!("━━━ TEST 1: Core Registry ━━━");
    stats.run_test("registry_registration", test_registry_registration);
    stats.run_test("registry_lookup", test_registry_lookup);
    stats.run_test("registry_stats", test_registry_stats);
    println!();

    // Test 2: Configuration
    println!("━━━ TEST 2: Configuration System ━━━");
    stats.run_test("config_defaults", test_config_defaults);
    stats.run_test("config_toml_parsing", test_config_toml_parsing);
    println!();

    // Test 3: Signals
    println!("━━━ TEST 3: Signal Handlers ━━━");
    stats.run_test("signal_flags", test_signal_flags);
    stats.run_test("signal_simulation", test_signal_simulation);
    println!();

    // Test 4: Threading
    println!("━━━ TEST 4: Multi-threading ━━━");
    stats.run_test("thread_safe_registry", test_thread_safe_registry);
    stats.run_test("registry_pool", test_registry_pool);
    println!();

    // Test 5: Load Balancing
    println!("━━━ TEST 5: Load Balancing ━━━");
    stats.run_test("load_balancer_round_robin", test_load_balancer_round_robin);
    stats.run_test("load_balancer_weighted", test_load_balancer_weighted);
    println!();

    // Test 6: Metrics
    println!("━━━ TEST 6: Metrics Export ━━━");
    stats.run_test("metrics_prometheus", test_metrics_prometheus);
    stats.run_test("metrics_json", test_metrics_json);
    println!();

    // Test 7: Versioning
    println!("━━━ TEST 7: Service Versioning ━━━");
    stats.run_test("version_compatibility", test_version_compatibility);
    stats.run_test("version_deprecation", test_version_deprecation);
    println!();

    #[cfg(feature = "ipc")]
    {
        println!("━━━ TEST 8: IPC Communication ━━━");
        stats.run_test("ipc_daemon_register", test_ipc_daemon_register);
        stats.run_test("ipc_daemon_lookup", test_ipc_daemon_lookup);
        stats.run_test("ipc_daemon_ping", test_ipc_daemon_ping);
        println!();
    }

    // Résumé
    println!("========================================");
    println!("  RÉSUMÉ DES TESTS");
    println!("========================================");
    println!("Total:  {} tests", stats.total);

    if stats.failed == 0 {
        println!("Passed: {} tests ✅", stats.passed);
        println!("Failed: 0 tests");
        println!();
        println!("🎉 TOUS LES TESTS RÉUSSIS!");
    } else {
        println!("Passed: {} tests", stats.passed);
        println!("Failed: {} tests ❌", stats.failed);
        println!();
        println!("❌ CERTAINS TESTS ONT ÉCHOUÉ");
        std::process::exit(1);
    }
}

// ============================================================================
// TEST 1: Core Registry
// ============================================================================

fn test_registry_registration() -Result<(), String> {
    let mut registry = Registry::new();

    for i in 0..10 {
        let name = ServiceName::new(&format!("service_{}", i))
            .map_err(|e| format!("{:?}", e))?;
        let info = ServiceInfo::new(&format!("/tmp/service_{}.sock", i));
        registry.register(name, info)
            .map_err(|e| format!("{:?}", e))?;
    }

    let stats = registry.stats();
    if stats.active_services.load(Ordering::Relaxed) != 10 {
        return Err("Expected 10 active services".into());
    }

    Ok(())
}

fn test_registry_lookup() -> Result<(), String> {
    let mut registry = Registry::new();

    let name = ServiceName::new("test_lookup").unwrap();
    let info = ServiceInfo::new("/tmp/test_lookup.sock");
    registry.register(name.clone(), info).unwrap();

    let found = registry.lookup(&name);
    if found.is_none() {
        return Err("Lookup should find registered service".into());
    }

    if found.unwrap().endpoint() != "/tmp/test_lookup.sock" {
        return Err("Endpoint mismatch".into());
    }

    Ok(())
}

fn test_registry_stats() -> Result<(), String> {
    let mut registry = Registry::new();

    for i in 0..5 {
        let name = ServiceName::new(&format!("stats_{}", i)).unwrap();
        let info = ServiceInfo::new(&format!("/tmp/stats_{}.sock", i));
        registry.register(name.clone(), info).unwrap();
        let _ = registry.lookup(&name);
    }

    let stats = registry.stats();
    if stats.total_lookups.load(Ordering::Relaxed) != 5 {
        return Err(format!("Expected 5 lookups, got {}",
            stats.total_lookups.load(Ordering::Relaxed)));
    }

    Ok(())
}

// ============================================================================
// TEST 2: Configuration
// ============================================================================

fn test_config_defaults() -> Result<(), String> {
    let config = SystemConfig::default();

    if config.registry.cache_size != 100 {
        return Err(format!("Expected cache_size 100, got {}", config.registry.cache_size));
    }

    Ok(())
}

fn test_config_toml_parsing() -> Result<(), String> {
    let toml = "[registry]\ncache_size = 500\nbloom_size = 50000\n";
    let mut parser = TomlParser::new(toml);
    let config = parser.parse()
        .map_err(|e| format!("Parse error: {}", e))?;

    if config.registry.cache_size != 500 {
        return Err(format!("Expected 500, got {}", config.registry.cache_size));
    }

    if config.registry.bloom_size != 50000 {
        return Err(format!("Expected 50000, got {}", config.registry.bloom_size));
    }

    Ok(())
}

// ============================================================================
// TEST 3: Signals
// ============================================================================

fn test_signal_flags() -> Result<(), String> {
    let flags = signal_flags();

    // Reset au début
    flags.clear_shutdown();
    flags.clear_reload_config();

    if flags.should_shutdown() {
        return Err("Should not shutdown initially".into());
    }

    if flags.should_reload_config() {
        return Err("Should not reload initially".into());
    }

    Ok(())
}

fn test_signal_simulation() -> Result<(), String> {
    let flags = signal_flags();
    flags.clear_shutdown();
    flags.clear_reload_config();

    simulate_signal(Signal::SIGTERM);
    if !flags.should_shutdown() {
        return Err("SIGTERM should set shutdown flag".into());
    }

    simulate_signal(Signal::SIGHUP);
    if !flags.should_reload_config() {
        return Err("SIGHUP should set reload flag".into());
    }

    flags.clear_shutdown();
    flags.clear_reload_config();

    Ok(())
}

// ============================================================================
// TEST 4: Threading
// ============================================================================

fn test_thread_safe_registry() -> Result<(), String> {
    let registry = ThreadSafeRegistry::new();

    let name = ServiceName::new("thread_test").unwrap();
    let info = ServiceInfo::new("/tmp/thread_test.sock");

    registry.register(name.clone(), info)
        .map_err(|e| format!("{:?}", e))?;

    let found = registry.lookup(&name);
    if found.is_none() {
        return Err("ThreadSafeRegistry lookup failed".into());
    }

    Ok(())
}

fn test_registry_pool() -> Result<(), String> {
    let pool = RegistryPool::new(4, RegistryConfig::new());

    for i in 0..20 {
        let name = ServiceName::new(&format!("pool_{}",  i)).unwrap();
        let info = ServiceInfo::new(&format!("/tmp/pool_{}.sock", i));
        pool.register(name, info).unwrap();
    }

    if pool.size() != 4 {
        return Err(format!("Expected pool size 4, got {}", pool.size()));
    }

    if pool.total_services() != 20 {
        return Err(format!("Expected 20 services, got {}", pool.total_services()));
    }

    Ok(())
}

// ============================================================================
// TEST 5: Load Balancing
// ============================================================================

fn test_load_balancer_round_robin() -> Result<(), String> {
    let mut lb = LoadBalancer::new(LoadBalancingStrategy::RoundRobin);
    lb.add_instance(RegistryInstance::new("instance1".into(), 1));
    lb.add_instance(RegistryInstance::new("instance2".into(), 1));

    for i in 0..10 {
        let name = ServiceName::new(&format!("lb_{}", i)).unwrap();
        let info = ServiceInfo::new(&format!("/tmp/lb_{}.sock", i));
        lb.register(name, info).unwrap();
    }

    if lb.total_instances() != 2 {
        return Err(format!("Expected 2 instances, got {}", lb.total_instances()));
    }

    if lb.healthy_instances() != 2 {
        return Err(format!("Expected 2 healthy, got {}", lb.healthy_instances()));
    }

    Ok(())
}

fn test_load_balancer_weighted() -> Result<(), String> {
    let mut lb = LoadBalancer::new(LoadBalancingStrategy::WeightedRoundRobin);
    lb.add_instance(RegistryInstance::new("heavy".into(), 80));
    lb.add_instance(RegistryInstance::new("light".into(), 20));

    if lb.total_instances() != 2 {
        return Err(format!("Expected 2 instances, got {}", lb.total_instances()));
    }

    Ok(())
}

// ============================================================================
// TEST 6: Metrics
// ============================================================================

fn test_metrics_prometheus() -> Result<(), String> {
    let mut registry = Registry::new();

    for i in 0..10 {
        let name = ServiceName::new(&format!("metric_{}", i)).unwrap();
        let info = ServiceInfo::new(&format!("/tmp/metric_{}.sock", i));
        registry.register(name.clone(), info).unwrap();
        let _ = registry.lookup(&name);
    }

    let exporter = MetricsExporter::new(MetricsFormat::Prometheus);
    let output = exporter.export(&registry.stats());

    if !output.contains("exo_registry_lookups_total") {
        return Err("Prometheus export missing lookups metric".into());
    }

    if !output.contains("exo_registry_active_services 10") {
        return Err("Prometheus export incorrect active services".into());
    }

    Ok(())
}

fn test_metrics_json() -> Result<(), String> {
    let registry = Registry::new();
    let exporter = MetricsExporter::new(MetricsFormat::Json);
    let output = exporter.export(&registry.stats());

    if !output.contains("\"total_lookups\"") {
        return Err("JSON export missing total_lookups".into());
    }

    Ok(())
}

// ============================================================================
// TEST 7: Versioning
// ============================================================================

fn test_version_compatibility() -> Result<(), String> {
    let mut mgr = VersionManager::new();
    let name = ServiceName::new("versioned").unwrap();

    let v1_0 = ServiceVersion::new(1, 0, 0);
    let v1_1 = ServiceVersion::new(1, 1, 0);

    mgr.register(VersionedService::new(
        name.clone(), v1_0, ServiceInfo::new("/tmp/v1.0.sock")
    )).unwrap();

    mgr.register(VersionedService::new(
        name.clone(), v1_1, ServiceInfo::new("/tmp/v1.1.sock")
    )).unwrap();

    let required = ServiceVersion::new(1, 0, 0);
    let found = mgr.find_compatible(&name, &required);

    if found.is_none() {
        return Err("Should find compatible version".into());
    }

    if found.unwrap().version != v1_1 {
        return Err("Should find v1.1 as best match".into());
    }

    Ok(())
}

fn test_version_deprecation() -> Result<(), String> {
    let mut mgr = VersionManager::new();
    let name = ServiceName::new("versioned2").unwrap();

    let v1_0 = ServiceVersion::new(1, 0, 0);
    let v1_1 = ServiceVersion::new(1, 1, 0);

    mgr.register(VersionedService::new(
        name.clone(), v1_0, ServiceInfo::new("/tmp/v1.0.sock")
    )).unwrap();

    mgr.register(VersionedService::new(
        name.clone(), v1_1, ServiceInfo::new("/tmp/v1.1.sock")
    )).unwrap();

    mgr.deprecate_version(&name, &v1_0).unwrap();

    if mgr.count_active_versions(&name) != 1 {
        return Err(format!("Expected 1 active version, got {}",
            mgr.count_active_versions(&name)));
    }

    Ok(())
}

// ============================================================================
// TEST 8: IPC (si feature activée)
// ============================================================================

#[cfg(feature = "ipc")]
fn test_ipc_daemon_register() -> Result<(), String> {
    let registry = Box::new(Registry::new());
    let mut daemon = RegistryDaemon::with_registry(registry);

    let name = ServiceName::new("ipc_test").unwrap();
    let info = ServiceInfo::new("/tmp/ipc_test.sock");
    let req = RegistryRequest::register(name, info);
    let resp = daemon.handle_request(req);

    if resp.response_type != exo_service_registry::protocol::ResponseType::Ok {
        return Err("Register should return Ok".into());
    }

    Ok(())
}

#[cfg(feature = "ipc")]
fn test_ipc_daemon_lookup() -> Result<(), String> {
    let registry = Box::new(Registry::new());
    let mut daemon = RegistryDaemon::with_registry(registry);

    let name = ServiceName::new("ipc_lookup").unwrap();
    let info = ServiceInfo::new("/tmp/ipc_lookup.sock");
    let reg_req = RegistryRequest::register(name.clone(), info);
    daemon.handle_request(reg_req);

    let lookup_req = RegistryRequest::lookup(name);
    let lookup_resp = daemon.handle_request(lookup_req);

    if lookup_resp.response_type != exo_service_registry::protocol::ResponseType::Found {
        return Err("Lookup should return Found".into());
    }

    Ok(())
}

#[cfg(feature = "ipc")]
fn test_ipc_daemon_ping() -> Result<(), String> {
    let registry = Box::new(Registry::new());
    let mut daemon = RegistryDaemon::with_registry(registry);

    let ping = RegistryRequest::ping();
    let pong = daemon.handle_request(ping);

    if pong.response_type != exo_service_registry::protocol::ResponseType::Pong {
        return Err("Ping should return Pong".into());
    }

    Ok(())
}
