# Changelog

All notable changes to exo_text will be documented in this file.

## [Unreleased]

### Added
- JSON streaming parser with serde integration
- TOML deserializer for configuration files
- Markdown parser with HTML and terminal renderers
- Unified diff generator
- SIMD optimizations for JSON parsing (when available)
- Zero-copy parsing where possible

### Performance
- JSON: 500-800 MB/s parsing throughput
- TOML: 50-100 MB/s parsing throughput
- Markdown: 100-200 MB/s rendering

### Features
- Feature flags for modular compilation
- Fuzzing harnesses for all parsers
- Property-based tests with proptest

## [0.1.0] - 2026-02-05

### Added
- Initial project structure
- Module organization (json, toml, markdown, diff)
- Documentation and examples
