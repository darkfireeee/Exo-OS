//! Tests d'intégration pour exo_service_registry
//!
//! Teste les workflows complets:
//! - Registration -> Lookup -> Unregister
//! - Health checking avec recovery
//! - Persistence avec TOML backend
//! - Performance sous charge

#![cfg(feature = "std")]

use exo_service_registry::prelude::*;

#[test]
fn test_complete_workflow() {
    let mut registry = Registry::new();

    // 1. Enregistrement
    let name = ServiceName::new("test_service").unwrap();
    let info = ServiceInfo::new("/tmp/test.sock");
    registry.register(name.clone(), info).unwrap();

    // 2. Lookup
    let found = registry.lookup(&name).expect("service should exist");
    assert_eq!(found.endpoint(), "/tmp/test.sock");
    assert_eq!(found.status(), ServiceStatus::Active);

    // 3. Heartbeat
    registry.heartbeat(&name).unwrap();

    // 4. List
    let services = registry.list();
    assert_eq!(services.len(), 1);

    // 5. Unregister
    registry.unregister(&name).unwrap();
    assert!(registry.lookup(&name).is_none());
}

#[test]
fn test_multiple_services() {
    let mut registry = Registry::new();

    // Enregistre 10 services
    for i in 0..10 {
        let name = ServiceName::new(&format!("service_{}", i)).unwrap();
        let info = ServiceInfo::new(&format!("/tmp/service_{}.sock", i));
        registry.register(name, info).unwrap();
    }

    // Vérifie qu'ils existent tous
    for i in 0..10 {
        let name = ServiceName::new(&format!("service_{}", i)).unwrap();
        let found = registry.lookup(&name);
        assert!(found.is_some());
    }

    // Vérifie le count
    let services = registry.list();
    assert_eq!(services.len(), 10);
}

#[test]
fn test_cache_effectiveness() {
    let config = RegistryConfig::default().with_cache_size(100);
    let mut registry = Registry::with_config(config);

    let name = ServiceName::new("cached_service").unwrap();
    let info = ServiceInfo::new("/tmp/cached.sock");
    registry.register(name.clone(), info).unwrap();

    // Premier lookup (cache miss)
    let _ = registry.lookup(&name);

    let stats_before = registry.stats();
    let cache_hits_before = stats_before.cache_hits.load(core::sync::atomic::Ordering::Relaxed);

    // Plusieurs lookups (devrait être cache hits)
    for _ in 0..100 {
        let _ = registry.lookup(&name);
    }

    let stats_after = registry.stats();
    let cache_hits_after = stats_after.cache_hits.load(core::sync::atomic::Ordering::Relaxed);

    // Au moins 90 cache hits sur 100 lookups
    assert!(cache_hits_after - cache_hits_before >= 90);
}

#[test]
fn test_bloom_filter_effectiveness() {
    let mut registry = Registry::new();

    // Enregistre quelques services
    for i in 0..10 {
        let name = ServiceName::new(&format!("service_{}", i)).unwrap();
        let info = ServiceInfo::new(&format!("/tmp/service_{}.sock", i));
        registry.register(name, info).unwrap();
    }

    let stats_before = registry.stats();
    let bloom_rejections_before = stats_before
        .bloom_rejections
        .load(core::sync::atomic::Ordering::Relaxed);

    // Lookup de services inexistants
    for i in 0..100 {
        let name = ServiceName::new(&format!("nonexistent_{}", i)).unwrap();
        let _ = registry.lookup(&name);
    }

    let stats_after = registry.stats();
    let bloom_rejections_after = stats_after
        .bloom_rejections
        .load(core::sync::atomic::Ordering::Relaxed);

    // Au moins 80% devrait être rejeté par bloom filter
    assert!(bloom_rejections_after - bloom_rejections_before >= 80);
}

#[test]
fn test_service_failure_and_recovery() {
    let mut registry = Registry::new();

    let name = ServiceName::new("failing_service").unwrap();
    let mut info = ServiceInfo::new("/tmp/failing.sock");

    // Simule des failures
    info.record_failure();
    info.record_failure();
    info.record_failure();
    assert_eq!(info.status(), ServiceStatus::Failed);

    registry.register(name.clone(), info).unwrap();

    // Heartbeat devrait le remettre en Active
    registry.heartbeat(&name).unwrap();

    let recovered = registry.lookup(&name).unwrap();
    assert_eq!(recovered.status(), ServiceStatus::Active);
}

#[test]
#[cfg(feature = "health_check")]
fn test_health_checker_integration() {
    use exo_service_registry::HealthChecker;

    let mut registry = Registry::new();

    // Enregistre des services
    for i in 0..5 {
        let name = ServiceName::new(&format!("service_{}", i)).unwrap();
        let info = ServiceInfo::new(&format!("/tmp/service_{}.sock", i));
        registry.register(name, info).unwrap();
    }

    // Health check
    let mut checker = HealthChecker::new();
    let results = checker.check_all(&registry);

    assert_eq!(results.len(), 5);

    let stats = checker.stats();
    assert_eq!(stats.total_services, 5);
}

#[test]
fn test_discovery_client() {
    let client = DiscoveryClient::new()
        .with_max_retries(5)
        .with_timeout(10000);

    assert_eq!(client.max_retries(), 5);
    assert_eq!(client.timeout_ms(), 10000);

    // Lookup devrait échouer (pas de registry réel)
    let name = ServiceName::new("test").unwrap();
    let result = client.find(&name);
    assert!(result.is_err());
}

#[test]
fn test_registry_stats() {
    let mut registry = Registry::new();

    // Enregistre et lookup
    let name = ServiceName::new("test").unwrap();
    registry.register(name.clone(), ServiceInfo::new("/tmp/test.sock")).unwrap();

    registry.lookup(&name);
    registry.lookup(&name);

    let stats = registry.stats();
    assert_eq!(
        stats.total_registrations.load(core::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        stats.total_lookups.load(core::sync::atomic::Ordering::Relaxed),
        2
    );
}

#[test]
fn test_persistent_backend() {
    #[cfg(feature = "persistent")]
    {
        use exo_service_registry::TomlBackend;

        let backend = TomlBackend::new("/tmp/test_registry.toml");
        let mut registry = Registry::with_backend(Box::new(backend));

        let name = ServiceName::new("persistent_service").unwrap();
        registry.register(name.clone(), ServiceInfo::new("/tmp/persistent.sock")).unwrap();

        // Flush
        registry.flush().unwrap();

        // Lookup
        let found = registry.lookup(&name);
        assert!(found.is_some());
    }
}

#[test]
fn test_service_name_validation() {
    // Valid names
    assert!(ServiceName::new("fs_service").is_ok());
    assert!(ServiceName::new("net_manager").is_ok());
    assert!(ServiceName::new("logger-daemon").is_ok());

    // Invalid names
    assert!(ServiceName::new("").is_err());
    assert!(ServiceName::new("FS_SERVICE").is_err());
    assert!(ServiceName::new("9service").is_err());
    assert!(ServiceName::new("service__bad").is_err());
    assert!(ServiceName::new("service.name").is_err());
}

#[test]
fn test_concurrent_operations() {
    use std::sync::{Arc, Mutex};
    use std::thread;

    let registry = Arc::new(Mutex::new(Registry::new()));

    let mut handles = vec![];

    // Spawn 10 threads qui enregistrent des services
    for i in 0..10 {
        let registry_clone = Arc::clone(&registry);
        let handle = thread::spawn(move || {
            let mut reg = registry_clone.lock().unwrap();
            let name = ServiceName::new(&format!("concurrent_{}", i)).unwrap();
            let info = ServiceInfo::new(&format!("/tmp/concurrent_{}.sock", i));
            reg.register(name, info).unwrap();
        });
        handles.push(handle);
    }

    // Attend que tous finissent
    for handle in handles {
        handle.join().unwrap();
    }

    // Vérifie que tous les services sont là
    let reg = registry.lock().unwrap();
    let services = reg.list();
    assert_eq!(services.len(), 10);
}

#[test]
fn test_service_list_by_status() {
    let mut registry = Registry::new();

    // Enregistre des services avec différents statuts
    let mut info1 = ServiceInfo::new("/tmp/service1.sock");
    info1.activate();
    registry.register(ServiceName::new("service1").unwrap(), info1).unwrap();

    let mut info2 = ServiceInfo::new("/tmp/service2.sock");
    info2.record_failure();
    info2.record_failure();
    info2.record_failure();
    registry.register(ServiceName::new("service2").unwrap(), info2).unwrap();

    // Liste par statut
    let active = registry.list_by_status(ServiceStatus::Active);
    assert_eq!(active.len(), 1);

    let failed = registry.list_by_status(ServiceStatus::Failed);
    assert_eq!(failed.len(), 1);
}

#[test]
fn test_registry_clear() {
    let mut registry = Registry::new();

    // Enregistre plusieurs services
    for i in 0..5 {
        let name = ServiceName::new(&format!("service_{}", i)).unwrap();
        let info = ServiceInfo::new(&format!("/tmp/service_{}.sock", i));
        registry.register(name, info).unwrap();
    }

    assert_eq!(registry.list().len(), 5);

    // Clear
    registry.clear();
    assert_eq!(registry.list().len(), 0);
}
