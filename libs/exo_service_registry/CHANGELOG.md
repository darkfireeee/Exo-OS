# Changelog

All notable changes to exo_service_registry will be documented in this file.

## [Unreleased]

### Added
- Service registry with HashMap storage
- Discovery client for service lookup
- Health checker with ping/pong heartbeat
- Persistent storage (SQLite or TOML)
- LRU cache for fast lookups
- Bloom filter for fast negative lookups
- Hot-reload detection (crashed service monitoring)
- Integration with init system

### Performance
- O(1) lookup with cache (<100ns)
- O(log n) registration
- <1ms health check (parallel ping)
- ~200 bytes per service

### Storage Backends
- SQLite (persistent, production)
- TOML file (lightweight, development)

## [0.1.0] - 2026-02-05

### Added
- Initial project structure
- Module organization
- Documentation and examples
