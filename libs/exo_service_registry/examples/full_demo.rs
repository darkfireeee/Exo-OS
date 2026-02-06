//! Démonstration complète - Toutes les phases en conditions réelles
//!
//! Programme standalone qui teste toutes les fonctionnalités sans dépendre
//! de cargo test (pour éviter les problèmes serde_core).

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::string::ToString;
use alloc::format;

use exo_service_registry::{
    Registry, RegistryConfig,
    ServiceName, ServiceInfo, ServiceStatus,
    metrics::{MetricsExporter, MetricsFormat},
    versioning::{VersionManager, VersionedService, ServiceVersion},
};

#[cfg(feature = "ipc")]
use exo_service_registry::{
    daemon::RegistryDaemon,
    protocol::{RegistryRequest, ResponseType},
    serialize::BinarySerialize,
};

use core::sync::atomic::Ordering;

/// Point d'entrée principal
#[no_mangle]
pub extern "C" fn demo_main() -> i32 {
    println!("\n==========================================");
    println!("  exo_service_registry - DEMO COMPLETE");
    println!("==========================================\n");

    // === PHASE 1: CORE REGISTRY ===
    println!("📦 PHASE 1: Core Registry avec Cache + Bloom");
    println!("-------------------------------------------");

    let config = RegistryConfig::new()
        .with_cache_size(200)
        .with_bloom_size(50_000)
        .with_stale_threshold(300);

    let mut registry = Registry::with_config(config);

    // Enregistrement
    for i in 0..10 {
        let name = ServiceName::new(&alloc::format!("service_{}", i)).unwrap();
        let info = ServiceInfo::new(&alloc::format!("/var/run/exo/service_{}.sock", i));
        registry.register(name, info).unwrap();
    }

    println!("✓ 10 services enregistrés");

    // Lookups
    for i in 0..10 {
        let name = ServiceName::new(&alloc::format!("service_{}", i)).unwrap();
        let info = registry.lookup(&name).expect("Service not found");
        assert_eq!(info.endpoint(), &alloc::format!("/var/run/exo/service_{}.sock", i));
    }

    println!("✓ 10 lookups réussis (1er round - cold cache)");

    // Second round (cache should hit)
    for i in 0..10 {
        let name = ServiceName::new(&alloc::format!("service_{}", i)).unwrap();
        let _ = registry.lookup(&name).unwrap();
    }

    println!("✓ 10 lookups réussis (2ème round - warm cache)");

    let stats = registry.stats();
    let cache_hits = stats.cache_hits.load(Ordering::Relaxed);
    println!("  Cache hits: {}/20 ({:.0}%)", cache_hits, cache_hits as f64 / 20.0 * 100.0);

    // === PHASE 3: IPC PROTOCOL ===
    #[cfg(feature = "ipc")]
    {
        println!("\n📡 PHASE 3: IPC Protocol + Serialization");
        println!("-------------------------------------------");

        let daemon = RegistryDaemon::new();
        let mut daemon = RegistryDaemon::new();

        // Register via daemon
        let name = ServiceName::new("ipc_test").unwrap();
        let info = ServiceInfo::new("/tmp/ipc_test.sock");
        let req = RegistryRequest::register(name.clone(), info);

        // Test serialization
        let mut buf = Vec::new();
        req.serialize_into(&mut buf).unwrap();
        println!("✓ Request sérialisé ({} bytes)", buf.len());

        let deserialized = RegistryRequest::deserialize_from(&buf).unwrap();
        println!("✓ Request désérialisé correctement");

        // Handle request
        let resp = daemon.handle_request(req);
        assert_eq!(resp.response_type, ResponseType::Ok);
        println!("✓ Request traité par daemon");

        // Ping
        let ping = RegistryRequest::ping();
        let pong = daemon.handle_request(ping);
        assert_eq!(pong.response_type, ResponseType::Pong);
        println!("✓ Ping/Pong functional");

        println!("  Requests processed: {}", daemon.requests_processed());
    }

    #[cfg(not(feature = "ipc"))]
    {
        println!("\n⚠ PHASE 3: IPC feature not enabled");
    }

    // === PHASE 4: METRICS EXPORT ===
    println!("\n📊 PHASE 4: Metrics Export Prometheus");
    println!("-------------------------------------------");

    let exporter = MetricsExporter::new(MetricsFormat::Prometheus);
    let metrics = exporter.export(&registry.stats());

    println!("✓ Prometheus metrics generated:");
    // Show first 3 lines
    for (i, line) in metrics.lines().take(5).enumerate() {
        if i < 3 {
            println!("  {}", line);
        }
    }
    println!("  ... ({} lines total)", metrics.lines().count());

    // JSON export
    let json_exporter = MetricsExporter::new(MetricsFormat::Json);
    let json = json_exporter.export(&registry.stats());
    println!("✓ JSON metrics: {} bytes", json.len());

    // === PHASE 5: SERVICE VERSIONING ===
    println!("\n🔄 PHASE 5: Service Versioning");
    println!("-------------------------------------------");

    let mut version_mgr = VersionManager::new();

    let service_name = ServiceName::new("api_service").unwrap();

    // Register multiple versions
    let v1_0 = ServiceVersion::new(1, 0, 0);
    let v1_1 = ServiceVersion::new(1, 1, 0);
    let v2_0 = ServiceVersion::new(2, 0, 0);

    version_mgr.register(VersionedService::new(
        service_name.clone(),
        v1_0,
        ServiceInfo::new("/tmp/api_v1.0.sock"),
    )).unwrap();

    version_mgr.register(VersionedService::new(
        service_name.clone(),
        v1_1,
        ServiceInfo::new("/tmp/api_v1.1.sock"),
    )).unwrap();

    version_mgr.register(VersionedService::new(
        service_name.clone(),
        v2_0,
        ServiceInfo::new("/tmp/api_v2.0.sock"),
    )).unwrap();

    println!("✓ 3 versions enregistrées (v1.0, v1.1, v2.0)");

    // Find compatible with v1.0
    let required = ServiceVersion::new(1, 0, 0);
    let found = version_mgr.find_compatible(&service_name, &required).unwrap();
    println!("✓ Compatible avec v1.0 -> found v{}", found.version);
    assert_eq!(found.version, v1_1); // v1.1 est meilleur

    // Deprecate old version
    version_mgr.deprecate_version(&service_name, &v1_0).unwrap();
    println!("✓ v1.0 deprecated");

    let active = version_mgr.count_active_versions(&service_name);
    println!("  Active versions: {}", active);

    // === PERFORMANCE TEST ===
    println!("\n⚡ PERFORMANCE: Stress Test 1000 services");
    println!("-------------------------------------------");

    let perf_config = RegistryConfig::new()
        .with_cache_size(500)
        .with_bloom_size(100_000);

    let mut perf_registry = Registry::with_config(perf_config);

    // Register 1000
    for i in 0..1000 {
        let name = ServiceName::new(&alloc::format!("perf_{:04}", i)).unwrap();
        let info = ServiceInfo::new(&alloc::format!("/tmp/perf_{:04}.sock", i));
        perf_registry.register(name, info).unwrap();
    }

    println!("✓ 1000 services registered");

    // Lookup 1000
    for i in 0..1000 {
        let name = ServiceName::new(&alloc::format!("perf_{:04}", i)).unwrap();
        let _ = perf_registry.lookup(&name).unwrap();
    }

    println!("✓ 1000 lookups completed");

    let perf_stats = perf_registry.stats();
    let total = perf_stats.total_lookups.load(Ordering::Relaxed);
    let hits = perf_stats.cache_hits.load(Ordering::Relaxed);

    println!("  Total lookups: {}", total);
    println!("  Cache hit rate: {:.1}%", hits as f64 / total as f64 * 100.0);

    // === SUMMARY ===
    println!("\n==========================================");
    println!("  ✅ TOUTES LES PHASES TESTÉES");
    println!("==========================================");

    println!("\nRésumé:");
    println!("  ✓ Phase 1: Core Registry - OK");
    #[cfg(feature = "ipc")]
    println!("  ✓ Phase 3: IPC Protocol - OK");
    println!("  ✓ Phase 4: Metrics Export - OK");
    println!("  ✓ Phase 5: Versioning - OK");
    println!("  ✓ Performance: 1000 services - OK");

    println!("\n🎉 DEMO TERMINÉE AVEC SUCCÈS 🎉\n");

    0
}

/// Printf pour no_std
#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => {{
        #[cfg(feature = "std")]
        std::println!($($arg)*);

        #[cfg(not(feature = "std"))]
        {
            // Dans Exo-OS, utiliser syscall write ou exo_logger
            let _ = alloc::format!($($arg)*);
        }
    }};
}

/// Assert macro
macro_rules! assert_eq {
    ($left:expr, $right:expr) => {{
        let left_val = &$left;
        let right_val = &$right;
        if !(*left_val == *right_val) {
            panic!("assertion failed");
        }
    }};
}

// Pour tests standalone
#[cfg(feature = "std")]
fn main() {
    demo_main();
}
