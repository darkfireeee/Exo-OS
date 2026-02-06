//! Exemple avancé: Service Registry avec Health Monitoring
//!
//! Démontre un workflow production-ready:
//! - Registry avec backend persistant
//! - Health monitoring automatique
//! - Recovery de services crashés
//! - Service watcher pour événements

use exo_service_registry::prelude::*;

#[cfg(feature = "health_check")]
use exo_service_registry::{HealthChecker, HealthConfig};

fn main() {
    println!("=== Advanced Service Registry Example ===\n");

    // 1. Configuration avancée du registry
    println!("--- Configuration ---");

    let config = RegistryConfig::new()
        .with_cache_size(200)
        .with_cache_ttl(120)
        .with_bloom_size(50_000)
        .with_stale_threshold(300);

    #[cfg(feature = "persistent")]
    let backend = {
        println!("  ✓ Backend persistant: TOML (/tmp/registry.toml)");
        Box::new(TomlBackend::new("/tmp/registry.toml"))
    };

    #[cfg(not(feature = "persistent"))]
    let backend = {
        println!("  ✓ Backend: In-Memory");
        Box::new(InMemoryBackend::new())
    };

    let mut registry = Registry::with_backend(backend);
    println!("  ✓ Registry configuré (cache: 200, TTL: 120s, stale: 300s)");

    // 2. Enregistrement de services système
    println!("\n--- Enregistrement de Services Système ---");

    let system_services = vec![
        ("fs_service", "/var/run/exo/fs.sock", "Filesystem service"),
        ("net_service", "/var/run/exo/net.sock", "Network manager"),
        ("logger_service", "/var/run/exo/logger.sock", "Logging daemon"),
        ("auth_service", "/var/run/exo/auth.sock", "Authentication service"),
        ("config_manager", "/var/run/exo/config.sock", "Configuration manager"),
        ("ipc_broker", "/var/run/exo/ipc.sock", "IPC message broker"),
        ("device_manager", "/var/run/exo/devmgr.sock", "Device manager"),
        ("scheduler", "/var/run/exo/sched.sock", "Task scheduler"),
    ];

    for (name_str, endpoint, description) in &system_services {
        let name = ServiceName::new(name_str).unwrap();
        let info = ServiceInfo::new(*endpoint);
        registry.register(name, info).unwrap();
        println!("  ✓ {} -> {} ({})", name_str, endpoint, description);
    }

    // 3. Simulation de charge (lookups multiples)
    println!("\n--- Simulation de Charge ---");

    let lookup_count = 10_000;
    println!("  Effectue {} lookups...", lookup_count);

    for i in 0..lookup_count {
        let idx = i % system_services.len();
        let name = ServiceName::new(system_services[idx].0).unwrap();
        let _ = registry.lookup(&name);
    }

    // Affiche les stats de performance
    let stats = registry.stats();
    println!(
        "  ✓ Lookups: {}",
        stats.total_lookups.load(core::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "  ✓ Cache hit rate: {:.2}%",
        stats.cache_hit_rate() * 100.0
    );
    println!(
        "  ✓ Bloom rejection rate: {:.2}%",
        stats.bloom_rejection_rate() * 100.0
    );

    // 4. Health monitoring (si feature activée)
    #[cfg(feature = "health_check")]
    {
        println!("\n--- Health Monitoring ---");

        let health_config = HealthConfig::new()
            .with_check_interval(30)
            .with_ping_timeout(1000)
            .with_max_failures(3)
            .with_auto_recovery(true);

        let mut checker = HealthChecker::with_config(health_config);

        // Premier health check
        println!("  Effectue health check sur tous les services...");
        let results = checker.check_all(&registry);

        for result in &results {
            println!(
                "    {} -> {} ({}μs)",
                result.service_name, result.status, result.response_time_us
            );
        }

        let health_stats = checker.stats();
        println!("\n  Statistiques de Health:");
        println!("    Total: {}", health_stats.total_services);
        println!("    Healthy: {}", health_stats.healthy_count);
        println!("    Degraded: {}", health_stats.degraded_count);
        println!("    Unhealthy: {}", health_stats.unhealthy_count);
        println!(
            "    Health rate: {:.1}%",
            health_stats.health_rate() * 100.0
        );
        println!(
            "    Availability: {:.1}%",
            health_stats.availability_rate() * 100.0
        );

        // Simulation de services failed
        println!("\n  Simulation de services crashés...");

        // Tente recovery
        println!("  Tentative de recovery automatique...");
        let recovered = checker.recover_failed_services(&mut registry);
        println!("    ✓ {} services récupérés", recovered.len());
    }

    // 5. Liste des services par statut
    println!("\n--- Services par Statut ---");

    let active = registry.list_by_status(ServiceStatus::Active);
    println!("  Active: {}", active.len());

    let degraded = registry.list_by_status(ServiceStatus::Degraded);
    println!("  Degraded: {}", degraded.len());

    let failed = registry.list_by_status(ServiceStatus::Failed);
    println!("  Failed: {}", failed.len());

    // 6. Detection de services stale
    println!("\n--- Detection de Services Stale ---");

    let stale = registry.get_stale_services();
    if stale.is_empty() {
        println!("  ✓ Aucun service stale détecté");
    } else {
        println!("  ⚠ {} services stale:", stale.len());
        for (name, _) in stale {
            println!("    - {}", name);
        }
    }

    // 7. Heartbeat simulation
    println!("\n--- Heartbeat Simulation ---");

    println!("  Envoi de heartbeats pour tous les services actifs...");
    for (name, _) in registry.list() {
        if let Err(e) = registry.heartbeat(&name) {
            println!("    ✗ Heartbeat failed for {}: {}", name, e);
        }
    }
    println!("  ✓ Heartbeats envoyés");

    // 8. Persistence (si activée)
    #[cfg(feature = "persistent")]
    {
        println!("\n--- Persistence ---");
        match registry.flush() {
            Ok(_) => println!("  ✓ Registry persisté dans /tmp/registry.toml"),
            Err(e) => println!("  ✗ Erreur de persistence: {}", e),
        }
    }

    // 9. Statistiques finales
    println!("\n--- Statistiques Finales ---");

    let final_stats = registry.stats();
    println!(
        "  Total registrations: {}",
        final_stats
            .total_registrations
            .load(core::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "  Total unregistrations: {}",
        final_stats
            .total_unregistrations
            .load(core::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "  Total lookups: {}",
        final_stats
            .total_lookups
            .load(core::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "  Cache hits: {} ({:.1}%)",
        final_stats
            .cache_hits
            .load(core::sync::atomic::Ordering::Relaxed),
        final_stats.cache_hit_rate() * 100.0
    );
    println!(
        "  Active services: {}",
        final_stats
            .active_services
            .load(core::sync::atomic::Ordering::Relaxed)
    );

    println!("\n=== Exemple Avancé Terminé ===");
}
