# 🔖 BACKLOG - Fonctionnalités Non Implémentées

**Date de création**: 2 Janvier 2026  
**Version**: v0.7.0  
**Objectif**: Tracker toutes les fonctionnalités planifiées mais non encore implémentées

---

## 📋 Résumé Exécutif

Ce document recense **toutes les fonctionnalités non implémentées** des phases précédentes et du module réseau actuel. Il sert de référence pour éviter les oublis lors des développements futurs.

### Statistiques
- **Phase 1**: 5 items manquants
- **Phase 2**: 8 items manquants
- **Module Réseau**: 15 items manquants
- **Phase 3**: À implémenter intégralement (28 items)
- **Total**: 56 items

---

## 🔴 PHASE 1 - Items Manquants (5 items)

### Phase 1b - Drivers Input

#### ❌ Keyboard Driver PS/2 (5 tests manquants)
**Priorité**: Moyenne  
**Effort**: 2-3 jours  
**Blocage**: Aucun test keyboard ne passe actuellement

**À implémenter**:
- [ ] PS/2 controller initialization
- [ ] Scancode to keycode mapping
- [ ] Keyboard interrupt handler (IRQ1)
- [ ] Input buffer management
- [ ] Special keys (Shift, Ctrl, Alt, etc.)

**Tests manquants**:
1. test_keyboard_irq - IRQ1 handler
2. test_scancode_to_keycode - Mapping
3. test_buffer_management - Input buffer
4. test_special_keys - Modificateurs
5. test_keyboard_leds - LEDs status

**Fichier**: `kernel/src/drivers/keyboard.rs` (non créé)

---

### Phase 1c - Memory Management

#### ❌ Page Tables Multi-niveaux (>8GB support)
**Priorité**: Basse (future-proofing)  
**Effort**: 1 semaine  
**Limitation actuelle**: mmap limité à <8GB

**À implémenter**:
- [ ] PML4 (level 4) pour >512GB
- [ ] PDPT (level 3) gestion complète
- [ ] Support mémoire physique >8GB
- [ ] Tests avec grandes allocations

**Fichier**: `kernel/src/memory/paging.rs` (à étendre)

---

### Phase 1d - Processus

#### ❌ ELF Loader exec() complet
**Priorité**: Haute (nécessaire userland)  
**Effort**: 3-4 jours  
**Status**: Structure présente mais non testée

**À implémenter**:
- [ ] Tests exec() avec vrais binaires ELF
- [ ] Dynamic linking (ld.so support)
- [ ] Environment variables
- [ ] Command-line arguments parsing
- [ ] Binary validation complète

**Fichier**: `kernel/src/process/exec.rs` (à tester)

---

### Phase 1e - Shell Userland

#### ❌ Shell Interactif
**Priorité**: Moyenne  
**Effort**: 1 semaine  
**Dépendance**: Keyboard driver + exec()

**À implémenter**:
- [ ] Command parsing
- [ ] Built-in commands (cd, pwd, exit, etc.)
- [ ] Process launching
- [ ] Job control (fg/bg)
- [ ] Pipeline support

**Dossier**: `userland/shell/` (vide)

---

### Phase 1f - Tests Intégration

#### ❌ Tests Bout-en-Bout
**Priorité**: Moyenne  
**Effort**: Ongoing

**À implémenter**:
- [ ] Test boot to shell
- [ ] Test multi-process scenarios
- [ ] Stress tests
- [ ] Regression suite complète

**Dossier**: `tests/integration/` (partiel)

---

## 🟡 PHASE 2 - Items Manquants (8 items)

### Phase 2a - Scheduler

#### ❌ CFS (Completely Fair Scheduler)
**Priorité**: Basse (3-queue OK pour l'instant)  
**Effort**: 2 semaines  
**Alternative actuelle**: 3-queue scheduler fonctionnel

**À implémenter**:
- [ ] Red-Black tree pour runqueue
- [ ] vruntime tracking
- [ ] CFS load balancing
- [ ] Group scheduling

**Fichier**: `kernel/src/scheduler/cfs.rs` (non créé)

---

### Phase 2b - IPC

#### ❌ Shared Memory (shmget/shmat)
**Priorité**: Moyenne  
**Effort**: 3-4 jours  
**Status**: Structures présentes, implémentation partielle

**À implémenter**:
- [ ] shmget() syscall
- [ ] shmat() syscall
- [ ] shmdt() syscall
- [ ] shmctl() syscall
- [ ] Tests partagés entre processus

**Fichier**: `libs/exo_ipc/src/shm.rs` (partiel)

---

#### ❌ Message Queues (msgget/msgsnd/msgrcv)
**Priorité**: Basse  
**Effort**: 2-3 jours

**À implémenter**:
- [ ] msgget() syscall
- [ ] msgsnd() syscall
- [ ] msgrcv() syscall
- [ ] msgctl() syscall
- [ ] Priority queues

**Fichier**: Non créé

---

### Phase 2c - SMP

#### ❌ NUMA Support
**Priorité**: Très basse (future)  
**Effort**: 2 semaines  
**Contexte**: Pour systèmes multi-socket

**À implémenter**:
- [ ] NUMA topology detection
- [ ] NUMA-aware allocation
- [ ] CPU affinity policies
- [ ] Local vs remote memory tracking

**Fichier**: Non créé

---

### Phase 2d - Tests & Validation

#### ❌ Performance Benchmarks Complets
**Priorité**: Moyenne  
**Effort**: Ongoing

**À implémenter**:
- [ ] IPC latency benchmarks
- [ ] Context switch benchmarks
- [ ] Memory allocator benchmarks
- [ ] Comparaison avec Linux baseline

**Dossier**: `tests/benchmarks/` (partiel)

---

## 🌐 MODULE RÉSEAU - Items Manquants (15 items)

### Layer 3 - Network

#### ❌ IPv6 Support Complet
**Priorité**: Moyenne  
**Effort**: 1 semaine  
**Status**: Structures créées mais logique manquante

**À implémenter**:
- [ ] IPv6 header parsing/writing
- [ ] ICMPv6 (NDP, Router Discovery)
- [ ] IPv6 routing
- [ ] Dual-stack IPv4/IPv6
- [ ] IPv6 fragmentation
- [ ] IPv6 extension headers

**Fichier**: `kernel/src/net/ip.rs` (à étendre)  
**Tests**: 6 tests à créer

---

#### ❌ Fragmentation IPv4
**Priorité**: Moyenne  
**Effort**: 2-3 jours  
**Status**: Structures présentes, logique manquante

**À implémenter**:
- [ ] Fragmentation lors de l'envoi
- [ ] Réassemblage à la réception
- [ ] Timeout pour fragments incomplets
- [ ] Tests avec gros paquets (>MTU)

**Fichier**: `kernel/src/net/ip.rs` (fragment_* structs)  
**Tests**: 4 tests à créer

---

### Layer 4 - Transport

#### ❌ TCP Features Avancées
**Priorité**: Haute (pour production)  
**Effort**: 1-2 semaines

**À implémenter**:
- [ ] **Window Scaling** (RFC 1323)
  - Grande window pour high-bandwidth
  - Négociation lors handshake
  
- [ ] **Selective ACK (SACK)** (RFC 2018)
  - ACK sélectifs pour gaps
  - Fast retransmit amélioré
  
- [ ] **Fast Retransmit/Recovery** (RFC 2581)
  - Détection rapide de perte
  - Recovery sans timeout
  
- [ ] **Nagle's Algorithm** (RFC 896)
  - Buffering de petits paquets
  - Réduction overhead
  
- [ ] **Timestamps** (RFC 1323)
  - RTT measurement précis
  - PAWS protection

**Fichier**: `kernel/src/net/tcp.rs` (à étendre)  
**Tests**: 10 tests à créer

---

#### ❌ UDP Avancé
**Priorité**: Basse  
**Effort**: 1-2 jours

**À implémenter**:
- [ ] Multicast (IGMP)
- [ ] Broadcast
- [ ] Socket options (SO_BROADCAST, etc.)
- [ ] Large datagrams (fragmentation)

**Fichier**: `kernel/src/net/udp.rs` (à étendre)  
**Tests**: 4 tests à créer

---

### Drivers Réseau

#### ❌ Drivers Réseau Physiques
**Priorité**: Haute (pour hardware réel)  
**Effort**: 2-3 semaines

**À implémenter**:
- [ ] **E1000** (Intel Gigabit)
  - PCI enumeration
  - RX/TX ring buffers
  - Interrupt handling
  - Link detection
  
- [ ] **VirtIO-Net** (QEMU/KVM)
  - Virtqueue management
  - Feature negotiation
  - Zero-copy optimization
  
- [ ] **RTL8139** (Realtek)
  - Legacy driver (simple pour tests)
  
- [ ] **Intel WiFi (iwlwifi)**
  - Wrapper autour driver Linux
  - Firmware loading
  - WPA2 support

**Dossier**: `kernel/src/drivers/net/` (vide)  
**Tests**: 12 tests à créer (3 par driver)

---

### Services Réseau

#### ❌ DHCP Client
**Priorité**: Haute (auto-configuration)  
**Effort**: 3-4 jours

**À implémenter**:
- [ ] DHCP Discovery/Offer/Request/Ack
- [ ] Lease management
- [ ] Renewal automatique
- [ ] Option parsing (DNS, gateway, etc.)
- [ ] IPv4 auto-configuration

**Dossier**: `kernel/src/net/services/dhcp/` (structure vide)  
**Tests**: 6 tests à créer

---

#### ❌ DNS Client
**Priorité**: Haute (résolution noms)  
**Effort**: 2-3 jours

**À implémenter**:
- [ ] DNS query (A, AAAA records)
- [ ] Response parsing
- [ ] Cache DNS
- [ ] Recursive vs iterative
- [ ] /etc/resolv.conf parsing

**Dossier**: `kernel/src/net/services/dns/` (structure vide)  
**Tests**: 5 tests à créer

---

#### ❌ NTP Client
**Priorité**: Basse  
**Effort**: 1-2 jours

**À implémenter**:
- [ ] NTP packet format
- [ ] Time synchronization
- [ ] Offset calculation
- [ ] Clock adjustment

**Dossier**: `kernel/src/net/services/ntp/` (structure vide)  
**Tests**: 3 tests à créer

---

### Sécurité Réseau

#### ❌ Firewall/NAT
**Priorité**: Moyenne  
**Effort**: 1 semaine  
**Status**: Structures créées mais inactives

**À implémenter**:
- [ ] **Firewall Rules**
  - Packet filtering (IP/port/protocol)
  - Stateful inspection
  - Rule management
  - Default policies
  
- [ ] **NAT (Network Address Translation)**
  - SNAT (Source NAT)
  - DNAT (Destination NAT)
  - Connection tracking
  - Port forwarding

**Dossier**: `kernel/src/net/firewall/` (vide après cleanup)  
**Tests**: 8 tests à créer

---

#### ❌ IPsec/VPN
**Priorité**: Basse  
**Effort**: 2-3 semaines

**À implémenter**:
- [ ] IPsec ESP/AH
- [ ] IKEv2 key exchange
- [ ] Tunnel mode
- [ ] WireGuard support

**Dossier**: `kernel/src/net/vpn/` (vide après cleanup)  
**Tests**: 10 tests à créer

---

### Optimisations Réseau

#### ❌ Zero-Copy Networking
**Priorité**: Moyenne (performance)  
**Effort**: 1 semaine

**À implémenter**:
- [ ] sendfile() syscall
- [ ] splice() syscall
- [ ] DMA direct to NIC
- [ ] mmap'd packet buffers

**Fichier**: Nouveau module  
**Tests**: 4 tests à créer

---

#### ❌ TCP Offload (TSO/LRO)
**Priorité**: Basse (NIC-specific)  
**Effort**: 1 semaine

**À implémenter**:
- [ ] TCP Segmentation Offload
- [ ] Large Receive Offload
- [ ] Checksum offload
- [ ] NIC feature detection

**Fichier**: Driver-specific  
**Tests**: 3 tests à créer

---

#### ❌ QoS (Quality of Service)
**Priorité**: Basse  
**Effort**: 1 semaine  
**Status**: Structures créées mais inactives

**À implémenter**:
- [ ] Traffic shaping (Token bucket)
- [ ] Priority queues
- [ ] Bandwidth limits
- [ ] DiffServ support

**Dossier**: `kernel/src/net/qos/` (vide après cleanup)  
**Tests**: 5 tests à créer

---

### Protocoles Avancés

#### ❌ TLS/SSL
**Priorité**: Haute (sécurité)  
**Effort**: 2-3 semaines

**À implémenter**:
- [ ] TLS 1.3 handshake
- [ ] Certificate validation
- [ ] Cipher suites
- [ ] Session resumption
- [ ] Integration avec socket API

**Dossier**: Nouveau module (userspace possible)  
**Tests**: 12 tests à créer

---

#### ❌ HTTP/2 & QUIC
**Priorité**: Basse  
**Effort**: 2 semaines

**À implémenter**:
- [ ] HTTP/2 multiplexing
- [ ] HPACK compression
- [ ] QUIC protocol (UDP-based)
- [ ] 0-RTT support

**Dossier**: Userspace probable  
**Tests**: 8 tests à créer

---

## 🚀 PHASE 3 - À Implémenter Intégralement (28 items)

### Semaines 1-2: Driver Framework (7 items)

#### 🔲 Linux DRM Compatibility Layer
**Priorité**: Haute  
**Effort**: 1 semaine  
**Objectif**: Permettre l'utilisation de drivers Linux GPL-2.0

**À implémenter**:
- [ ] struct device abstraction
- [ ] struct driver abstraction
- [ ] Device tree/ACPI shims
- [ ] Power management hooks
- [ ] DMA API compatibility
- [ ] Kernel module loading (basique)
- [ ] GPL symbol exports

**Fichier**: `kernel/src/drivers/compat/linux.rs`  
**Tests**: 10 tests

---

#### 🔲 PCI Subsystem Complet
**Priorité**: Haute  
**Effort**: 3-4 jours

**À implémenter**:
- [ ] PCI bus enumeration (scan)
- [ ] Config space access
- [ ] BAR (Base Address Register) mapping
- [ ] PCI capabilities parsing
- [ ] Device identification (vendor/device ID)
- [ ] Resource allocation
- [ ] PCI bridges support

**Fichier**: `kernel/src/drivers/pci.rs`  
**Tests**: 8 tests

---

#### 🔲 MSI/MSI-X Support
**Priorité**: Haute (performance)  
**Effort**: 2-3 jours

**À implémenter**:
- [ ] MSI capability detection
- [ ] MSI-X table management
- [ ] Interrupt vector allocation
- [ ] Per-CPU delivery
- [ ] Legacy IRQ fallback

**Fichier**: `kernel/src/arch/x86_64/interrupts/msi.rs`  
**Tests**: 5 tests

---

#### 🔲 ACPI Support Basique
**Priorité**: Moyenne  
**Effort**: 1 semaine

**À implémenter**:
- [ ] RSDP/RSDT/XSDT parsing
- [ ] FADT (Fixed ACPI Description Table)
- [ ] MADT (Multiple APIC Description)
- [ ] Device enumeration
- [ ] Power management basics

**Fichier**: `kernel/src/acpi/`  
**Tests**: 6 tests

---

### Semaines 3-4: Network Drivers (4 items)

#### 🔲 VirtIO-Net Driver (Pure Rust)
**Priorité**: Haute  
**Effort**: 1 semaine

**À implémenter**:
- [ ] VirtIO PCI detection
- [ ] Virtqueue setup (RX/TX)
- [ ] Feature negotiation
- [ ] Packet send/receive
- [ ] Interrupt handling
- [ ] Link status detection

**Fichier**: `kernel/src/drivers/net/virtio_net.rs`  
**Tests**: 8 tests

---

#### 🔲 E1000 Driver Wrapper
**Priorité**: Haute  
**Effort**: 1 semaine

**À implémenter**:
- [ ] Linux e1000 driver wrapping
- [ ] GPL compatibility layer
- [ ] RX/TX ring setup
- [ ] DMA buffer management
- [ ] Interrupt handling
- [ ] EEPROM reading

**Fichier**: `kernel/src/drivers/net/e1000.rs`  
**Tests**: 6 tests

---

#### 🔲 RTL8139 Driver Wrapper
**Priorité**: Moyenne  
**Effort**: 3-4 jours

**À implémenter**:
- [ ] Linux 8139too driver wrapping
- [ ] Simple RX/TX
- [ ] Legacy interrupt handling
- [ ] Basic configuration

**Fichier**: `kernel/src/drivers/net/rtl8139.rs`  
**Tests**: 4 tests

---

#### 🔲 Intel WiFi (iwlwifi) Wrapper
**Priorité**: Basse (bonus)  
**Effort**: 2 semaines

**À implémenter**:
- [ ] Linux iwlwifi wrapping
- [ ] Firmware loading
- [ ] MAC80211 integration
- [ ] WPA2 support
- [ ] Scan/associate

**Fichier**: `kernel/src/drivers/net/iwlwifi.rs`  
**Tests**: 8 tests

---

### Semaines 5-6: Block Drivers (7 items)

#### 🔲 VirtIO-Blk Driver
**Priorité**: Haute  
**Effort**: 4-5 jours

**À implémenter**:
- [ ] VirtIO block device detection
- [ ] Request queue setup
- [ ] Read/write operations
- [ ] Flush support
- [ ] Discard/trim support

**Fichier**: `kernel/src/drivers/block/virtio_blk.rs`  
**Tests**: 6 tests

---

#### 🔲 AHCI/SATA Driver
**Priorité**: Haute  
**Effort**: 1 semaine

**À implémenter**:
- [ ] AHCI controller detection
- [ ] Port initialization
- [ ] Command queuing (NCQ)
- [ ] FIS (Frame Information Structure)
- [ ] Error handling
- [ ] Hot-plug detection

**Fichier**: `kernel/src/drivers/block/ahci.rs`  
**Tests**: 8 tests

---

#### 🔲 NVMe Driver Basique
**Priorité**: Moyenne  
**Effort**: 1 semaine

**À implémenter**:
- [ ] NVMe PCI detection
- [ ] Admin queue setup
- [ ] I/O queue pairs
- [ ] Submission/completion queues
- [ ] Namespace discovery
- [ ] Basic read/write

**Fichier**: `kernel/src/drivers/block/nvme.rs`  
**Tests**: 8 tests

---

#### 🔲 Block Layer (bio/request)
**Priorité**: Haute (infrastructure)  
**Effort**: 1 semaine

**À implémenter**:
- [ ] **struct bio** - Block I/O operations
  - Scatter-gather lists
  - Merging/splitting
  
- [ ] **Request Queue**
  - Elevator/scheduler (NOOP, Deadline)
  - Request merging
  - I/O prioritization
  
- [ ] **Block Device Interface**
  - Generic read/write
  - Partitioning support
  - Disk statistics

**Fichier**: `kernel/src/block/`  
**Tests**: 10 tests

---

### Semaines 7-8: Filesystems Réels (10 items)

#### 🔲 FAT32 Lecture
**Priorité**: Haute (compatibilité)  
**Effort**: 4-5 jours

**À implémenter**:
- [ ] Boot sector parsing
- [ ] FAT table reading
- [ ] Directory entry parsing
- [ ] File reading
- [ ] Long filename support (LFN)
- [ ] Readonly mount

**Fichier**: `kernel/src/fs/fat32.rs`  
**Tests**: 8 tests

---

#### 🔲 FAT32 Écriture
**Priorité**: Moyenne  
**Effort**: 3-4 jours

**À implémenter**:
- [ ] File creation
- [ ] File writing
- [ ] Directory creation
- [ ] File deletion
- [ ] FAT table update
- [ ] Sync/flush

**Fichier**: `kernel/src/fs/fat32.rs` (extension)  
**Tests**: 6 tests

---

#### 🔲 ext4 Lecture
**Priorité**: Haute (Linux compat)  
**Effort**: 1 semaine

**À implémenter**:
- [ ] Superblock parsing
- [ ] Inode reading
- [ ] Extent trees
- [ ] Directory indexing (htree)
- [ ] File reading
- [ ] Readonly mount

**Fichier**: `kernel/src/fs/ext4.rs`  
**Tests**: 10 tests

---

#### 🔲 ext4 Écriture Basique
**Priorité**: Moyenne  
**Effort**: 1-2 semaines

**À implémenter**:
- [ ] File creation
- [ ] File writing (simple)
- [ ] Directory creation
- [ ] Inode allocation
- [ ] Block allocation
- [ ] Journal basique (metadata only)

**Fichier**: `kernel/src/fs/ext4.rs` (extension)  
**Tests**: 12 tests

---

#### 🔲 Page Cache
**Priorité**: Haute (performance)  
**Effort**: 1 semaine

**À implémenter**:
- [ ] **Radix Tree** pour pages
  - Insertion/lookup O(log n)
  - Range queries
  
- [ ] **Read-ahead**
  - Sequential detection
  - Prefetch pages
  
- [ ] **Write-back**
  - Dirty page tracking
  - Periodic flush (pdflush-like)
  
- [ ] **Page reclaim**
  - LRU eviction
  - Memory pressure handling

**Fichier**: `kernel/src/fs/page_cache.rs`  
**Tests**: 8 tests

---

#### 🔲 Buffer Cache
**Priorité**: Moyenne  
**Effort**: 3-4 jours

**À implémenter**:
- [ ] Block-level caching
- [ ] Hash table for buffers
- [ ] Dirty buffer tracking
- [ ] Sync/flush operations

**Fichier**: `kernel/src/fs/buffer_cache.rs`  
**Tests**: 5 tests

---

#### 🔲 VFS Extensions
**Priorité**: Haute  
**Effort**: 3-4 jours

**À implémenter**:
- [ ] mount/umount syscalls
- [ ] Filesystem registration
- [ ] Superblock operations
- [ ] Inode cache global
- [ ] Dentry cache (dcache)

**Fichier**: `kernel/src/fs/vfs.rs` (extension)  
**Tests**: 8 tests

---

## 📊 Récapitulatif par Priorité

### Priorité Haute (23 items)
- ELF Loader exec() (Phase 1)
- TCP Features Avancées (Réseau)
- Drivers Réseau Physiques (Réseau)
- DHCP Client (Réseau)
- DNS Client (Réseau)
- TLS/SSL (Réseau)
- Linux DRM Compatibility (Phase 3)
- PCI Subsystem (Phase 3)
- MSI/MSI-X (Phase 3)
- VirtIO-Net Driver (Phase 3)
- E1000 Driver (Phase 3)
- VirtIO-Blk Driver (Phase 3)
- AHCI Driver (Phase 3)
- Block Layer (Phase 3)
- FAT32 Lecture (Phase 3)
- ext4 Lecture (Phase 3)
- Page Cache (Phase 3)
- VFS Extensions (Phase 3)

### Priorité Moyenne (18 items)
- Keyboard Driver (Phase 1)
- Shell Interactif (Phase 1)
- Tests Bout-en-Bout (Phase 1)
- Shared Memory IPC (Phase 2)
- Performance Benchmarks (Phase 2)
- IPv6 Support (Réseau)
- Fragmentation IPv4 (Réseau)
- Firewall/NAT (Réseau)
- Zero-Copy Networking (Réseau)
- ACPI Support (Phase 3)
- RTL8139 Driver (Phase 3)
- NVMe Driver (Phase 3)
- FAT32 Écriture (Phase 3)
- ext4 Écriture (Phase 3)
- Buffer Cache (Phase 3)

### Priorité Basse (15 items)
- Page Tables >8GB (Phase 1)
- CFS Scheduler (Phase 2)
- Message Queues IPC (Phase 2)
- NUMA Support (Phase 2)
- UDP Avancé (Réseau)
- NTP Client (Réseau)
- IPsec/VPN (Réseau)
- TCP Offload (Réseau)
- QoS (Réseau)
- HTTP/2 & QUIC (Réseau)
- Intel WiFi (Phase 3)

---

## 🎯 Dépendances Critiques

### Chaîne de Dépendances

```
Keyboard Driver → Shell Interactif → Tests E2E
                                    ↓
                          Tests Userland Complets

PCI Subsystem → Drivers Réseau/Block → Filesystems Réels
              ↓
          MSI/MSI-X → Performance Optimale

DHCP + DNS → Network Auto-Config → Production Ready
           ↓
      TLS/SSL → Secure Communications

Page Cache → FS Performance → Production Ready
```

---

## 📅 Planning Suggéré

### Semaine 1 (Phase 3)
- Linux DRM Compatibility Layer
- PCI Subsystem

### Semaine 2
- MSI/MSI-X Support
- ACPI Basique

### Semaine 3
- VirtIO-Net Driver
- Tests réseau

### Semaine 4
- E1000 Driver
- Network stack integration

### Semaine 5
- VirtIO-Blk Driver
- AHCI Driver basics

### Semaine 6
- Block Layer
- AHCI Driver complete

### Semaine 7
- FAT32 Lecture
- ext4 Lecture basics

### Semaine 8
- Page Cache
- ext4 Lecture complete

---

## 🔖 Notes d'Implémentation

### Principes Généraux

1. **Tests First**: Écrire tests avant implémentation
2. **Incrémental**: Features basiques d'abord
3. **Documentation**: Inline docs + README par module
4. **Compatibilité**: Garder API stable
5. **Performance**: Profiler avant optimiser

### Outils de Suivi

- Ce fichier (BACKLOG.md) - Référence globale
- GitHub Issues - Tracking détaillé par item
- ROADMAP.md - Vue stratégique
- Sprint docs - Planning 2 semaines

---

## 📝 Historique des Updates

| Date | Version | Changements |
|------|---------|-------------|
| 2026-01-02 | v1.0 | Création initiale post v0.7.0 |

---

**Maintenu par**: ExoOS Team  
**Dernière mise à jour**: 2 Janvier 2026  
**Prochaine revue**: Fin Phase 3
