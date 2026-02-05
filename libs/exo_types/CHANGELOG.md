# Changelog

All notable changes to exo_types will be documented in this file.

## [Unreleased]

### Added
- Pid newtype for process IDs
- FileDescriptor with RAII (auto-close on drop)
- Complete errno mapping (POSIX + Exo custom codes)
- Timestamp (monotonic and realtime)
- Signal type-safe numbers
- Uid/Gid newtypes
- Syscall number constants
- Zero-copy serialization (bytemuck, serde)
- Property-based tests for conversions

### Design
- Repr(transparent) for zero-cost abstractions
- Type safety preventing ID mixing
- Automatic resource cleanup (RAII)

## [0.1.0] - 2026-02-05

### Added
- Initial module structure
- Basic type definitions
