<div align="center">

```
███████╗██╗  ██╗ ██████╗       ██████╗ ███████╗
██╔════╝╚██╗██╔╝██╔═══██╗     ██╔═══██╗██╔════╝
█████╗   ╚███╔╝ ██║   ██║     ██║   ██║███████╗
██╔══╝   ██╔██╗ ██║   ██║     ██║   ██║╚════██║
███████╗██╔╝ ██╗╚██████╔╝     ╚██████╔╝███████║
╚══════╝╚═╝  ╚═╝ ╚═════╝       ╚═════╝ ╚══════╝
```

### Microkernel Hybride Haute Performance

[![Status](https://img.shields.io/badge/status-en%20développement-orange?style=flat-square)](.)
[![Rust](https://img.shields.io/badge/Rust-no__std%20nightly-orange?style=flat-square&logo=rust)](.)
[![Arch](https://img.shields.io/badge/cible-x86__64%20·%20aarch64-blue?style=flat-square)](.)
[![Preuves](https://img.shields.io/badge/preuves-Coq%20·%20TLA%2B-8b5cf6?style=flat-square)](.)
[![Crypto](https://img.shields.io/badge/crypto-XChaCha20--Poly1305-22c55e?style=flat-square)](.)
[![Licence](https://img.shields.io/badge/licence-MIT-lightgrey?style=flat-square)](.)

<br>

*"Make it work, make it right, make it fast."*

<br>

</div>
# Exo-OS

Système d'exploitation écrit intégralement en Rust, conçu autour d'une architecture exokernel modifiée. Le kernel Ring 0 expose les ressources matérielles brutes via des syscalls capability-gated. Les drivers, services système et applications s'exécutent hors du kernel dans des processus isolés.

> **État du projet** — En développement actif
> Modules terminés : `exo-boot` · `ipc` · `scheduler` · `memory` · `syscall` · `arch` · `security`


---

## Pourquoi Exo-OS

La majorité des OS modernes placent les drivers dans le kernel. Un driver bugué provoque une panique système complète. Exo-OS résout ce problème structurellement : chaque driver est un processus Ring 1 isolé. Un crash de driver est récupéré par le kernel sans impact sur le reste du système.

Le second objectif est la performance prévisible. Le scheduler atteint `pick_next_task()` en 100–150 cycles. Le context switch en 500–800 cycles. L'IPC small message en 500–700 cycles. Ces chiffres sont des contraintes de conception, pas des objectifs aspirationnels.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  HARDWARE                                                       │
│       ↓  UEFI Secure Boot / BIOS                                │
│  exo-boot/          Bootloader séparé — vérifie signature Ed25519│
│       ↓  BootInfo* handoff                                      │
│  KERNEL Ring 0      memory · scheduler · security · ipc · fs    │
│       ↓  syscalls capability-gated                              │
│  Ring 1 — Drivers   ahci · nvme · e1000 · ps2 · framebuffer…   │
│  Ring 1 — Servers   init · shield · net_stack · crypto_server… │
│       ↓  libexo / libc_exo                                      │
│  Ring 3 — Apps      shell · coreutils · compositor…            │
└─────────────────────────────────────────────────────────────────┘
```

### Couches kernel (Ring 0)

| Couche | Module | Dépendances |
|--------|--------|-------------|
| 0 | `memory/` | aucune |
| 1 | `scheduler/` | `memory/` |
| 1.5 | `process/` | `memory/` · `scheduler/` |
| 2a | `ipc/` | `memory/` · `scheduler/` · `security/` |
| 3 | `fs/` | `memory/` · `scheduler/` · `security/` |
| transversal | `security/` | `memory/` |
| transversal | `arch/` | aucune |
| interface | `syscall/` | tous |

La règle fondamentale : **aucune dépendance remontante**. `memory/` ne connaît pas `scheduler/`. `scheduler/` ne connaît pas `ipc/`. Les dépendances circulaires sont résolues par des traits abstraits enregistrés au boot (`DmaWakeupHandler`, `ElfLoader`).

---

## Modules kernel

### `memory/` — Couche 0

Buddy allocator O(log n), slab/SLUB, per-CPU pools lock-free, NUMA-aware. EmergencyPool pour les allocations IRQ sans heap. Gestion DMA avec isolation IOMMU par device (Intel VT-d, AMD-Vi). Page tables 4 niveaux, KPTI, CoW, THP 2 MB.

Le module DMA reste dans `memory/` malgré son besoin de réveiller des threads : la dépendance vers `process/` est inversée via le trait `DmaWakeupHandler` enregistré au boot. Couche 0 reste sans dépendance remontante.

### `scheduler/` — Couche 1

CFS + RT (SCHED_FIFO/RR) + EDF (SCHED_DEADLINE) + IDLE. Context switch complet : registres callee-saved, MXCSR, x87 FCW, CR3 (KPTI). FPU lazy avec `XSAVE`/`XRSTOR`. Load balancing SMP NUMA-aware.

L'IA-guided scheduling (`ai_guided.rs`) utilise une EMA 8 bytes inline dans le TCB — classification `IoBound`/`CpuBound`/`Mixed`/`RealtimeCandidate` en O(1). Fallback CFS immédiat si désactivé.

### `security/` — Transversal

Système de capability avec token 128 bits inforgeable : `[ObjectId:64][Rights:16][Generation:32][Tag:16]`. Révocation O(1) par incrémentation de génération — aucun token révoqué n'est utilisable sans parcours de liste.

Tous les accès passent par `security::access_control::check_access()` qui appelle `capability::verify()` et logue automatiquement dans le ring buffer d'audit. Zero Trust, crypto (XChaCha20-Poly1305, Blake3, Ed25519, X25519, AES-256-GCM), KASLR, KPTI, Retpoline, SSBD, CET Shadow Stack.

> Les invariants de sécurité (`INV-1` à `INV-5`) sont couverts par `proptest` (1000+ cas aléatoires) et un CI strict qui bloque toute tentative de bypass de `capability::verify()`. La preuve formelle Coq/TLA+ a été abandonnée en faveur de cette approche pragmatique.

### `ipc/` — Couche 2a

SPSC lock-free (Release/Acquire), MPMC, canaux synchrones/asynchrones, zero-copy (partage de page physique), SHM NUMA-aware. Fast IPC via fichier ASM (évite le syscall). Fusion Ring avec batching adaptatif anti-thundering-herd.

Toutes les vérifications de droits passent par `security::access_control::check_access()` — plus de bridge intermédiaire depuis la v6.

### `fs/` — Couche 3

Trois systèmes de fichiers distincts et isolés :

**`fs/ext4plus/`** — Système principal Exo-OS. Format propriétaire, incompatible ext4 standard par conception (incompat flags `EXO_BLAKE3 | EXO_DELAYED | EXO_REFLINK`). Linux refuse de monter ce disque — les données sont protégées. Fonctionnalités :
- **Data=Ordered** : WAL sur métadonnées seules, données écrites directement à destination finale. Write amplification divisée par deux vs mode `Data=Journal`.
- **Delayed Allocation** : aucun bloc alloué pendant `write()`. Allocation groupée au writeback (toutes les ~5s) en un seul bloc contigu. Les fichiers temporaires n'atteignent jamais le disque.
- **Reflinks** : copie instantanée de fichiers par partage de blocs physiques. CoW déclenché uniquement sur le bloc modifié.
- **Blake3** checksums sur toutes les écritures.

**`drivers/fs/ext4/`** — Ext4 classique pour disques Linux. Vérification stricte des flags `INCOMPAT` avant montage. Journal pas rejoué si `needs_recovery` — montage read-only proposé. Format 100% compatible Linux.

**`drivers/fs/fat32/`** — Clés USB et échange universel (Windows/Linux/Mac). Monté obligatoirement avec `MS_NOEXEC`. Validation BPB par calcul du cluster count (spec Microsoft exacte).

### `process/` — Couche 1.5

`fork()` CoW < 1 µs. Création thread < 500 ns. Signaux POSIX dans `process/signal/` (déplacés depuis `scheduler/` — le scheduler lit uniquement `signal_pending: AtomicBool` dans le TCB). Namespaces PID/mount/net/UTS/user. cgroups v2.

### `arch/`

x86_64 complet : GDT, IDT, TSS + IST stacks, APIC local + I/O, x2APIC, ACPI (MADT, HPET), SMP (INIT + SIPI), hotplug CPU. Mitigations Spectre/Meltdown : KPTI, Retpoline, SSBD per-thread, IBRS/IBPB/STIBP. ARM64 placeholder.

---

## Bootloader — `exo-boot/`

Binaire séparé du kernel (deux crates indépendantes, zéro partage de code). Deux chemins : UEFI (chemin principal, machines modernes) et BIOS legacy (VMs).

Chaîne de confiance : UEFI Secure Boot vérifie `exo-boot`, qui vérifie la signature Ed25519 du kernel avant chargement. Un kernel non signé est refusé si Secure Boot est actif.

Handoff via `BootInfo*` (contrat versionné avec magic + version) : carte mémoire unifiée E820/UEFI, framebuffer GOP, ACPI RSDP, 64 bytes d'entropy pour KASLR + CSPRNG, adresse base réelle après randomisation PIE.

---

## Drivers — `drivers/`

Chaque driver est un **processus Ring 1 séparé**. Accès hardware exclusivement via syscalls `exo_*` avec `CapToken` vérifié par le kernel :

```
exo_map_mmio(phys, size, cap)      → VirtAddr mappée dans l'espace du driver
exo_request_irq(irq, handler, cap) → handler exécuté dans le thread du driver
exo_alloc_dma_buffer(size, cap)    → buffer DMA-safe (physiquement contigu)
exo_iommu_bind(device_id, domain)  → isolation IOMMU
```

Le `driver_manager` (PID 2) supervise tous les drivers : probe ACPI/PCI → attribution des capabilities → lancement. Politique de redémarrage automatique : 3 crashs en 60 secondes → abandon + log.

Drivers inclus : AHCI, NVMe, VirtIO Block/Net/GPU, E1000, PS/2, USB HID, framebuffer GOP, Intel HD Audio, TTY/PTY, RTC/HPET.

---

## Services système — `servers/`

| Service | PID | Rôle |
|---------|-----|------|
| `init` | 1 | Superviseur de tous les services |
| `driver_manager` | 2 | Supervision drivers Ring 1 |
| `shield` | — | Anti-malware Ring 1, ML < 100 µs, sandboxé |
| `net_stack` | — | TCP/IP Ring 1 (crash réseau ≠ crash système) |
| `crypto_server` | — | Source unique de crypto — aucun autre service ne ré-implémente |
| `ipc_broker` | — | Directory service — lookup nom → endpoint cap |
| `vfs_server` | — | Montages, namespaces, /proc /sys /dev |
| `login_manager` | — | Authentification, seul habilité à gérer UID/GID |
| `power_manager` | — | ACPI, cpufreq, suspend S3/S4 |

---

## Bibliothèques — `libs/`

- **`libexo/`** — API système native : syscalls Rust safe, IPC, mmap, threads, futex
- **`libc_exo/`** — Compatibilité POSIX subset : stdio, stdlib, string, pthread, signal
- **`libexo_net/`** — Sockets : socket/bind/connect/listen/accept/send/recv
- **`libexo_ui/`** — UI minimale : fenêtres, canvas 2D, events input, rendu texte

---

## Userspace — `userspace/`

Shell POSIX (job control, history, completion), coreutils (ls/cp/mv/grep/find/awk/sed/ps/top…), net_tools (ping/curl/ssh), éditeur vi-like, gestionnaire de paquets avec résolution SAT + vérification signature, compositor graphique protocole IPC Wayland-réduit.

---

## Outillage — `tools/`

| Outil | Rôle |
|-------|------|
| `ai_trainer/` | Entraînement offline des tables IA kernel (NUMA hints, seuils EMA) |
| `exo-trace/` | Collecte de traces kernel (context switch, alloc, IRQ) |
| `exo-debug/` | Débogueur kernel (GDB remote, breakpoints hardware DR0-DR3) |
| `exo-bench/` | Benchmarks micro (context switch, IPC, allocateur) et macro |
| `mkimage/` | Génération image disque GPT + signature Ed25519 kernel |
| `exo-ci/` | Pipeline CI : build + tests QEMU + proptest + bench baseline |

---

## Sécurité

**Modèle de capability.** Token 128 bits inforgeable. Révocation O(1) par génération++. Délégation uniquement vers un sous-ensemble de droits (AND binaire — garanti par construction). Point de vérification unique : `security::access_control::check_access()`. Logging audit automatique sur chaque refus.

**Mitigations hardware.** KPTI (Meltdown), Retpoline sur tous les appels indirects hot path, SSBD par thread switché avec le contexte, IBRS/IBPB/STIBP, SMEP, SMAP, NX/XD, PKU, CET Shadow Stack (Intel), KASLR, stack canaries, CFI.

**Intégrité.** Secure Boot (chaîne exo-boot → kernel), signatures Ed25519 sur tous les binaires, Blake3 checksums sur toutes les écritures ext4plus, WAL journaling, Shield daemon (anti-malware Ring 1).

**Isolation.** Chaque driver dans son propre processus Ring 1. IOMMU par device (pas d'accès DMA cross-device). Namespaces (PID, mount, net, UTS, user). Sandbox pledge-style. W^X strict partout.

---

## Performance — Objectifs et contraintes

| Métrique | Cible |
|----------|-------|
| `pick_next_task()` | 100–150 cycles |
| Context switch complet | 500–800 cycles |
| IPC small msg (< 40B) | 500–700 cycles |
| IPC throughput zero-copy | > 100M msgs/s |
| `fork()` | < 1 µs |
| Création thread | < 500 ns |
| IPI latence SMP | < 10 µs |
| Allocation buddy | O(log n) |
| Révocation capability | O(1) |

Ces chiffres sont des **contraintes** documentées dans les règles de chaque module. Un hot path qui dépasse son budget est un bug d'architecture, pas une optimisation à faire plus tard.

**IA kernel.** Deux modules d'inférence statique : hints NUMA (`ai_hints.rs`, table 2 KB en `.rodata`) et classification thread (`ai_guided.rs`, EMA 8 bytes inline dans TCB). Entraînement exclusivement offline (`tools/ai_trainer/`). Fallback déterministe garanti si désactivé. Zéro allocation, zéro inférence dynamique en Ring 0.

---

## Structure du dépôt

```
exo-os/
├── kernel/          # Ring 0 — arch/ memory/ scheduler/ security/ ipc/ fs/ process/ syscall/
├── exo-boot/        # Bootloader UEFI + BIOS (binaire séparé)
├── loader/          # Dynamic linker Ring 3 (ld.so équivalent)
├── drivers/         # Drivers Ring 1 — framework/ storage/ network/ input/ display/ audio/
├── servers/         # Services Ring 1 — init/ shield/ net_stack/ crypto_server/ ...
├── libs/            # libexo/ libc_exo/ libexo_net/ libexo_ui/
├── userspace/       # shell/ coreutils/ net_tools/ compositor/ package_manager/
├── tools/           # ai_trainer/ exo-trace/ exo-debug/ exo-bench/ mkimage/ exo-ci/
├── tests/           # integration/ conformance/ security/
└── docs/            # DOC1-10 — documentation architecture complète
```

---

## Documentation

| Document | Contenu |
|----------|---------|
| `DOC1` | Corrections arborescence — signal, capability, modules IA |
| `DOC2` | Module `memory/` — allocateurs, DMA, IOMMU, protection |
| `DOC3` | Module `scheduler/` — CFS, RT, EDF, FPU, SMP |
| `DOC4` | Module `process/` — TCB, fork, exec, signal, namespaces |
| `DOC5` | Module `ipc/` — rings, SHM, RPC, zero-copy |
| `DOC6` | Module `fs/` — ext4plus, ext4 classique, FAT32, cache |
| `DOC7` | Module `security/` — capability, crypto, Zero Trust, audit |
| `DOC8` | Module `memory/dma/` — IOMMU, engines, completion |
| `DOC9` | Shield — anti-malware Ring 1, ML, sandbox, hooks |
| `DOC10` | Bootloader · Loader · Drivers · Userspace |
| `ARCHITECTURE_KERNEL_v6` | Référence globale v6 — règles transversales, boot sequence |

---

## Démarrage rapide

```bash
# Prérequis
rustup target add x86_64-unknown-none
cargo install cargo-make

# Build kernel
cargo build --package exo-kernel --target x86_64-unknown-none --release

# Build bootloader UEFI
cargo build --package exo-boot --target x86_64-unknown-uefi --release

# Générer image disque
cargo run --package mkimage -- \
    --kernel target/x86_64-unknown-none/release/exo-kernel \
    --bootloader target/x86_64-unknown-uefi/release/exo-boot.efi \
    --output exo-os.img

# Lancer dans QEMU
qemu-system-x86_64 \
    -bios /usr/share/ovmf/OVMF.fd \
    -drive file=exo-os.img,format=raw \
    -m 512M \
    -smp 4 \
    -enable-kvm \
    -serial stdio
```

---

## Principes de conception

**Tout ce qui peut être hors kernel doit l'être.** Le kernel Ring 0 contient uniquement ce qui est structurellement impossible de mettre ailleurs : gestion de la mémoire physique, ordonnancement, IPC primitif, filesystem de base, sécurité. Les drivers, la stack réseau, l'antivirus, le DNS — tout ça est Ring 1.

**Les dépendances sont strictement orientées.** Aucun module de couche basse ne connaît une couche haute. Les inversions nécessaires utilisent des traits abstraits enregistrés au boot. Cette règle est vérifiable mécaniquement par le compilateur Rust.

**La performance est une contrainte d'architecture.** Chaque module a des budgets en cycles documentés. Un hot path alloue zéro, dort zéro, prend zéro lock. Les exceptions sont explicites et justifiées.

**La sécurité est structurelle, pas optionnelle.** Les capabilities ne peuvent pas être bypassées — le CI le vérifie à chaque commit. Un driver ne peut pas accéder à du MMIO sans capability vérifiée par le kernel. Un binaire ne peut pas être exécuté depuis FAT32.

---

*Exo-OS — Système d'exploitation en Rust*
