# Services Framework

Framework commun pour tous les services userspace d'Exo-OS.

## Vue d'Ensemble

Le framework `services` fournit une infrastructure standardisée pour créer, enregistrer et gérer des services système. Il simplifie considérablement la création de nouveaux services en fournissant des abstractions pour les tâches communes.

## Composants

### 1. Service Trait

Tous les services doivent implémenter le trait `Service`:

```rust
use services::Service;

struct MyService {
    // État du service
}

impl Service for MyService {
    fn name(&self) -> &str {
        "my_service"
    }
    
    fn capabilities_required(&self) -> &ServiceCapabilities {
        &self.caps
    }
    
    fn start(&mut self) -> Result<()> {
        // Initialisation
        Ok(())
    }
    
    fn stop(&mut self) -> Result<()> {
        // Cleanup
        Ok(())
    }
    
    fn health_check(&self) -> HealthStatus {
        HealthStatus::Healthy
    }
}
```

### 2. Service Registry

Enregistrement auprès d'init:

```rust
use services::ServiceRegistry;

let handle = ServiceRegistry::register(my_service)?;
ServiceRegistry::notify_ready(handle)?;

// Heartbeat périodique
ServiceRegistry::heartbeat(handle)?;
```

### 3. Service Discovery

Trouver et se connecter à d'autres services:

```rust
use services::ServiceDiscovery;

// Trouver un service par nom
let info = ServiceDiscovery::find_service("fs_service")?;

// Attendre qu'un service soit prêt
let info = ServiceDiscovery::wait_for_service("net_service", 5000)?;

// Lister tous les services
let services = ServiceDiscovery::list_services()?;
```

### 4. IPC Helpers

Patterns de communication simplifiés:

#### Request/Response (synchrone)

```rust
use services::RequestResponseClient;

// Client
let client = RequestResponseClient::<Req, Resp>::new("fs_service")?;
let response = client.request(my_request)?;

// Serveur
let server = RequestResponseServer::new(|req: Req| -> Result<Resp> {
    // Traiter la requête
    Ok(response)
});
server.serve()?; // Boucle infinie
```

#### Pub/Sub (asynchrone)

```rust
use services::{Publisher, Subscriber};

// Publisher
let pub = Publisher::<Event>::new("system_events")?;
pub.publish(Event::SystemStarted)?;

// Subscriber
let sub = Subscriber::<Event>::subscribe("system_events")?;
let event = sub.recv()?; // Bloquant
```

## Utilisation avec fs_service

Exemple d'intégration du framework dans fs_service:

```rust
// fs_service/src/main.rs
use services::{Service, ServiceRegistry, RequestResponseServer};

struct FsService {
    vfs: VFS,
    caps: ServiceCapabilities,
}

impl Service for FsService {
    // Implémentation...
}

fn main() {
    let service = FsService::new()?;
    let handle = ServiceRegistry::register(service)?;
    
    // Créer serveur IPC
    let server = RequestResponseServer::new(handle_request);
    
    ServiceRegistry::notify_ready(handle)?;
    server.serve()?;
}
```

## TODOs

- [ ] Implémentation IPC réelle (actuellement stubs)
- [ ] Intégration avec exo_ipc::Channel
- [ ] Sérialisation/désérialisation messages
- [ ] Message broker pour Pub/Sub
- [ ] Timeouts configurables
- [ ] Retry logic
