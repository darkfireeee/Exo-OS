# 📚 Exo-OS Documentation Index

## Vue d'ensemble

Exo-OS est un système d'exploitation hybride ultra-performant conçu pour surpasser Linux.

## Catégories de Documentation

### 📡 [IPC - Inter-Process Communication](./ipc/README.md)
Communication inter-processus haute performance (12-50x plus rapide que Linux).
- [Core Primitives](./ipc/core.md) - CoalesceController, CreditController, PriorityClass
- [UltraFastRing](./ipc/ultra_fast_ring.md) - Ring optimisé 80-100 cycles
- [Advanced Channels](./ipc/advanced_channels.md) - Priority, Multicast, Anycast, Request-Reply
- [Fusion Ring](./ipc/fusion_ring.md) - IPC adaptatif inline/zerocopy

### ⏱️ [Scheduler - Ordonnanceur](./scheduler/README.md)
Ordonnanceur 3-Queue avec prédiction EMA et context switch ultra-rapide.
- [3-Queue System](./scheduler/3_queue.md) - Hot/Normal/Cold queues
- [EMA Prediction](./scheduler/ema_prediction.md) - Prédiction adaptative
- [Context Switch](./scheduler/context_switch.md) - 304 cycles
- [Real-Time](./scheduler/realtime.md) - Deadline scheduling

### 🖥️ [x86_64 - Architecture](./x86_64/README.md)
Support complet de l'architecture x86_64.
- [Boot Sequence](./x86_64/boot.md) - GDT, IDT, TSS
- [CPU Features](./x86_64/cpu.md) - CPUID, MSRs, SIMD
- [Interrupts](./x86_64/interrupts.md) - APIC, IOAPIC, IPI
- [System Calls](./x86_64/syscall.md) - SYSCALL/SYSRET
- [User Mode](./x86_64/usermode.md) - Ring 3 transition, IRETQ/SYSRET

### 📦 [Loader - Chargeur ELF](./loader/elf_loader.md)
Chargeur d'exécutables ELF64.
- Support ET_EXEC et ET_DYN (PIE)
- Program headers (PT_LOAD, PT_TLS, PT_INTERP)
- Auxiliary vector pour _start

### 💾 [Memory - Gestion Mémoire](./memory/README.md)
Gestion mémoire physique et virtuelle.
- [Physical Memory](./memory/physical.md) - Frame allocator
- [Virtual Memory](./memory/virtual.md) - Page tables, TLB
- [Heap Allocator](./memory/heap.md) - Slab + Buddy
- [Shared Memory](./memory/shared.md) - IPC zero-copy

### 📁 [VFS - Virtual File System](./vfs/README.md)
Interface unifiée pour les systèmes de fichiers.
- [Inodes](./vfs/inodes.md) - Structure et opérations
- [Dentries](./vfs/dentries.md) - Directory entries
- [Mount Points](./vfs/mount.md) - Montage de FS

## Performance Highlights

### IPC vs Linux
| Opération | Exo-OS | Linux | Gain |
|-----------|--------|-------|------|
| Inline ≤40B | 80-100 cycles | ~1200 cycles | **12-15x** |
| Zero-copy | 200-300 cycles | ~1200 cycles | **4-6x** |
| Batch | 25-35 cycles/msg | ~1200 cycles | **35-50x** |

### Scheduler
| Opération | Exo-OS | Linux | Gain |
|-----------|--------|-------|------|
| Context switch | 304 cycles | ~1500 cycles | **5x** |
| Scheduling decision | ~50 cycles | ~200 cycles | **4x** |

## Versions

- **Current**: v0.5.0-dev
- **Stable**: v0.4.1

## Building

```bash
cargo build --release
```

## Running (QEMU)

```bash
qemu-system-x86_64 -kernel target/x86_64-unknown-none/release/exo-kernel
```
