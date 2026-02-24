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

---

## Qu'est-ce qu'Exo-OS ?

**Exo-OS** est un noyau de système d'exploitation **microkernel hybride** écrit en Rust, conçu autour de trois piliers non-négociables :

- 🔴 **Performance extrême** — context switch 500–800 cycles, IPC 500–700 cycles, allocateur thread-local 15–25 cycles
- 🔐 **Sécurité prouvée mathématiquement** — TCB de ~500 lignes soumis à preuves formelles Coq + TLA+, Zero Trust intégral, XChaCha20-Poly1305 sur tous les canaux inter-domaines
- 🏗️ **Architecture microkernel pure** — seul `fs/` reste en Ring 0, tous les drivers tournent en Ring 1 isolé

> **Décision fondamentale :** un crash de driver ne fait **jamais** planter le noyau. Pile réseau, GPU, USB et audio opèrent en espace utilisateur privilégié avec isolation IOMMU complète.

---

## Table des matières

- [Architecture](#architecture)
- [Modèle de sécurité](#modèle-de-sécurité)
- [Métriques cibles](#métriques-cibles)
- [Modules kernel](#modules-kernel)
- [Preuves formelles](#preuves-formelles)
- [Règles absolues](#règles-absolues)
- [Ordre de boot](#ordre-de-boot)
- [Structure du projet](#structure-du-projet)
- [Stack technique](#stack-technique)
- [Documentation](#documentation)

---

## Architecture

### Hiérarchie des couches

**Règle absolue : aucune dépendance remontante n'est tolérée.** Toute violation est un bug architectural, pas un avertissement.

```
┌─────────────────────────────────────────────────────────────┐
│  RING 3 · Userland                                          │
│  applications/   shell/   coreutils/   libs/                │
├─────────────────────────────────────────────────────────────┤
│  RING 1 · Serveurs système                                  │
│  shield/   network_server/   device_server/   init_server/  │
│  crypto_server/   ipc_router/   drivers/                    │
├─────────────────────────────────────────────────────────────┤
│  RING 0 · Kernel                                            │
│  security/capability/   ipc/   fs/ ★   process/             │
│  scheduler/   memory/                                       │
├─────────────────────────────────────────────────────────────┤
│  MATÉRIEL                                                   │
│  x86_64/   APIC/   ACPI/   SMP/   IOMMU/                   │
└─────────────────────────────────────────────────────────────┘

  ★ seul module autorisé en Ring 0 hors TCB
```

### Chaîne de dépendances (règle inviolable)

```
memory/ (0) → scheduler/ (1) → process/ (1.5) → ipc/ (2a) → fs/ (3)
```

| Depuis ╲ Vers | `memory/` | `scheduler/` | `process/` | `ipc/` | `fs/` |
|:---|:---:|:---:|:---:|:---:|:---:|
| **`memory/`**    | —  | ✗  | ✗  | ✗  | ✗ |
| **`scheduler/`** | ✅ | —  | ✗  | ✗  | ✗ |
| **`process/`**   | ✅ | ✅ | —  | ✗  | ✗ † |
| **`ipc/`**       | ✅ | ✅ | ✅ | —  | ✗ ‡ |
| **`fs/`**        | ✅ | ✅ | ✅ | ✅ | — |

> `†` `process/` accède à `fs/` uniquement via le **trait abstrait `ElfLoader`** enregistré au boot  
> `‡` `ipc/` accède à `fs/` uniquement via le **shim `fs/ipc_fs/shim.rs`**

### Résolution des cycles de dépendances

Les dépendances circulaires apparentes sont résolues par des **traits abstraits enregistrés au boot** :

```rust
// Problème : memory/dma/ doit réveiller des threads (process/)
//            mais memory/ ne peut pas importer process/
// Solution : trait abstrait — process/ s'enregistre au boot

pub trait DmaWakeupHandler: Send + Sync {
    fn wakeup_thread(&self, id: ThreadId, result: Result<(), DmaError>);
}

// process/ enregistre l'implémentation concrète au boot
// DMA reste sous memory/ → couche 0 respectée, zéro cycle
```

Le même pattern est appliqué pour `ElfLoader` (`process/` → `fs/`).

---

## Modèle de sécurité

### 3 couches indépendantes

```
┌─────────────────────────────────────────────────────────────┐
│  COUCHE 1 · TCB Kernel (Preuves formelles Coq / TLA+)       │
│  security/capability/ + security/crypto/                    │
│  ~500 lignes · 4 propriétés prouvées mathématiquement       │
├─────────────────────────────────────────────────────────────┤
│  COUCHE 2 · Zero Trust (Vérification systématique)          │
│  XChaCha20-Poly1305 · Capabilities gated · KPTI             │
│  Retpoline · SSBD per-thread · KASLR · CET Shadow Stack     │
├─────────────────────────────────────────────────────────────┤
│  COUCHE 3 · Shield (Ring 1 · type Bitdefender)              │
│  ML comportemental <100µs · 47 syscalls hookés              │
│  Sandbox auto · Firewall stateful · DNS guard anti-C2        │
└─────────────────────────────────────────────────────────────┘
```

### Système de capabilities

Source de vérité **unique** dans `security/capability/`. Révocation **O(1)** par incrémentation de génération.

```
CapToken · 128 bits · inforgeable
├── object_id  : u64   identifiant objet unique cross-module
├── rights     : u16   READ | WRITE | EXEC | GRANT | REVOKE | DELEGATE
├── generation : u32   incrémentée à la révocation → invalidation instantanée
└── tag        : u16   type d'objet (channel, file, shm, ...)
```

**Propriété garantie :** `revoke(oid)` rend immédiatement invalide **tout** token portant cet `object_id`, en O(1), sans parcours de tokens existants.

### XChaCha20-Poly1305

Chiffrement de **tous** les canaux inter-domaines :
- Messages IPC entre domaines de sécurité distincts
- Transferts DMA vers périphériques hors-TCB
- Communication kernel ↔ drivers Ring 1

### Shield — daemon anti-malware

> ⚠️ **Shield est lui-même sandboxé.** Un daemon de sécurité non-isolé est une escalade de privilèges garantie pour un attaquant.

```rust
fn self_isolate() {
    syscall_filter::apply_shield_filter();      // restriction syscalls
    capability::drop_all_except(&[             // least privilege strict
        Cap::SYS_PTRACE,
        Cap::NET_ADMIN,
        Cap::AUDIT_WRITE,
    ]);
    watchdog::register(Duration::from_secs(30)); // redémarrage si crash
}
```

---

## Métriques cibles

> Philosophie : **viser 2–3× Linux est ambitieux mais réaliste. Viser 10× est du marketing.**  
> Calibrage sur : L4 microkernel, Solaris, FreeBSD ULE, jemalloc.

| Métrique | Cible v1.0 | Linux référence | Gain |
|:---|:---:|:---:|:---:|
| Context switch | 500–800 cycles | ~2134 cycles | **3–4×** |
| Latence IPC | 500–700 cycles | ~1247 cycles | **2–2.5×** |
| Allocateur TLS | 15–25 cycles | ~50 cycles | **2–3×** |
| Scheduler `pick_next` | 100–150 cycles | ~200 cycles | **1.3–2×** |
| Syscall fast path | 80–100 cycles | ~150 cycles | **1.5–2×** |
| Boot total | <1 s | ~2 s | **2×** |
| SMP init | <300 ms | ~400 ms | — |
| IPI latence | <10 µs | ~20–50 µs | — |
| DMA submit | <500 ns | — | — |
| NVMe 4K E2E | <10 µs | ~15 µs | — |
| NVMe séquentiel | >12 GB/s | ~10 GB/s | — |

### Anti-objectifs (impossibles physiquement)

| Objectif irréaliste | Raison |
|:---|:---|
| Context switch <200 cycles | Incompatible avec isolation mémoire |
| Syscall <50 cycles | Minimum hardware ~60 cycles |
| Boot <100 ms | ACPI parsing incompressible ~50 ms |
| IPI <1 µs | Limite hardware APIC |

---

## Modules kernel

### `memory/` — Couche 0 absolue

> `memory/` n'importe **jamais** `scheduler/`, `ipc/`, `fs/` ou `process/`.

| Composant | Rôle |
|:---|:---|
| Buddy allocator | O(log n), zones DMA / DMA32 / NORMAL / MOVABLE |
| SLUB | Objets fixes, moins de fragmentation que slab classique |
| Per-CPU pools | 512 frames/CPU, lock-free, zéro contention fast path |
| **EmergencyPool** | 64 `WaitNode` statiques, initialisé **EN PREMIER** absolu |
| **FutexTable** | **Unique** dans tout l'OS, indexée par adresse **physique** |
| TLB shootdown | Synchrone avec barrière ACK — jamais de free avant ACK |
| DMA Engine | Sous `memory/dma/`, wakeup via trait abstrait |
| CoW | Fork duplique les VMAs, copie physique au premier write |

**Anti-deadlock fondamental :**

```
Scénario deadlock évité :
  Allocation → heap plein → reclaim → écriture swap → sleep
  → wait_queue veut allouer WaitNode depuis heap → DEADLOCK

Solution EmergencyPool :
  wait_queue n'appelle JAMAIS le heap
  → deadlock impossible même sous pression mémoire maximale
```

---

### `scheduler/` — Couche 1

> `pick_next_task()` en **100–150 cycles**. Le hot path ne fait jamais d'allocation ni de sleep.

**`signal/` est ABSENT de `scheduler/`** — déplacé dans `process/signal/`. Le scheduler lit uniquement un `AtomicBool signal_pending` dans le TCB.

| Composant | Détail |
|:---|:---|
| Run queues | 3 files simples RT / Normal / Idle, pop head = O(1) |
| `switch_asm.s` | ASM pur : `rbx`, `rbp`, `r12–r15`, `rsp`, `MXCSR`, `x87 FCW` |
| KPTI | CR3 switché **atomiquement** dans `switch_asm.s` avant restauration |
| Lazy FPU | CR0.TS=1 au switch, `#NM` → `XSAVE/XRSTOR` avec détection AVX |
| PreemptGuard | RAII obligatoire, jamais de `disable()`/`enable()` directs |
| WaitQueue | Utilise `EmergencyPool` exclusivement — jamais le heap |
| C-state governor | `fetch_min(latency)` à l'admission d'un thread RT |

> ⚠️ **`r15` obligatoire dans `switch_asm.s`** — utilisé par `ext4plus/inode/ops.rs`. Son omission provoque une corruption silencieuse lors de préemptions en zone FS.

```asm
context_switch_asm:
    push    %rbx
    push    %rbp
    push    %r12
    push    %r13
    push    %r14
    push    %r15          # ← obligatoire (ext4plus/inode/ops.rs)
    sub     $4,  %rsp
    stmxcsr (%rsp)        # MXCSR — registre contrôle SSE
    sub     $4,  %rsp
    fstcw   (%rsp)        # x87 FCW — contrôle virgule flottante
    mov     %rsp, (%rdi)  # sauvegarder rsp ancien thread
    cmp     %rdx, %cr3
    je      .skip_cr3
    mov     %rdx, %cr3    # KPTI — switcher les page tables
.skip_cr3:
    mov     %rsi, %rsp    # charger rsp nouveau thread
    # ... restauration symétrique ...
    ret
```

---

### `process/` — Couche 1.5

| Fonctionnalité | Implémentation |
|:---|:---|
| `fork()` CoW | <1 µs — duplication VMAs, protection read-only, refcount frames |
| `execve()` | Via trait abstrait `ElfLoader` enregistré par `fs/` au boot |
| `signal/` | Livraison **uniquement** au retour userspace — pas depuis le hot path |
| TCB | ≤128 bytes (2 cache lines) pour accès sans cache miss |
| Namespaces | PID / mount / net / UTS / user |
| Wakeup DMA | Impl `DmaWakeupHandler` enregistrée auprès de `memory/dma/` |

---

### `ipc/` — Couche 2a

| Composant | Règle critique |
|:---|:---|
| SPSC Ring | Head et tail sur **cache lines séparées** (`CachePadded`) — sans ça : false sharing → dégradation 10–100× |
| Fusion Ring | Batching adaptatif anti-thundering herd, seuil ajusté dynamiquement |
| SHM pages | `FrameFlags::NO_COW` **obligatoire** — un `fork()` ne copie pas les canaux IPC |
| `futex.rs` | Délégation pure à `memory/utils/futex_table` — zéro logique locale |
| `capability_bridge/` | Shim ~50 lignes vers `security/capability/` — zéro logique de droits locale |

---

### `fs/` — Couche 3 (seul en Ring 0)

| Composant | Détail |
|:---|:---|
| VFS + ext4+ | io_uring natif, zero-copy DMA, fsync optimisé |
| Intégrité | Blake3 checksums sur toutes les écritures, WAL avant métadonnées |
| Anti-deadlock | Lock inode relâché **avant** tout sleep (`release-before-sleep`) |
| EINTR | `IORING_OP_ASYNC_CANCEL` systématique si signal pendant wait |
| Accès IPC | Uniquement via `fs/ipc_fs/shim.rs` — jamais d'import direct |

---

### `security/` — TCB prouvé formellement

**Périmètre Coq/TLA+** : exactement 5 fichiers, ~500 lignes.  
Toute modification requiert une mise à jour des preuves associées.

```
security/capability/
├── model.rs        ← PÉRIMÈTRE PROUVÉ
├── token.rs        ← PÉRIMÈTRE PROUVÉ
├── rights.rs       ← PÉRIMÈTRE PROUVÉ
├── revocation.rs   ← PÉRIMÈTRE PROUVÉ
└── delegation.rs   ← PÉRIMÈTRE PROUVÉ
```

`ipc/capability_bridge/` est un shim **hors périmètre** qui délègue tout à `security/capability/`.

---

## Preuves formelles

**Outils :** Coq 8.17 (propriétés fonctionnelles) + TLC model checker (propriétés de concurrence)

**PROP-1 — Sûreté des capabilities**
```
∀ token : CapToken, table : CapTable,
  verify(table, token) = Ok →
    token.object_id ∈ table ∧
    token.generation = table[token.object_id].generation
```

**PROP-2 — Révocation instantanée O(1)**
```
∀ token : CapToken, oid : ObjectId,
  revoke(oid) ; verify(table', token) = Err(Revoked)
    quand token.object_id = oid

  Implémentation : génération++ uniquement — jamais de parcours de tokens
```

**PROP-3 — Confinement de la délégation**
```
∀ t_src t_child : CapToken,
  delegate(t_src) = t_child  →  t_child.rights ⊆ t_src.rights

  On ne peut jamais déléguer plus de droits qu'on en possède
```

**PROP-4 — Correction XChaCha20-Poly1305**
```
∀ p : Plaintext, k : Key, n : Nonce,
  decrypt(k, n, encrypt(k, n, p)) = Ok(p) ∧ integrity_verified
```

---

## Règles absolues

### Dépendances (violations = bug architectural)

```
✗  memory/    →  scheduler/  ipc/  fs/  process/
✗  scheduler/ →  ipc/  fs/
✗  process/   →  fs/            (sauf trait ElfLoader)
✗  ipc/       →  fs/            (sauf fs/ipc_fs/shim.rs)
```

### Hot path — zéro exception

```
✗  Allocation heap dans pick_next_task(), switch, IRQ handlers
✗  Sleep ou wait dans le hot path scheduler
✗  Appel vers une couche supérieure depuis le hot path
```

### Mémoire

```
✅  EmergencyPool initialisé EN PREMIER (étape 3 du boot)
✅  FutexTable indexée par adresse PHYSIQUE, unique dans memory/
✅  TLB shootdown synchrone + ACK avant tout free_pages()
✅  FrameFlags::DMA_PINNED maintenu jusqu'à wait_dma_complete()
✅  FrameFlags::NO_COW sur toutes les pages IPC / SHM
✗   Split huge page si DMA_PINNED
✗   Free frame DMA avant ACK de complétion
```

### Context switch

```
✅  Sauvegarder : rbx  rbp  r12  r13  r14  r15  rsp
✅  Sauvegarder : MXCSR  x87 FCW
✅  CR3 switché DANS switch_asm.s — avant restauration des registres
✅  Lazy FPU sauvegardée AVANT l'appel à switch_asm
✗   Oublier r15  →  corruption silencieuse inode/ext4
✗   Switcher CR3 après restauration  →  race condition KPTI
```

### Sécurité

```
✅  security/capability/ = unique source de vérité pour verify()
✅  XChaCha20-Poly1305 sur tout canal inter-domaines
✅  Retpoline sur tout appel indirect dans le hot path
✗   Dupliquer verify() dans un autre module
✗   Canal inter-domaines non chiffré
✗   Modifier security/capability/model.rs sans MAJ des preuves Coq
```

### Préemption (RAII obligatoire)

```rust
// ✅ CORRECT — préemption rétablie automatiquement au Drop
let _guard = PreemptGuard::new();

// ✗ INTERDIT — risque de déséquilibre si panic entre les deux
preempt_disable();
// ...
preempt_enable();
```

---

## Ordre de boot

```
 1   arch::boot::early_init()                      GDT minimal, paging identité
 2   arch::boot::parse_memory_map()                E820 / UEFI memory map
 3   memory::frame::emergency_pool::init()         ← EN PREMIER ABSOLU
 4   memory::allocator::bitmap::bootstrap()        Allocateur minimal bootstrap
 5   memory::allocator::buddy::init()              Buddy complet actif
 6   memory::heap::global::init()                  #[global_allocator] opérationnel
 7   memory::utils::futex_table::init()            Table futex unique
 8   arch::x86_64::gdt::init()
 9   arch::x86_64::idt::init()
10   arch::x86_64::tss::init_with_ist_stacks()     IST pour #DF / #NMI / #MCE
11   arch::x86_64::apic::init()
12   arch::x86_64::acpi::parser::init()            MADT → topologie SMP
13   scheduler::core::init()
14   scheduler::fpu::detect_xsave_size()           Détection AVX / AVX-512 / AMX
15   scheduler::timer::tick::init(HZ=1000)
16   scheduler::timer::hrtimer::init()
17   security::capability::init()                  TCB capabilities opérationnel
18   security::crypto::rng::init()                 CSPRNG (RDRAND + entropy pool)
19   process::core::registry::init()
20   process::state::wakeup::register_with_dma()   Enregistrer DmaWakeupHandler
21   memory::dma::iommu::init()
22   fs::core::vfs::init()
23   fs::ext4plus::mount_root()
24   ipc::core::init()
25   security::exploit_mitigations::kaslr::verify()
26   arch::x86_64::smp::start_aps()                Démarrer CPUs additionnels
27   memory::frame::pool::init_percpu()            Per-CPU pools (après SMP)
28   memory::utils::oom_killer::start_thread()
29   process::lifecycle::spawn_pid1()              init_server (PID 1)
30   # PID 1 démarre shield/  drivers/  network_server/  ...
```

> Chaque module vérifie en debug que ses dépendances sont initialisées. Une violation d'ordre provoque une `panic!` explicite, jamais un comportement indéfini.

---

## Structure du projet

```
exo-os/
├── kernel/                         Ring 0 — Microkernel TCB minimal
│   └── src/
│       ├── arch/                   x86_64 (boot, GDT, IDT, TSS, APIC, ACPI, SMP)
│       ├── memory/                 Couche 0 (buddy, slab, DMA, IOMMU, swap, CoW)
│       ├── scheduler/              Couche 1 (CFS, RT, EDF, switch_asm.s, FPU lazy)
│       ├── process/                Couche 1.5 (fork, exec, signal, namespaces)
│       ├── ipc/                    Couche 2a (SPSC, Fusion Ring, SHM zero-copy, RPC)
│       ├── fs/                     Couche 3 (VFS, ext4+, io_uring, WAL, Blake3)
│       ├── security/               TCB (capability, crypto, zero_trust, mitigations)
│       └── syscall/                Table + entry ASM + validation arguments
│
├── servers/                        Ring 1 — Services système isolés
│   ├── shield/                     Protection anti-malware
│   ├── network_server/             Pile TCP/IP userspace (io_uring async)
│   ├── device_server/              Gestion périphériques + hotplug
│   ├── crypto_server/              Service cryptographie centralisé
│   └── init_server/                PID 1 — gestionnaire de services
│
├── drivers/                        Ring 1/2 — Pilotes isolés (IOMMU)
│   ├── storage/nvme/
│   ├── network/
│   └── platform/pci/
│
├── userland/                       Ring 3
│   ├── libc/                       Exo-libc (conformité POSIX)
│   └── apps/
│
├── libs/                           Bibliothèques partagées (no_std)
│   ├── exo_crypto/                 XChaCha20, Blake3, Ed25519, X25519
│   ├── exo_collections/            rbtree, radixtree, structures lock-free
│   └── exo_sync/                   spinlock, mutex, rwlock, seqlock
│
├── proofs/                         Preuves formelles
│   ├── kernel_security/            Coq + TLA+ pour security/capability/
│   └── capability_system/          Modèle formel capabilities
│
├── tests/                          Tests unitaires, intégration, stress, conformité POSIX
├── bench/                          Benchmarks micro + système (lmbench-style)
└── tools/                          exo-debug  exo-trace  exo-prof  ai_trainer/
```

---

## Stack technique

| Composant | Choix | Justification |
|:---|:---|:---|
| Langage principal | Rust nightly `no_std` | Sûreté mémoire, zéro runtime overhead |
| ASM | AT&T syntax fichiers `.s` | Context switch, SYSCALL/SYSRET, SMP trampoline |
| Preuves formelles | Coq 8.17 + TLA+ / TLC | Fonctionnel (Coq) + concurrence (TLC model checker) |
| Crypto | XChaCha20-Poly1305 (RFC 8439) | AEAD, nonce 192-bit, résistant aux réutilisations |
| Checksums FS | Blake3 | Vitesse + sécurité pour intégrité données |
| I/O async | io_uring natif | Zero-copy, batch submissions, latence minimale |
| IA kernel | Lookup tables `.rodata` + EMA O(1) | **Zéro inférence dynamique en Ring 0** |
| Entraînement IA | `tools/ai_trainer/` offline | Tables compilées dans le binaire kernel |
| Benchmarks | lmbench-style | Comparaison reproductible avec Linux 6.x |

```toml
# rust-toolchain.toml
[toolchain]
channel    = "nightly"
components = ["rust-src", "llvm-tools-preview", "rustfmt", "clippy"]
targets    = ["x86_64-unknown-none", "aarch64-unknown-none"]
```

### Intelligence artificielle embarquée

L'IA dans le kernel est **strictement limitée** à des lookup tables compilées en `.rodata` et des heuristiques EMA O(1). Aucune inférence dynamique en Ring 0.

| Module | Type | Overhead max |
|:---|:---|:---:|
| `memory/physical/allocator/ai_hints.rs` | Table NUMA 2 KB | ≤5 cycles |
| `scheduler/policies/ai_guided.rs` | EMA classification thread | ≤10 cycles |
| `fs/cache/prefetch.rs` | Prefetch adaptatif readahead | ≤100 cycles |
| `tools/ai_trainer/` | Entraînement **offline** uniquement | — |

---

## Documentation

| Document | Contenu |
|:---|:---|
| [`DOC1_CORRECTIONS_ARBORESCENCE.md`](docs/DOC1_CORRECTIONS_ARBORESCENCE.md) | Corrections C1 (signal→process), C2 (capability→security), C3 (IA) |
| [`DOC2_MODULE_MEMORY.md`](docs/DOC2_MODULE_MEMORY.md) | Conception complète `memory/` avec code Rust annoté |
| [`DOC3_MODULE_SCHEDULER.md`](docs/DOC3_MODULE_SCHEDULER.md) | Conception complète `scheduler/` avec `switch_asm.s` |
| [`DOC4_TO_DOC9_MODULES.md`](docs/DOC4_TO_DOC9_MODULES.md) | Process, IPC, FS, Security, DMA, Shield + règles transversales |
| [`EXO_OS_ARBORESCENCE_COMPLETE.md`](docs/EXO_OS_ARBORESCENCE_COMPLETE.md) | Arborescence annotée complète v1.0 |
| [`proofs/kernel_security/`](proofs/kernel_security/) | Fichiers Coq `.v` et TLA+ `.tla` |
| [`METRICS.md`](docs/METRICS.md) | Métriques détaillées, plan d'optimisation, anti-objectifs |

---

## Références de conception

| Référence | Leçon intégrée |
|:---|:---|
| **L4 microkernel** | IPC ~600 cycles — architecture canaux + fast IPC path |
| **seL4** | Périmètre TCB minimal pour preuves formelles réalistes |
| **Solaris** | Context switch ~500 cycles avec register windows |
| **FreeBSD ULE** | Scheduler pick ~120 cycles, 3 files simples suffisent |
| **jemalloc** | ~20 cycles avec TLS magazine layer |
| **Linux io_uring** | Zero-copy I/O, batch submissions |

---

<div align="center">

```
RING 0 · TCB MINIMAL · ZERO TRUST · PREUVES FORMELLES
```

*Exo-OS — Architecture v1.0 — Rust + ASM + Coq / TLA+*

</div>
