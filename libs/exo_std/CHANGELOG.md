# Changelog

All notable changes to exo_std will be documented in this file.

## [Unreleased]

### Added
- Process management APIs (spawn, fork, exec, wait)
- File I/O operations
- Synchronization primitives (Mutex, RwLock, Atomic)
- Thread spawning and management
- IPC primitives
- Time APIs (monotonic, realtime)
- Security/capability primitives
- Collections module (RingBuffer, BoundedVec, IntrusiveList, RadixTree) - planned

### Design
- No implicit allocations
- Zero-cost abstractions
- Capability-based security
- Custom syscall layer integration

## [0.1.0] - 2026-02-05

### Added
- Initial module structure
- Process, IO, Sync, Thread, IPC, Time, Security modules
- Basic stubs and type definitions
