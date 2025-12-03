# 📡 IPC - Inter-Process Communication

## Vue d'ensemble

Le sous-système IPC d'Exo-OS est conçu pour **écraser les performances de Linux** avec des latences 12-50x plus rapides.

## Architecture

```
kernel/src/ipc/
├── core/                    # Primitives fondamentales
│   ├── advanced.rs          # Coalescing, Credits, Priorities
│   ├── ultra_fast_ring.rs   # Ring 80-100 cycles
│   ├── advanced_channels.rs # Priority/Multicast/Anycast
│   ├── mpmc_ring.rs         # MPMC lock-free
│   ├── sequence.rs          # Disruptor-style sequences
│   ├── futex.rs             # Futex userspace
│   └── ...
├── channel/                 # Canaux haut niveau
├── fusion_ring/             # Adaptive inline/zerocopy
├── shared_memory/           # Zero-copy transfers
└── named.rs                 # Named pipes
```

## Performance vs Linux

| Opération | Exo-OS | Linux Pipes | Avantage |
|-----------|--------|-------------|----------|
| Inline ≤40B | 80-100 cycles | ~1200 cycles | **12-15x** |
| Zero-copy | 200-300 cycles | ~1200 cycles | **4-6x** |
| Batch | 25-35 cycles/msg | ~1200 cycles | **35-50x** |
| Futex | ~20 cycles | ~50 cycles | **2.5x** |

## Modules

- [Core Primitives](./core.md)
- [UltraFastRing](./ultra_fast_ring.md)
- [Advanced Channels](./advanced_channels.md)
- [Fusion Ring](./fusion_ring.md)
