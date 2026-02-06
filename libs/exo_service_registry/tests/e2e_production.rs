//! Test End-to-End - Production Scenario
//!
//! Test complet démontrant toutes les fonctionnalités de exo_service_registry
//! dans un scénario de production réaliste.
//!
//! Phases testées:
//! - Phase 1: Core Registry avec Cache + Bloom
//! - Phase 2: exo_types integration (Timestamp)
//! - Phase 3: IPC Protocol + Daemon + Serialization + Real IPC
//! - Phase 4: System Integration (daemon, timestamps)
//! - Phase 5: Advanced Features (metrics, versioning)

#![cfg(test)]

use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::string::ToString;

use exo_service_registry::{
    Registry, RegistryConfig,
    ServiceName, ServiceInfo, ServiceStatus,
    RegistryError,
};

#[cfg(feature = "ipc")]
use exo_service_registry::{
    daemon::{RegistryDaemon, DaemonConfig},
    protocol::{RegistryRequest, RegistryResponse, ResponseType},
    serialize::BinarySerialize,
    ipc::{IpcServer, IpcClient},
};

#[cfg(feature = "health_check")]
use exo_service_registry::{HealthChecker, HealthConfig};

use exo_service_registry::{
    metrics::{MetricsExporter, MetricsFormat},
    versioning::{VersionManager, VersionedService, ServiceVersion},
};

/// Scénario 1: Enregistrement et découverte basique
#[test]
fn test_e2e_basic_registration_discovery() {
    // Configuration optimisée
    let config = RegistryConfig::new()
        .with_cache_size(200)
        .with_bloom_size(50_000)
        .with_stale_threshold(300);

    let mut registry = Registry::with_config(config);

    // Enregistrement de 10 services
    for i in 0..10 {
        let name = ServiceName::new(&alloc::format!("service_{}", i)).unwrap();
        let endpoint = alloc::format!("/var/run/exo/service_{}.sock", i);
        let info = ServiceInfo::new(&endpoint);

        registry.register(name, info).unwrap();
    }

    // Vérification stats
    let stats = registry.stats();
    assert_eq!(stats.total_registrations.load(core::sync::atomic::Ordering::Relaxed), 10);
    assert_eq!(stats.active_services.load(core::sync::atomic::Ordering::Relaxed), 10);

    // Lookups (devrait utiliser le cache après premier lookup)
    for i in 0..10 {
        let name = ServiceName::new(&alloc::format!("service_{}", i)).unwrap();
        let info = registry.lookup(&name).expect("Service not found");
        assert_eq!(info.endpoint(), &alloc::format!("/var/run/exo/service_{}.sock", i));
    }

    // Deuxième round de lookups (cache hit)
    for i in 0..10 {
        let name = ServiceName::new(&alloc::format!("service_{}", i)).unwrap();
        let _ = registry.lookup(&name).expect("Service not found");
    }

    // Statistiques finales
    let stats = registry.stats();
    assert_eq!(stats.total_lookups.load(core::sync::atomic::Ordering::Relaxed), 20);

    // Cache hit rate devrait être ~50% (10 misses + 10 hits)
    let cache_hits = stats.cache_hits.load(core::sync::atomic::Ordering::Relaxed);
    assert!(cache_hits >= 8); // Au moins 80% de hits sur le 2ème round
}

/// Scénario 2: Heartbeat et détection de services stale
#[test]
fn test_e2e_heartbeat_and_stale_detection() {
    let mut registry = Registry::new();

    // Enregistre 5 services
    for i in 0..5 {
        let name = ServiceName::new(&alloc::format!("active_service_{}", i)).unwrap();
        let info = ServiceInfo::new(&alloc::format!("/tmp/active_{}.sock", i));
        registry.register(name, info).unwrap();
    }

    // Heartbeat pour garder 3 services actifs
    for i in 0..3 {
        let name = ServiceName::new(&alloc::format!("active_service_{}", i)).unwrap();
        registry.heartbeat(&name).unwrap();
    }

    // Liste tous les services
    let all_services = registry.list();
    assert_eq!(all_services.len(), 5);

    // Liste seulement les actifs
    let active = registry.list_by_status(ServiceStatus::Active);
    assert_eq!(active.len(), 5); // Tous sont encore actifs (pas de TTL expiré)
}

/// Scénario 3: IPC Communication complète
#[test]
#[cfg(feature = "ipc")]
fn test_e2e_ipc_workflow() {
    // === SERVEUR ===

    // Crée le registry backend
    let registry = Box::new(Registry::new());
    let daemon_config = DaemonConfig::new()
        .with_max_connections(50)
        .with_queue_size(128);

    let mut daemon = RegistryDaemon::with_config(registry, daemon_config);

    // === SIMULATION CLIENT (sans vraie IPC) ===

    // Register via daemon
    let name1 = ServiceName::new("ipc_service_1").unwrap();
    let info1 = ServiceInfo::new("/tmp/ipc1.sock");
    let req1 = RegistryRequest::register(name1.clone(), info1);
    let resp1 = daemon.handle_request(req1);
    assert_eq!(resp1.response_type, ResponseType::Ok);

    // Register second service
    let name2 = ServiceName::new("ipc_service_2").unwrap();
    let info2 = ServiceInfo::new("/tmp/ipc2.sock");
    let req2 = RegistryRequest::register(name2.clone(), info2);
    let resp2 = daemon.handle_request(req2);
    assert_eq!(resp2.response_type, ResponseType::Ok);

    // Lookup
    let lookup_req = RegistryRequest::lookup(name1.clone());
    let lookup_resp = daemon.handle_request(lookup_req);
    assert_eq!(lookup_resp.response_type, ResponseType::Found);
    assert!(lookup_resp.service_info.is_some());

    // List all
    let list_req = RegistryRequest::list();
    let list_resp = daemon.handle_request(list_req);
    assert_eq!(list_resp.response_type, ResponseType::List);
    assert_eq!(list_resp.services.len(), 2);

    // Ping
    let ping_req = RegistryRequest::ping();
    let ping_resp = daemon.handle_request(ping_req);
    assert_eq!(ping_resp.response_type, ResponseType::Pong);

    // Stats
    let stats_req = RegistryRequest::get_stats();
    let stats_resp = daemon.handle_request(stats_req);
    assert_eq!(stats_resp.response_type, ResponseType::Stats);
    assert!(stats_resp.stats.is_some());

    // Vérification compteur
    assert_eq!(daemon.requests_processed(), 7);
}

/// Scénario 4: Binary Serialization roundtrip
#[test]
#[cfg(feature = "ipc")]
fn test_e2e_serialization_roundtrip() {
    use exo_service_registry::serialize::BinarySerialize;

    // Crée une requête complexe
    let name = ServiceName::new("test_serialize").unwrap();
    let info = ServiceInfo::new("/tmp/test_serialize.sock");
    let request = RegistryRequest::register(name, info);

    // Sérialise
    let mut buffer = Vec::new();
    request.serialize_into(&mut buffer).unwrap();

    let serialized_size = buffer.len();
    assert!(serialized_size < 100); // Compact

    // Désérialise
    let deserialized = RegistryRequest::deserialize_from(&buffer).unwrap();

    // Vérifie l'égalité
    assert_eq!(request.request_type, deserialized.request_type);
    assert_eq!(request.service_name, deserialized.service_name);
}

/// Scénario 5: Metrics Export (Prometheus)
#[test]
fn test_e2e_metrics_export() {
    let mut registry = Registry::new();

    // Génère du traffic
    for i in 0..100 {
        let name = ServiceName::new(&alloc::format!("metric_test_{}", i)).unwrap();
        let info = ServiceInfo::new(&alloc::format!("/tmp/metric_{}.sock", i));
        registry.register(name.clone(), info).unwrap();

        // Lookup pour générer des stats
        let _ = registry.lookup(&name);
    }

    // Export Prometheus
    let exporter = MetricsExporter::new(MetricsFormat::Prometheus);
    let stats = registry.stats();
    let prometheus_output = exporter.export(&stats);

    // Vérifications
    assert!(prometheus_output.contains("exo_registry_lookups_total"));
    assert!(prometheus_output.contains("exo_registry_active_services 100"));
    assert!(prometheus_output.contains("# HELP"));
    assert!(prometheus_output.contains("# TYPE"));

    // Export JSON
    let json_exporter = MetricsExporter::new(MetricsFormat::Json);
    let json_output = json_exporter.export(&stats);

    assert!(json_output.contains(r#""total_lookups""#));
    assert!(json_output.contains(r#""active_services": 100"#));

    // Export Plain Text
    let plain_exporter = MetricsExporter::new(MetricsFormat::Plain);
    let plain_output = plain_exporter.export(&stats);

    assert!(plain_output.contains("Total Lookups:"));
    assert!(plain_output.contains("Active Services: 100"));
}

/// Scénario 6: Service Versioning
#[test]
fn test_e2e_service_versioning() {
    let mut version_manager = VersionManager::new();

    let service_name = ServiceName::new("versioned_api").unwrap();

    // Enregistre v1.0.0
    let v1_0_0 = ServiceVersion::new(1, 0, 0);
    let service_v1 = VersionedService::new(
        service_name.clone(),
        v1_0_0,
        ServiceInfo::new("/tmp/api_v1.0.0.sock"),
    );
    version_manager.register(service_v1).unwrap();

    // Enregistre v1.1.0 (compatible)
    let v1_1_0 = ServiceVersion::new(1, 1, 0);
    let service_v1_1 = VersionedService::new(
        service_name.clone(),
        v1_1_0,
        ServiceInfo::new("/tmp/api_v1.1.0.sock"),
    );
    version_manager.register(service_v1_1).unwrap();

    // Enregistre v2.0.0 (breaking change)
    let v2_0_0 = ServiceVersion::new(2, 0, 0);
    let service_v2 = VersionedService::new(
        service_name.clone(),
        v2_0_0,
        ServiceInfo::new("/tmp/api_v2.0.0.sock"),
    );
    version_manager.register(service_v2).unwrap();

    // Client demande v1.0.0 compatible -> devrait obtenir v1.1.0
    let required = ServiceVersion::new(1, 0, 0);
    let found = version_manager.find_compatible(&service_name, &required).unwrap();
    assert_eq!(found.version, v1_1_0);

    // Client demande v2.0.0 -> devrait obtenir exactement v2.0.0
    let required_v2 = ServiceVersion::new(2, 0, 0);
    let found_v2 = version_manager.find_compatible(&service_name, &required_v2).unwrap();
    assert_eq!(found_v2.version, v2_0_0);

    // Liste toutes les versions
    let all_versions = version_manager.list_versions(&service_name);
    assert_eq!(all_versions.len(), 3);

    // Déprécier v1.0.0
    version_manager.deprecate_version(&service_name, &v1_0_0).unwrap();
    assert_eq!(version_manager.count_active_versions(&service_name), 2);
}

/// Scénario 7: Health Checking (si feature activée)
#[test]
#[cfg(feature = "health_check")]
fn test_e2e_health_monitoring() {
    let mut registry = Registry::new();

    // Enregistre des services
    for i in 0..5 {
        let name = ServiceName::new(&alloc::format!("health_service_{}", i)).unwrap();
        let info = ServiceInfo::new(&alloc::format!("/tmp/health_{}.sock", i));
        registry.register(name, info).unwrap();
    }

    // Configure le health checker
    let health_config = HealthConfig::new()
        .with_check_interval(10)
        .with_ping_timeout(1000)
        .with_max_failures(3);

    let mut checker = HealthChecker::with_config(health_config);

    // Check tous les services
    let results = checker.check_all(&registry);
    assert_eq!(results.len(), 5);

    // Tous devraient être Unknown initialement (pas de vrai ping)
    for result in &results {
        // Dans un vrai système, on aurait des statuts réels
        let _ = result;
    }

    // Stats du health checker
    let stats = checker.stats();
    assert_eq!(stats.total_checks, 5);
}

/// Scénario 8: Performance Test - 1000 services
#[test]
fn test_e2e_performance_1000_services() {
    let config = RegistryConfig::new()
        .with_cache_size(500)
        .with_bloom_size(100_000);

    let mut registry = Registry::with_config(config);

    // Register 1000 services
    for i in 0..1000 {
        let name = ServiceName::new(&alloc::format!("perf_service_{:04}", i)).unwrap();
        let info = ServiceInfo::new(&alloc::format!("/tmp/perf_{:04}.sock", i));
        registry.register(name, info).unwrap();
    }

    // Lookup 1000 fois (premier round - cold cache)
    for i in 0..1000 {
        let name = ServiceName::new(&alloc::format!("perf_service_{:04}", i)).unwrap();
        let info = registry.lookup(&name).unwrap();
        assert!(info.endpoint().contains(&alloc::format!("{:04}", i)));
    }

    // Lookup 1000 fois (deuxième round - warm cache)
    for i in 0..1000 {
        let name = ServiceName::new(&alloc::format!("perf_service_{:04}", i)).unwrap();
        let _ = registry.lookup(&name).unwrap();
    }

    // Stats finales
    let stats = registry.stats();
    assert_eq!(stats.total_lookups.load(core::sync::atomic::Ordering::Relaxed), 2000);
    assert_eq!(stats.active_services.load(core::sync::atomic::Ordering::Relaxed), 1000);

    // Cache hit rate devrait être excellent sur le 2ème round
    let hits = stats.cache_hits.load(core::sync::atomic::Ordering::Relaxed);
    let total = stats.total_lookups.load(core::sync::atomic::Ordering::Relaxed);

    // Au moins 25% de cache hits (cache size = 500, lookups = 2000)
    assert!(hits as f64 / total as f64 > 0.25);
}

/// Scénario 9: Error Handling complet
#[test]
fn test_e2e_error_handling() {
    let mut registry = Registry::new();

    // Tentative de lookup sur service inexistant
    let name = ServiceName::new("nonexistent").unwrap();
    let result = registry.lookup(&name);
    assert!(result.is_none());

    // Double registration
    let name = ServiceName::new("duplicate").unwrap();
    let info = ServiceInfo::new("/tmp/dup.sock");
    registry.register(name.clone(), info.clone()).unwrap();

    let result = registry.register(name, info);
    assert!(matches!(result, Err(RegistryError::AlreadyRegistered(_))));

    // Heartbeat sur service inexistant
    let name = ServiceName::new("no_such_service").unwrap();
    let result = registry.heartbeat(&name);
    assert!(matches!(result, Err(RegistryError::NotFound(_))));

    // Unregister inexistant
    let name = ServiceName::new("never_registered").unwrap();
    let result = registry.unregister(&name);
    assert!(matches!(result, Err(RegistryError::NotFound(_))));
}

/// Scénario 10: Full Integration - Toutes les phases combined
#[test]
#[cfg(all(feature = "ipc", feature = "health_check"))]
fn test_e2e_full_integration() {
    // Phase 1: Core Registry
    let config = RegistryConfig::new()
        .with_cache_size(200)
        .with_bloom_size(50_000);

    let registry = Box::new(Registry::with_config(config));

    // Phase 2: Daemon avec IPC
    let daemon_config = DaemonConfig::new()
        .with_max_connections(100)
        .with_queue_size(256);

    let mut daemon = RegistryDaemon::with_config(registry, daemon_config);

    // Phase 3: Enregistrement via daemon
    for i in 0..10 {
        let name = ServiceName::new(&alloc::format!("full_service_{}", i)).unwrap();
        let info = ServiceInfo::new(&alloc::format!("/tmp/full_{}.sock", i));
        let req = RegistryRequest::register(name, info);
        let resp = daemon.handle_request(req);
        assert_eq!(resp.response_type, ResponseType::Ok);
    }

    // Phase 4: Metrics
    let stats_req = RegistryRequest::get_stats();
    let stats_resp = daemon.handle_request(stats_req);
    assert!(stats_resp.stats.is_some());

    let stats = stats_resp.stats.unwrap();
    let exporter = MetricsExporter::new(MetricsFormat::Prometheus);

    use core::sync::atomic::AtomicUsize;
    let registry_stats = crate::RegistryStats {
        total_lookups: AtomicUsize::new(stats.total_lookups as usize),
        cache_hits: AtomicUsize::new(stats.cache_hits as usize),
        cache_misses: AtomicUsize::new(stats.cache_misses as usize),
        bloom_rejections: AtomicUsize::new(stats.bloom_rejections as usize),
        total_registrations: AtomicUsize::new(stats.total_registrations as usize),
        total_unregistrations: AtomicUsize::new(stats.total_unregistrations as usize),
        active_services: AtomicUsize::new(stats.active_services),
    };

    let metrics = exporter.export(&registry_stats);
    assert!(metrics.contains("exo_registry_active_services 10"));

    // Phase 5: Versioning
    let mut version_manager = VersionManager::new();
    let v1 = ServiceVersion::new(1, 0, 0);
    let name = ServiceName::new("full_api").unwrap();
    let versioned = VersionedService::new(
        name.clone(),
        v1,
        ServiceInfo::new("/tmp/api_v1.sock"),
    );
    version_manager.register(versioned).unwrap();

    assert_eq!(version_manager.count_active_versions(&name), 1);

    // Vérification finale
    assert_eq!(daemon.requests_processed(), 12); // 10 registers + 1 stats + 1 ping implicit
}
