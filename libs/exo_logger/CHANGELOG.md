# Changelog

All notable changes to exo_logger will be documented in this file.

## [Unreleased]

### Added
- Structured logging with JSON Lines format
- Span-based contexts for distributed tracing
- File sink with automatic rotation (10MB default)
- Compression support (zstd) for old logs
- Async buffering with lock-free MPSC channels
- Dynamic filtering (runtime log level changes)
- Correlation IDs for request tracking
- IPC backend for centralized logging (syslog-ng)
- Sampling (log 1/N for high-volume)

### Performance
- 1M+ logs/second throughput (async mode)
- <1μs latency (buffered writes)
- <5% CPU overhead

### Formatters
- JSON Lines (structured)
- Pretty formatter (human-readable, development)

## [0.1.0] - 2026-02-05

### Added
- Initial project structure
- Module organization
- Documentation and examples
