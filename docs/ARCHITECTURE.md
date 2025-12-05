# Exo-OS Architecture Documentation

## Overview

Exo-OS is a next-generation operating system designed to exceed Linux in performance, security, and user experience while maintaining maximum POSIX compatibility via the POSIX-X layer.

## Key Performance Targets

| Component | Metric | Exo-OS Target | Linux Baseline | Improvement |
|-----------|--------|---------------|----------------|-------------|
| **IPC** | Latency (inline) | 347 cycles | 1247 cycles | **3.6x faster** |
| **Context Switch** | Latency | 304 cycles | 2134 cycles | **7x faster** |
| **Allocator** | Thread-local | 8 cycles | ~50 cycles | **6.25x faster** |
| **Scheduler** | Pick next | 87 cycles | ~200 cycles | **2.3x faster** |
| **Audio** | Latency | < 1 ms | ~5 ms | **5x faster** |
| **Boot** | To desktop | < 3 sec | ~8 sec | **2.7x faster** |

## Architecture Layers

```
┌─────────────────────────────────────────────────────────────────┐
│                    LAYER 4: APPLICATIONS                        │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐          │
│  │  POSIX Apps  │  │ Native Apps  │  │  AI Agents   │          │
│  │  (Firefox,   │  │  (Rust apps) │  │  (6 agents)  │          │
│  │   nginx...)  │  │              │  │              │          │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘          │
└─────────┼──────────────────┼──────────────────┼─────────────────┘
          │                  │                  │
┌─────────┼──────────────────┼──────────────────┼─────────────────┐
│         ▼                  ▼                  ▼                 │
│                LAYER 3: USERSPACE RUNTIME                       │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                    POSIX-X Layer                          │  │
│  │  • 3-path execution (Fast/Hybrid/Legacy)                  │  │
│  │  • Capability cache (90%+ hit rate)                       │  │
│  │  • Zero-copy optimization                                 │  │
│  └──────────────────────────────────────────────────────────┘  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                    exo_std (Native API)                   │  │
│  │  • Zero-copy IPC (Fusion Rings)                           │  │
│  │  • Capability-based I/O                                   │  │
│  └──────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
          │
┌─────────┼───────────────────────────────────────────────────────┐
│         ▼                                                        │
│                LAYER 2: SYSTEM SERVICES                          │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐           │
│  │  Cosmic  │ │ Network  │ │  Audio   │ │  Power   │           │
│  │ Desktop  │ │ Service  │ │ Service  │ │ Service  │           │
│  └──────────┘ └──────────┘ └──────────┘ └──────────┘           │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐           │
│  │ Package  │ │Container │ │ Firmware │ │  Backup  │           │
│  │ Manager  │ │ Runtime  │ │ Manager  │ │ Service  │           │
│  └──────────┘ └──────────┘ └──────────┘ └──────────┘           │
└─────────────────────────────────────────────────────────────────┘
          │
┌─────────┼───────────────────────────────────────────────────────┐
│         ▼                                                        │
│                LAYER 1: KERNEL (< 50K LoC)                       │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │              Core Innovations                               │ │
│  │  • Fusion Rings IPC        (347 cycles vs 1247 Linux)       │ │
│  │  • Windowed Context Switch (304 cycles vs 2134 Linux)       │ │
│  │  • 3-Level Allocator       (8 cycles thread-local)          │ │
│  │  • Predictive Scheduler    (87 cycles pick avg)             │ │
│  └────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

## Directory Structure

```
exo-os/
├── kernel/                     # Kernel core (< 50K LoC)
│   └── src/
│       ├── ipc/               # Inter-process communication
│       │   └── fusion_ring/   # Lock-free ring buffer IPC
│       ├── scheduler/         # Process scheduling
│       │   ├── core/          # Predictive scheduler
│       │   └── switch/        # Windowed context switch
│       ├── memory/            # Memory management
│       │   └── heap/          # 3-level hybrid allocator
│       └── security/          # Capability-based security
│
├── posix_x/                   # POSIX compatibility layer
│   └── src/
│       ├── kernel_interface/  # Capability cache
│       ├── optimization/      # Zero-copy, batch optimization
│       ├── translation/       # Syscall translation
│       └── signals/           # Signal handling
│
├── userland/
│   ├── desktop/
│   │   └── cosmic/            # Cosmic Desktop Environment
│   │       ├── cosmic-comp/   # Wayland compositor
│   │       ├── cosmic-panel/  # Panel/Taskbar
│   │       ├── cosmic-launcher/
│   │       ├── cosmic-settings/
│   │       ├── cosmic-files/
│   │       └── cosmic-term/
│   │
│   ├── services/
│   │   ├── audio_service/     # PipeWire-like audio (< 1ms latency)
│   │   ├── power_service/     # Power management
│   │   └── ...
│   │
│   └── package_manager/       # OSTree-based atomic updates
│
├── libs/
│   ├── exo_types/             # Common types
│   ├── exo_std/               # Standard library
│   ├── exo_ipc/               # IPC primitives
│   └── exo_crypto/            # Cryptography
│
└── tools/                     # Development tools
```

## POSIX-X Layer

The POSIX-X layer provides Linux application compatibility with a 3-path execution strategy:

### Execution Paths

| Path | Usage | Latency | Example Syscalls |
|------|-------|---------|------------------|
| **Fast** | 70% | < 50 cycles | getpid, clock_gettime, brk |
| **Hybrid** | 25% | 400-1000 cycles | open, read, write, mmap |
| **Legacy** | 5% | 8000-50000 cycles | fork, execve, ptrace |

### Capability Cache

```
Cache Configuration:
- Size: 1024 entries (LRU eviction)
- Hit latency: ~50 cycles
- Miss latency: ~2000 cycles
- Target hit rate: 90%+
```

## Audio Service

Ultra-low latency audio using Fusion Rings:

- **Target latency**: < 1 ms (vs 5 ms PipeWire Linux)
- **Buffer size**: 48 samples at 48kHz (~1ms)
- **IPC**: Fusion Rings (347 cycles inline)
- **Zero-copy**: Shared memory for large buffers

## Build & Run

```bash
# Build kernel
make build

# Build with release optimizations
make release

# Run in QEMU
make qemu

# Run tests
make test
```

## Contributing

See [CONTRIBUTING.md](../CONTRIBUTING.md) for contribution guidelines.
