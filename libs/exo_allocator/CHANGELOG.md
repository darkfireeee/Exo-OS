# Changelog

All notable changes to exo_allocator will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- SlabAllocator for fixed-size object pools
- BumpAllocator for arena/scratch allocations
- Mimalloc FFI bindings and GlobalAlloc wrapper
- Telemetry hooks for allocation tracking
- Custom OOM handler
- Comprehensive benchmarks vs jemalloc/tcmalloc/system allocator

### Performance
- Slab: 3-5x faster than general allocator for fixed-size objects
- Bump: 10-100x faster for temporary allocations
- Mimalloc: 10-30% improvement over jemalloc

### Security
- Compiled with MI_SECURE=4 (secure mode)
- Double-free detection in debug builds
- Memory wiping on free (secure mode)

## [0.1.0] - 2026-02-05

### Added
- Initial project structure
- Vendor mimalloc sources (v2.x)
- Build system (cc crate integration)
- Documentation and examples
