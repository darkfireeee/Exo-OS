# Changelog

All notable changes to exo_metrics will be documented in this file.

## [Unreleased]

### Added
- Metrics registry with thread-local counters
- Prometheus exporter (text format)
- Counter, Gauge, Histogram types
- Timer helpers for latency measurement
- System metrics collector (CPU, memory, I/O)
- Lock-free atomic metrics
- Historical percentiles (P50, P95, P99)

### Performance
- <1% CPU overhead for typical workloads
- Lock-free counter operations
- Thread-local caching
- ~100 bytes memory per metric

### Exporters
- Prometheus text format
- HTTP `/metrics` endpoint (via simple HTTP server)

## [0.1.0] - 2026-02-05

### Added
- Initial project structure
- Module organization
- Documentation and examples
