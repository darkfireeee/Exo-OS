# 💾 Memory Management

## Vue d'ensemble

Le système de mémoire d'Exo-OS utilise une architecture moderne avec support complet de la pagination x86_64.

## Architecture

```
kernel/src/memory/
├── address.rs        # Types PhysicalAddress, VirtualAddress
├── frame_allocator.rs # Allocateur de frames physiques
├── heap/             # Allocateur heap kernel
├── virtual_mem/      # Mémoire virtuelle
├── physical/         # Gestion mémoire physique
├── shared/           # Mémoire partagée (IPC)
├── mmap.rs           # Memory mapping
├── protection.rs     # Protections de pages
├── cache.rs          # Cache management
├── dma.rs            # DMA buffers
└── pat.rs            # Page Attribute Table
```

## Modules

- [Physical Memory](./physical.md)
- [Virtual Memory](./virtual.md)
- [Heap Allocator](./heap.md)
- [Shared Memory](./shared.md)
