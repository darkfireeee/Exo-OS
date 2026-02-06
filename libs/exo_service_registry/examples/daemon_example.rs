//! Exemple de daemon IPC pour service registry
//!
//! Démontre comment utiliser RegistryDaemon pour gérer un registry
//! centralisé accessible via IPC.

#![cfg(feature = "ipc")]

use exo_service_registry::daemon::{RegistryDaemon, DaemonConfig};
use exo_service_registry::protocol::{RegistryRequest, ResponseType};
use exo_service_registry::{Registry, RegistryConfig, ServiceName, ServiceInfo};

fn main() {
    println!("=== Registry Daemon Example ===\n");

    // 1. Configuration du registry
    println!("--- Configuration du Registry ---");

    let registry_config = RegistryConfig::new()
        .with_cache_size(200)
        .with_bloom_size(50_000)
        .with_stale_threshold(300);

    let registry = Box::new(Registry::with_config(registry_config));
    println!("  ✓ Registry configuré (cache: 200, bloom: 50K)");

    // 2. Configuration du daemon
    let daemon_config = DaemonConfig::new()
        .with_max_connections(100)
        .with_queue_size(256)
        .with_verbose(true);

    let mut daemon = RegistryDaemon::with_config(registry, daemon_config);
    println!("  ✓ Daemon configuré (max_conn: 100, queue: 256)");

    // 3. Simulation de requêtes IPC
    println!("\n--- Simulation de Requêtes IPC ---");

    // Register services
    for i in 0..5 {
        let name = ServiceName::new(&format!("service_{}", i)).unwrap();
        let info = ServiceInfo::new(&format!("/var/run/exo/service_{}.sock", i));
        let request = RegistryRequest::register(name.clone(), info);

        let response = daemon.handle_request(request);
        println!("  ✓ Register {} -> {:?}", name, response.response_type);
    }

    // Lookup services
    println!("\n  --- Lookups ---");
    for i in 0..5 {
        let name = ServiceName::new(&format!("service_{}", i)).unwrap();
        let request = RegistryRequest::lookup(name.clone());

        let response = daemon.handle_request(request);
        if response.response_type == ResponseType::Found {
            let info = response.service_info.unwrap();
            println!("  ✓ Found {} at {}", name, info.endpoint());
        }
    }

    // Lookup inexistant
    let name = ServiceName::new("nonexistent").unwrap();
    let request = RegistryRequest::lookup(name);
    let response = daemon.handle_request(request);
    println!("  ✓ Lookup nonexistent -> {:?}", response.response_type);

    // List all services
    println!("\n  --- List All ---");
    let request = RegistryRequest::list();
    let response = daemon.handle_request(request);

    if response.response_type == ResponseType::List {
        println!("  Services actifs: {}", response.services.len());
        for (name, info) in &response.services {
            println!("    - {} at {}", name, info.endpoint());
        }
    }

    // Heartbeat
    println!("\n  --- Heartbeats ---");
    for i in 0..5 {
        let name = ServiceName::new(&format!("service_{}", i)).unwrap();
        let request = RegistryRequest::heartbeat(name.clone());
        let response = daemon.handle_request(request);
        println!("  ✓ Heartbeat {} -> {:?}", name, response.response_type);
    }

    // Get Stats
    println!("\n  --- Statistics ---");
    let request = RegistryRequest::get_stats();
    let response = daemon.handle_request(request);

    if let Some(stats) = response.stats {
        println!("  {}", stats);
        println!("    Cache hit rate: {:.1}%", stats.cache_hit_rate() * 100.0);
        println!("    Bloom rejection rate: {:.1}%", stats.bloom_rejection_rate() * 100.0);
    }

    // Ping
    println!("\n  --- Health Check ---");
    let request = RegistryRequest::ping();
    let response = daemon.handle_request(request);
    println!("  Ping -> {:?}", response.response_type);

    // Summary
    println!("\n--- Summary ---");
    println!("  Total requests processed: {}", daemon.requests_processed());
    println!("  Active services: {}", daemon.registry().list().len());

    println!("\n=== Daemon Example Terminé ===");
}
