//! Exemple d'intégration IPC complète
//!
//! Démontre comment utiliser IpcServer et IpcClient pour communiquer
//! avec le service registry via exo_ipc.
//!
//! Note: Dans un vrai système, le serveur et les clients seraient
//! dans des processus séparés communicant via sockets Unix. Ici on
//! simule juste la sérialisation/désérialisation IPC.

#![cfg(feature = "ipc")]

use exo_service_registry::{
    Registry, RegistryConfig,
    ServiceName, ServiceInfo,
    daemon::{RegistryDaemon, DaemonConfig},
    ipc::{IpcServer, IpcClient},
    protocol::{RegistryRequest, RegistryResponse, ResponseType},
    serialize::BinarySerialize,
};

fn main() {
    println!("=== IPC Integration Example ===\n");

    // 1. Configuration et création du registry
    println!("--- Configuration du Registry ---");
    let registry_config = RegistryConfig::new()
        .with_cache_size(200)
        .with_bloom_size(50_000)
        .with_stale_threshold(300);

    let registry = Box::new(Registry::with_config(registry_config));
    println!("  ✓ Registry configuré");

    // 2. Création du daemon
    let daemon_config = DaemonConfig::new()
        .with_max_connections(100)
        .with_queue_size(256)
        .with_verbose(true);

    let daemon = RegistryDaemon::with_config(registry, daemon_config);
    println!("  ✓ Daemon créé");

    // 3. Création du serveur IPC
    let mut server = IpcServer::new(daemon, 64).expect("Failed to create IPC server");
    println!("  ✓ IPC Server initialisé");

    // 4. Simulation de requêtes IPC (sans vraie communication inter-process)
    println!("\n--- Simulation Sérialisation IPC ---");

    // Register service
    let name = ServiceName::new("test_service").unwrap();
    let info = ServiceInfo::new("/var/run/exo/test_service.sock");
    let request = RegistryRequest::register(name.clone(), info);

    println!("  → Sérialisation de RegistryRequest::Register");
    let mut req_buf = Vec::new();
    request.serialize_into(&mut req_buf).unwrap();
    println!("    Taille sérialisée: {} bytes", req_buf.len());

    // Désérialisation et traitement
    let deserialized_req = RegistryRequest::deserialize_from(&req_buf).unwrap();
    let response = server.daemon_mut().handle_request(deserialized_req);
    println!("  ✓ Request traitée -> {:?}", response.response_type);

    // Sérialise la réponse
    let mut resp_buf = Vec::new();
    response.serialize_into(&mut resp_buf).unwrap();
    println!("    Response size: {} bytes", resp_buf.len());

    // Lookup service
    println!("\n  → Lookup du service");
    let lookup_req = RegistryRequest::lookup(name.clone());
    let mut lookup_buf = Vec::new();
    lookup_req.serialize_into(&mut lookup_buf).unwrap();
    println!("    Lookup request size: {} bytes", lookup_buf.len());

    let lookup_req_deser = RegistryRequest::deserialize_from(&lookup_buf).unwrap();
    let lookup_resp = server.daemon_mut().handle_request(lookup_req_deser);

    if lookup_resp.response_type == ResponseType::Found {
        println!("  ✓ Service trouvé!");
        if let Some(info) = lookup_resp.service_info {
            println!("    Endpoint: {}", info.endpoint());
        }
    }

    // List all services
    println!("\n  → List all services");
    let list_req = RegistryRequest::list();
    let mut list_buf = Vec::new();
    list_req.serialize_into(&mut list_buf).unwrap();

    let list_req_deser = RegistryRequest::deserialize_from(&list_buf).unwrap();
    let list_resp = server.daemon_mut().handle_request(list_req_deser);

    if list_resp.response_type == ResponseType::List {
        println!("  ✓ Services actifs: {}", list_resp.services.len());
        for (name, info) in &list_resp.services {
            println!("    - {} at {}", name, info.endpoint());
        }
    }

    // Heartbeat
    println!("\n  → Heartbeat");
    let hb_req = RegistryRequest::heartbeat(name.clone());
    let mut hb_buf = Vec::new();
    hb_req.serialize_into(&mut hb_buf).unwrap();

    let hb_req_deser = RegistryRequest::deserialize_from(&hb_buf).unwrap();
    let hb_resp = server.daemon_mut().handle_request(hb_req_deser);
    println!("  ✓ Heartbeat -> {:?}", hb_resp.response_type);

    // Ping
    println!("\n  → Ping");
    let ping_req = RegistryRequest::ping();
    let mut ping_buf = Vec::new();
    ping_req.serialize_into(&mut ping_buf).unwrap();
    println!("    Ping request size: {} bytes", ping_buf.len());

    let ping_req_deser = RegistryRequest::deserialize_from(&ping_buf).unwrap();
    let ping_resp = server.daemon_mut().handle_request(ping_req_deser);
    println!("  ✓ Ping -> {:?}", ping_resp.response_type);

    // Stats
    println!("\n--- Statistiques finales ---");
    let stats_req = RegistryRequest::get_stats();
    let mut stats_buf = Vec::new();
    stats_req.serialize_into(&mut stats_buf).unwrap();

    let stats_req_deser = RegistryRequest::deserialize_from(&stats_buf).unwrap();
    let stats_resp = server.daemon_mut().handle_request(stats_req_deser);

    if let Some(stats) = stats_resp.stats {
        println!("  {}", stats);
        println!("  Cache hit rate: {:.1}%", stats.cache_hit_rate() * 100.0);
    }

    println!("  Total requests processed: {}", server.requests_processed());

    // 5. Test de création du client (pour montrer l'API)
    println!("\n--- Client IPC ---");
    let client = IpcClient::new(64).expect("Failed to create client");
    println!("  ✓ IPC Client créé");

    // Note: Dans un vrai système, le client se connecterait au serveur
    // via un socket Unix et enverrait des requêtes. Ici on montre juste
    // que les structures sont correctes et que la sérialisation fonctionne.

    println!("\n=== IPC Example Terminé ===");
    println!("\nNote: Dans un environnement de production:");
    println!("  - Le serveur tournerait sur /var/run/exo/registry.sock");
    println!("  - Les clients se connecteraient via exo_ipc::channel");
    println!("  - Communication vraiment inter-process avec zéro-copy");
}
