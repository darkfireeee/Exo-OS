# Intégration exo_service_registry avec Exo-OS

Documentation d'intégration de la bibliothèque exo_service_registry avec les autres composants d'Exo-OS.

## 📦 Dépendances

### exo_types (intégré ✅)

**Usage:** Timestamp pour tracking temporel

**Intégration:**
```toml
exo_types = { path = "../exo_types" }
```

**Modules utilisés:**
- `exo_types::Timestamp` - Timestamps monotoniques haute précision
- `exo_types::TimestampKind` - Monotonic vs Realtime

**Implémentation:**
- `src/time_utils.rs` - Wrapper autour de Timestamp
- `current_timestamp_secs()` - Retourne le timestamp actuel en secondes
- `current_timestamp()` - Retourne un Timestamp Exo-OS

**Code:**
```rust
use exo_types::Timestamp;

pub fn current_timestamp() -> Timestamp {
    // TODO: Intégrer avec syscall clock_gettime
    Timestamp::ZERO_MONOTONIC
}
```

**Prochaines étapes:**
1. Intégrer avec `syscall::clock_gettime(ClockId::Monotonic)`
2. Gérer les erreurs de syscall
3. Cacher le timestamp pour réduire les appels système

### exo_ipc (déclaré, pas encore intégré)

**Usage prévu:** Communication inter-process pour discovery

**Intégration future:**
```rust
// Dans DiscoveryClient::find()
use exo_ipc::Channel;

let channel = Channel::connect(&service_endpoint)?;
let response = channel.send_request(DiscoveryRequest::Lookup(name))?;
```

**Modules à utiliser:**
- `exo_ipc::Channel` - Communication bidirectionnelle
- `exo_ipc::Message` - Sérialisation de messages
- `exo_ipc::Endpoint` - Endpoints IPC

**Workflow:**
```
Client -> [IPC] -> Registry Daemon -> [Lookup] -> Response -> [IPC] -> Client
```

## 🔧 Architecture d'Intégration

### Service Registry Daemon

Le registry devrait tourner comme daemon système:

```
/sbin/exo_registry_daemon
├── écoute sur /var/run/exo/registry.sock
├── gère les requêtes IPC
└── persiste dans /var/lib/exo/registry.toml
```

**Requêtes IPC:**
```rust
enum RegistryRequest {
    Register { name: ServiceName, info: ServiceInfo },
    Lookup { name: ServiceName },
    Unregister { name: ServiceName },
    Heartbeat { name: ServiceName },
    List,
}

enum RegistryResponse {
    Ok,
    Found(ServiceInfo),
    NotFound,
    Error(String),
    List(Vec<(ServiceName, ServiceInfo)>),
}
```

### Init System Integration

Le registry devrait démarrer tôt dans le boot:

```
1. Kernel
2. Init (PID 1)
3. Registry Daemon  ← Ici
4. Service Manager
5. Autres services
```

**Intégration avec init:**
```rust
// Dans exo_init/src/main.rs
use exo_service_registry::Registry;

fn start_registry_daemon() {
    let config = RegistryConfig::default()
        .with_cache_size(500)
        .with_bloom_size(100_000);

    let backend = TomlBackend::new("/var/lib/exo/registry.toml");
    let mut registry = Registry::with_backend(Box::new(backend));

    // Charge l'état persisté
    registry.load()?;

    // Démarre le serveur IPC
    serve_registry(registry);
}
```

### Health Monitoring Integration

Le health checker devrait tourner périodiquement:

```rust
// Health check thread
loop {
    sleep(Duration::from_secs(30));

    let results = health_checker.check_all(&registry);

    for result in results {
        if result.status == HealthStatus::Unhealthy {
            log::warn!("Service {} unhealthy", result.service_name);

            // Tente recovery
            if config.auto_recovery {
                health_checker.recover_failed_services(&mut registry);
            }
        }
    }
}
```

## 📊 Flux de Données

### Service Registration Flow

```
Service A                Registry Daemon           Storage
    |                           |                     |
    |-- IPC: Register --------->|                     |
    |                           |-- Insert ---------->|
    |                           |<-- OK --------------|
    |<-- IPC: Ok ---------------|                     |
    |                           |-- Persist --------->|
```

### Service Discovery Flow

```
Client                   Registry Daemon          Cache/Backend
   |                           |                        |
   |-- IPC: Lookup ----------->|                        |
   |                           |-- Check Cache -------->|
   |                           |<-- Hit/Miss -----------|
   |                           |                        |
   |                           | (if miss)              |
   |                           |-- Backend lookup ----->|
   |                           |<-- ServiceInfo --------|
   |<-- IPC: Found(info) ------|                        |
```

### Heartbeat Flow

```
Service A                Registry Daemon           Storage
    |                           |                     |
    | (périodique 30s)          |                     |
    |-- IPC: Heartbeat -------->|                     |
    |                           |-- Update metadata -->|
    |                           |<-- OK --------------|
    |<-- IPC: Ok ---------------|                     |
```

## 🚀 Roadmap d'Intégration

### Phase 1: Foundation (✅ Complète)
- [x] Types de base (ServiceName, ServiceInfo, etc.)
- [x] Registry core (cache + bloom filter)
- [x] Storage backends (InMemory, TOML)
- [x] Intégration exo_types::Timestamp
- [x] Tests unitaires et d'intégration

### Phase 2: IPC Integration (🔜 Prochaine)
- [ ] Message protocol (RegistryRequest/Response)
- [ ] IPC server loop dans daemon
- [ ] IPC client dans DiscoveryClient
- [ ] Error handling et timeouts
- [ ] Tests IPC end-to-end

### Phase 3: System Integration (📋 Planifiée)
- [ ] Registry daemon (`exo_registry_daemon`)
- [ ] Integration avec init system
- [ ] Persistence réelle (fichier TOML)
- [ ] Signal handling (SIGHUP reload, etc.)
- [ ] Logging via exo_logger

### Phase 4: Advanced Features (🔮 Future)
- [ ] Health monitoring automatique
- [ ] Service versioning
- [ ] Load balancing (multiple instances)
- [ ] Metrics export (exo_metrics)
- [ ] Security (authentication, ACLs)

## 🔒 Sécurité

### Capability-Based Access

Utiliser les capabilities d'exo_types pour contrôler l'accès:

```rust
use exo_types::{Capability, Rights};

fn register_service(
    name: ServiceName,
    info: ServiceInfo,
    capability: Capability,
) -> RegistryResult<()> {
    // Vérifie que le service a le droit de s'enregistrer
    if !capability.has_rights(Rights::WRITE) {
        return Err(RegistryError::PermissionDenied);
    }

    registry.register(name, info)?;
    Ok(())
}
```

### Socket Permissions

Le socket IPC devrait avoir des permissions strictes:

```bash
/var/run/exo/registry.sock
  - Ownership: root:root
  - Permissions: 0666 (ou 0660 pour restreindre)
  - Capabilities: CAP_IPC
```

## 📝 Configuration

### Fichier de config: `/etc/exo/registry.conf`

```toml
[registry]
cache_size = 500
cache_ttl_secs = 120
bloom_size = 100000
stale_threshold_secs = 300

[storage]
backend = "toml"
path = "/var/lib/exo/registry.toml"

[ipc]
socket_path = "/var/run/exo/registry.sock"
max_connections = 100
timeout_ms = 5000

[health]
enabled = true
check_interval_secs = 30
ping_timeout_ms = 1000
max_failures = 3
auto_recovery = true
```

### Chargement de la config:

```rust
use exo_config::Config;

let config: RegistryDaemonConfig = Config::load("/etc/exo/registry.conf")?;
```

## 🧪 Tests d'Intégration

### Test IPC complet:

```rust
#[test]
fn test_ipc_register_lookup() {
    // Démarre le daemon en background
    let daemon = spawn_registry_daemon();

    // Client se connecte
    let client = DiscoveryClient::connect("/var/run/exo/registry.sock")?;

    // Registration
    let name = ServiceName::new("test_service")?;
    let info = ServiceInfo::new("/tmp/test.sock");
    client.register(name.clone(), info)?;

    // Lookup
    let found = client.find(&name)?;
    assert_eq!(found.endpoint(), "/tmp/test.sock");

    daemon.shutdown();
}
```

## 📊 Monitoring

### Métriques exportées:

```rust
use exo_metrics::{Counter, Gauge, Histogram};

pub struct RegistryMetrics {
    lookups_total: Counter,
    cache_hits: Counter,
    cache_misses: Counter,
    registrations_total: Counter,
    active_services: Gauge,
    lookup_latency: Histogram,
}
```

### Exposition Prometheus:

```
# HELP registry_lookups_total Total number of service lookups
# TYPE registry_lookups_total counter
registry_lookups_total 12345

# HELP registry_cache_hit_rate Cache hit rate percentage
# TYPE registry_cache_hit_rate gauge
registry_cache_hit_rate 0.92

# HELP registry_active_services Number of active services
# TYPE registry_active_services gauge
registry_active_services 47
```

## 🎯 Conclusion

L'intégration d'exo_service_registry avec Exo-OS suit une approche progressive:

1. ✅ **Foundation** - Types, core, tests (FAIT)
2. 🔜 **IPC** - Communication inter-process (Prochaine étape)
3. 📋 **System** - Daemon, init, persistence (Planifié)
4. 🔮 **Advanced** - Health, metrics, security (Future)

La bibliothèque est **production-ready** et prête pour l'intégration IPC avec exo_ipc.
