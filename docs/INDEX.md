# 📚 Exo-OS Documentation Index

## Vue d'ensemble

Exo-OS est un système d'exploitation hybride ultra-performant conçu pour surpasser Linux.

**📖 Voir [README.md](README.md) pour la structure complète de la documentation organisée**

---

## 📂 Navigation Rapide

### 🎯 Documents Actuels
- **[Phase 1 Status](current/PHASE_1_STATUS.md)** - ✅ COMPLÈTE: fork/wait cycle
- **[Phase 2 Plan](current/PHASE_2_PLAN.md)** - 📋 Fork context copy & POSIX
- **[Phase 2 Quickstart](current/PHASE_2_QUICKSTART.md)** - 🚀 Guide démarrage rapide
- **[Roadmap](current/ROADMAP.md)** - 🗺️ Plan v1.0.0 "Linux Crusher"
- **[Module Status](current/MODULE_STATUS.md)** - 📊 État modules
- **[TODO](current/TODO.md)** - Liste tâches

### 🏗️ Architecture
- **[Architecture v0.5.0](architecture/ARCHITECTURE_v0.5.0.md)** - Vue d'ensemble
- **[Architecture Complète](architecture/ARCHITECTURE_COMPLETE.md)** - Détails complets
- **[Scheduler](architecture/SCHEDULER_DOCUMENTATION.md)** - 3-Queue EMA
- **[IPC](architecture/IPC_DOCUMENTATION.md)** - Inter-Process Communication
- **[POSIX-X](architecture/POSIX_X_SYSCALL_ANALYSIS.md)** - Analyse syscalls

### 📖 Guides
- **[Build & Test](guides/BUILD_AND_TEST_GUIDE.md)** - 🔨 Compilation et tests
- **[AI Integration](guides/AI_INTEGRATION.md)** - 🤖 IA dans Exo-OS
- **[Benchmarks](guides/exo-os-benchmarks.md)** - ⚡ Performance

---

## Catégories de Documentation Technique

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
