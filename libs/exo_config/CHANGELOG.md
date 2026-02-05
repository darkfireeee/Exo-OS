# Changelog

All notable changes to exo_config will be documented in this file.

## [Unreleased]

### Added
- Multi-source configuration loading
- Hierarchical merge strategy (user > system > default)
- Hot-reload with file watching
- Schema validation support
- Auto-migration of old config formats
- Environment variable overrides
- Lazy loading of config sections
- Memoized parsing cache

### Performance
- Zero allocations for cache hits
- Incremental updates on file changes
- Memory-efficient config storage

## [0.1.0] - 2026-02-05

### Added
- Initial project structure
- Module organization
- Documentation and examples
