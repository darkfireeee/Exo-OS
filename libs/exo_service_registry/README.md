# exo_service_registry

Service discovery and health checking for Exo-OS distributed services.

## Features

- **Service registration**: Register services with name and endpoint
- **Discovery**: Lookup services by name
- **Health checking**: Ping/pong heartbeat monitoring
- **Persistent storage**: SQLite or TOML file backend
- **Cache**: LRU cache for fast lookups
- **Bloom filter**: Fast negative lookups (service not found)
- **Hot-reload**: Detect crashed services and restart

## Architecture

```
exo_service_registry/
├── src/
│   ├── registry.rs     # HashMap<ServiceName, Endpoint>
│   ├── discovery.rs    # Client lookup API
│   ├── health.rs       # Heartbeat monitoring
│   └── storage.rs      # Persistent storage
```

## Usage

### Register Service

```rust
use exo_service_registry::Registry;

let mut registry = Registry::new();
registry.register("fs_service", "/tmp/fs.sock")?;
```

### Discover Service

```rust
use exo_service_registry::Discovery;

let discovery = Discovery::new();
let endpoint = discovery.find("fs_service")?;
println!("fs_service at: {}", endpoint);
```

### Health Monitoring

```rust
use exo_service_registry::HealthChecker;

let health = HealthChecker::new();
health.check_all()?; // Ping all registered services
```

## Service Conventions

Service names follow pattern: `{category}_{name}`

Examples:
- `fs_service`: Filesystem service
- `net_service`: Network service
- `logger_service`: Logging daemon
- `config_manager`: Configuration manager

## Storage

### SQLite Backend (Persistent)

```sql
CREATE TABLE services (
    name TEXT PRIMARY KEY,
    endpoint TEXT NOT NULL,
    registered_at INTEGER,
    last_heartbeat INTEGER
);
```

### TOML Backend (Lightweight)

```toml
[services.fs_service]
endpoint = "/tmp/fs.sock"
registered_at = 1707123456

[services.net_service]
endpoint = "/tmp/net.sock"
registered_at = 1707123457
```

## Performance

- **Lookup**: O(1) with cache, <100ns typical
- **Registration**: O(log n) with persistent storage
- **Health check**: Parallel ping, <1ms total
- **Memory**: ~200 bytes per service

## Cache Strategy

- **LRU cache**: 100 entries default
- **TTL**: 60 seconds default
- **Bloom filter**: 10K entries, 1% false positive rate

## References

- [Service Discovery Patterns](https://microservices.io/patterns/service-registry.html)
