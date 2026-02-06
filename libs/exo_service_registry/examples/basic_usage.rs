//! Exemple d'utilisation basique de exo_service_registry
//!
//! Montre comment:
//! - Créer un registry
//! - Enregistrer des services
//! - Faire des lookups
//! - Gérer les heartbeats
//! - Afficher les statistiques

use exo_service_registry::prelude::*;

fn main() {
    println!("=== Service Registry Example ===\n");

    // 1. Créer le registry
    let config = RegistryConfig::new()
        .with_cache_size(100)
        .with_cache_ttl(60)
        .with_bloom_size(10_000);

    let mut registry = Registry::with_config(config);
    println!("✓ Registry créé avec cache LRU (100 entrées) et bloom filter (10K)");

    // 2. Enregistrer des services
    println!("\n--- Enregistrement de services ---");

    let services = vec![
        ("fs_service", "/tmp/fs.sock"),
        ("net_service", "/tmp/net.sock"),
        ("logger_service", "/tmp/logger.sock"),
        ("config_manager", "/tmp/config.sock"),
        ("auth_service", "/tmp/auth.sock"),
    ];

    for (name_str, endpoint) in &services {
        let name = ServiceName::new(name_str).unwrap();
        let info = ServiceInfo::new(*endpoint);
        registry.register(name, info).unwrap();
        println!("  ✓ Enregistré: {} -> {}", name_str, endpoint);
    }

    // 3. Lookup de services
    println!("\n--- Lookup de services ---");

    let fs_name = ServiceName::new("fs_service").unwrap();
    if let Some(info) = registry.lookup(&fs_name) {
        println!(
            "  ✓ Trouvé: fs_service at {} (status: {})",
            info.endpoint(),
            info.status()
        );
    }

    // Lookup d'un service inexistant
    let fake_name = ServiceName::new("nonexistent_service").unwrap();
    if registry.lookup(&fake_name).is_none() {
        println!("  ✓ Service inexistant correctement rejeté (bloom filter)");
    }

    // 4. Heartbeat
    println!("\n--- Heartbeat ---");

    let net_name = ServiceName::new("net_service").unwrap();
    registry.heartbeat(&net_name).unwrap();
    println!("  ✓ Heartbeat envoyé pour net_service");

    // 5. Liste tous les services
    println!("\n--- Liste de tous les services ---");

    let all_services = registry.list();
    println!("  Services actifs: {}", all_services.len());
    for (name, info) in &all_services {
        println!(
            "    - {} at {} ({})",
            name,
            info.endpoint(),
            info.status()
        );
    }

    // 6. Statistiques
    println!("\n--- Statistiques du Registry ---");

    let stats = registry.stats();
    println!(
        "  Total lookups: {}",
        stats.total_lookups.load(core::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "  Cache hits: {}",
        stats.cache_hits.load(core::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "  Cache misses: {}",
        stats.cache_misses.load(core::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "  Bloom rejections: {}",
        stats.bloom_rejections.load(core::sync::atomic::Ordering::Relaxed)
    );
    println!("  Cache hit rate: {:.1}%", stats.cache_hit_rate() * 100.0);
    println!(
        "  Services actifs: {}",
        stats.active_services.load(core::sync::atomic::Ordering::Relaxed)
    );

    // 7. Discovery client
    println!("\n--- Discovery Client ---");

    let discovery = DiscoveryClient::new()
        .with_max_retries(3)
        .with_timeout(5000);

    println!(
        "  ✓ Discovery client créé (max_retries: {}, timeout: {}ms)",
        discovery.max_retries(),
        discovery.timeout_ms()
    );

    // 8. Health checking (si feature activée)
    #[cfg(feature = "health_check")]
    {
        println!("\n--- Health Checking ---");

        let mut checker = HealthChecker::new();
        let results = checker.check_all(&registry);

        println!("  Health check effectué sur {} services", results.len());

        let health_stats = checker.stats();
        println!(
            "  Healthy: {}/{} ({:.1}%)",
            health_stats.healthy_count,
            health_stats.total_services,
            health_stats.health_rate() * 100.0
        );
        println!(
            "  Availability: {:.1}%",
            health_stats.availability_rate() * 100.0
        );
        println!(
            "  Avg response time: {}μs",
            health_stats.avg_response_time_us
        );
    }

    // 9. Désregistration
    println!("\n--- Désregistration ---");

    let logger_name = ServiceName::new("logger_service").unwrap();
    registry.unregister(&logger_name).unwrap();
    println!("  ✓ logger_service désregistré");

    let remaining = registry.list();
    println!("  Services restants: {}", remaining.len());

    println!("\n=== Exemple terminé ===");
}
