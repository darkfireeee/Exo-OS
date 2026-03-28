# ExoOS — Guide d'Implémentation Master
## Index, Règles Globales & Conventions d'Écriture

> **Ce document est un guide d'implémentation, pas une spécification.**  
> La spécification est dans les fichiers `ExoOS_Architecture_v7.md`, `ExoOS_Kernel_Types_v10.md`, etc.  
> Ce guide explique **comment coder**, dans quel ordre, et quels pièges éviter.

---

## 0. Différence Fondamentale : Spec vs Guide d'Implémentation

| Document de Spec | Guide d'Implémentation |
|-----------------|----------------------|
| **QUOI** implémenter | **COMMENT** l'implémenter |
| Définit les invariants | Explique comment les atteindre |
| Mentionne les erreurs | Montre comment les éviter concrètement |
| Décrit le résultat final | Décrit le chemin de construction |
| Indépendant du langage | Spécifique Rust no_std x86_64 |

---

## 1. Structure des Guides

| Guide | Domaine | Fichier |
|-------|---------|---------|
| **GI-00** | Ce document — Master Index + Règles Globales | `GI-00_Master.md` |
| **GI-01** | Workspace, Cargo, Types Partagés, TCB, SSR | `GI-01_Types_TCB_SSR.md` |
| **GI-02** | Boot Séquence, Context Switch, FPU Lazy | `GI-02_Boot_ContextSwitch.md` |
| **GI-03** | Driver Framework : IRQ, DMA, PCI, IOMMU | `GI-03_Drivers_IRQ_DMA.md` |
| **GI-04** | ExoFS, POSIX Bridge, Syscalls | `GI-04_ExoFS_POSIX.md` |
| **GI-05** | ExoPhoenix : Gel, Restore, SSR Protocol | `GI-05_ExoPhoenix.md` |
| **GI-06** | Servers Ring 1, IPC, Capabilities | `GI-06_Servers_Ring1.md` |

**Ordre d'implémentation global :**
```
GI-01 → GI-02 → GI-03 → GI-04 → GI-05 → GI-06
```
Chaque guide liste ses prérequis spécifiques.

---

## 2. Environnement de Développement Obligatoire

### 2.1 Outillage

```toml
# rust-toolchain.toml — à la racine du workspace
[toolchain]
channel = "nightly-2025-01-15"    # Version nightly précise pour reproductibilité
components = ["rust-src", "rustfmt", "clippy", "llvm-tools-preview"]
targets = ["x86_64-unknown-none"]
```

```bash
# Installation
rustup toolchain install nightly-2025-01-15 --component rust-src rustfmt clippy llvm-tools-preview
rustup target add x86_64-unknown-none

# Vérification
rustc +nightly --version | grep nightly
cargo +nightly build --target x86_64-unknown-none -Z build-std=core,alloc
```

### 2.2 Target personnalisée ExoOS

```json
// x86_64-exoos-kernel.json
{
  "llvm-target":              "x86_64-unknown-none",
  "data-layout":              "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128",
  "arch":                     "x86_64",
  "os":                       "none",
  "vendor":                   "exoos",
  "linker-flavor":            "ld.lld",
  "linker":                   "rust-lld",
  "panic-strategy":           "abort",
  "disable-redzone":          true,
  "features":                 "-mmx,-sse,-sse2,+soft-float",
  "code-model":               "kernel",
  "relocation-model":         "static",
  "frame-pointer":            "always",
  "no-stack-check":           false
}
```

> **⚠️ `-mmx,-sse,-sse2,+soft-float`** : OBLIGATOIRE pour le kernel. Le kernel ne peut pas utiliser les registres SSE/AVX directement — ils appartiennent aux processus utilisateur (Lazy FPU). Sans ce flag, le compilateur peut émettre des instructions SSE dans les fonctions kernel, corrompant l'état FPU des processus Ring 3.

### 2.3 `.cargo/config.toml`

```toml
# ExoOS/.cargo/config.toml
[build]
target = "x86_64-exoos-kernel.json"

[unstable]
build-std = ["core", "compiler_builtins"]
build-std-features = ["compiler-builtins-mem"]

[profile.dev]
panic = "abort"
opt-level = 1      # Minimum pour éviter les stack frames énormes en debug

[profile.release]
panic = "abort"
opt-level = 3
lto = "thin"
codegen-units = 1
```

---

## 3. Règles de Style ExoOS (Obligatoires)

### 3.1 Structure de fichier Rust

```rust
// ═══ TEMPLATE FICHIER KERNEL ExoOS ═══════════════════════════════════════
//
// Fichier : kernel/src/chemin/vers/fichier.rs
// Rôle    : Description concise en une ligne.
//
// DÉPENDANCES :
//   - Quels modules sont requis avant d'initialiser ce module.
//
// INVARIANTS :
//   - INVARIANT-1 : Description de l'invariant maintenu par ce module.
//   - INVARIANT-2 : ...
//
// SÉCURITÉ ISR :
//   - Indique si les fonctions de ce module sont appelables en ISR context.
//   - Toute fonction ISR-safe doit être annotée // [ISR-SAFE]
//   - Toute fonction ISR-unsafe doit être annotée // [THREAD-ONLY]
//
// SOURCE DE VÉRITÉ :
//   ExoOS_Kernel_Types_v10.md §X ou ExoOS_Architecture_v7.md §Y
//
// ══════════════════════════════════════════════════════════════════════════

#![no_std]  // Dans les crates kernel uniquement
use core::sync::atomic::{AtomicU64, Ordering};
// ...
```

### 3.2 Règles de nommage

```rust
// ─── CONSTANTES ───────────────────────────────────────────────────────────
// SCREAMING_SNAKE_CASE — toujours avec leur unité dans le nom
pub const MAX_HANDLERS_PER_IRQ:    usize = 8;       // ✅ "per IRQ" dans le nom
pub const FREEZE_TIMEOUT_MS:       u64   = 100;     // ✅ "MS" dans le nom
pub const KERNEL_STACK_SIZE_BYTES: usize = 16_384;  // ✅ unité explicite
pub const MAX_CORES:               usize = 8;       // ❌ ambiguïté — ajouter contexte

// ─── FONCTIONS ISR ────────────────────────────────────────────────────────
// Les fonctions appelables depuis un ISR portent le suffixe _isr ou la note [ISR-SAFE]
pub fn dispatch_irq(irq: u8) { /* [ISR-SAFE] */ }
pub fn sys_irq_register(...) { /* [THREAD-ONLY] */ }

// ─── TYPES D'ADRESSE ──────────────────────────────────────────────────────
// Jamais mélanger PhysAddr, IoVirtAddr, VirtAddr sans cast explicite
// ❌ MAL :
let addr: u64 = phys_addr.0;
iommu_register(addr);  // Est-ce une adresse physique ou IOVA ?

// ✅ BIEN :
let iova: IoVirtAddr = iommu_alloc_iova(phys_addr);
iommu_register(iova);
```

### 3.3 Commentaires obligatoires

```rust
// ─── RÈGLE DE COMMENTAIRE ExoOS ──────────────────────────────────────────
//
// 1. TOUT unsafe block DOIT avoir un commentaire SAFETY :
unsafe {
    // SAFETY : Ce pointeur est valide car :
    //   a) Alloué par buddy_allocator::alloc() qui garantit l'alignement
    //   b) Aucun autre thread ne tient de référence (lock IRQ actif)
    //   c) La durée de vie dépasse la portée de cet unsafe block
    core::ptr::write(ptr, value);
}

// 2. TOUT AtomicXxx.store/load DOIT justifier l'Ordering choisi :
counter.store(0, Ordering::Release);
// Release : les écritures précédentes (slot.data) doivent être visibles
// pour les threads qui font load(Acquire) sur ce même counter.

// 3. TOUTE fn avec des effets de bord non évidents DOIT documenter :
/// Calibre le TSC et stocke la fréquence dans BOOT_TSC_KHZ.
///
/// # Préconditions
/// - DOIT être appelé avant enable_interrupts()
/// - DOIT être appelé une seule fois au boot (pas idempotent)
///
/// # Effets de bord
/// - Écrit dans BOOT_TSC_KHZ et BOOT_TSC_BASE (AtomicU64)
/// - Utilise le PIT timer (lecture/écriture I/O ports 0x40-0x43)
///
/// # Erreurs silencieuses si mal utilisé
/// - Si appelé après enable_interrupts() : calibration inexacte
///   (interruptions interfèrent avec la mesure PIT)
pub fn calibrate_tsc_khz() { ... }
```

### 3.4 Gestion des erreurs

```rust
// ─── RÈGLE D'ERREURS ExoOS ───────────────────────────────────────────────

// ❌ JAMAIS .unwrap() dans le kernel (sauf dans les tests)
let frame = buddy_allocator::alloc(1).unwrap(); // CRASH si OOM

// ✅ TOUJOURS propager ou gérer explicitement
let frame = buddy_allocator::alloc(1)
    .ok_or(KernelError::OutOfMemory)?;

// ❌ JAMAIS panic!() dans les chemins normaux du kernel
panic!("IRQ table full"); // Kernel mort

// ✅ Retourner une erreur ou utiliser le log + fallback
match irq_table.push(route) {
    Ok(_)  => Ok(reg_id),
    Err(_) => {
        log::error!("IRQ table full (irq={})", irq);
        Err(IrqError::HandlerLimitReached)
    }
}

// ❌ JAMAIS ignorer silencieusement les erreurs d'atomiques
let _ = cas.compare_exchange(...); // La valeur "ignorée" peut indiquer une race

// ✅ Logger ou compter les échecs
match cas.compare_exchange(old, new, Success, Failure) {
    Ok(_)  => { /* succès */ }
    Err(actual) => {
        self.dropped.fetch_add(1, Ordering::Relaxed);
        log::debug!("CAS race: expected {}, got {}", old, actual);
        return false;
    }
}
```

---

## 4. Règles absolues no_std Kernel

```rust
// ═══ RÈGLES no_std KERNEL ExoOS ══════════════════════════════════════════

// RÈGLE NS-01 : Imports autorisés dans le kernel
use core::sync::atomic::{AtomicU64, AtomicU32, AtomicBool, AtomicUsize, Ordering};
use core::cell::UnsafeCell;
use core::mem::{size_of, align_of, offset_of};
use core::ptr::{read_volatile, write_volatile, addr_of, addr_of_mut};
use core::hint::spin_loop;
use core::arch::asm;
use core::arch::x86_64::_rdtsc;
use heapless::{Vec, FnvIndexMap};  // Structures sans allocation
use spin::{Mutex, RwLock};         // Spinlocks no_std

// RÈGLE NS-02 : JAMAIS en ISR context
// ❌ spin::Mutex::lock()  → peut bloquer si lock contenu
// ❌ Vec::push()          → peut allouer
// ❌ log::info!()         → selon implémentation, peut allouer
// ✅ AtomicXxx            → toujours safe en ISR
// ✅ core::ptr::*         → toujours safe si pointeurs valides
// ✅ heapless::Vec::push() → no-alloc, retourne Err si plein
// ✅ log::error!()        → si implémenté avec ring buffer statique

// RÈGLE NS-03 : Déclarations globales statiques
// ❌ static mut GLOBAL: Vec<T> = Vec::new();  → non thread-safe
// ✅ static GLOBAL: Mutex<heapless::Vec<T, N>> = Mutex::new(heapless::Vec::new());
// ✅ static ATOMIC: AtomicU64 = AtomicU64::new(0);

// RÈGLE NS-04 : Initialisation différée
// Les statics const-constructibles sont initialisées inline.
// Les statics nécessitant une initialisation runtime utilisent
// le pattern init() appelé explicitement au boot.
static IOMMU_QUEUE: IommuFaultQueue = IommuFaultQueue::new(); // const-constructible
// puis : IOMMU_QUEUE.init(); // appelé au boot (pas dans new())

// RÈGLE NS-05 : Types #[repr(C)] pour tout ce qui traverse des frontières
// Tout type passé via IPC, stocké dans le SSR, ou partagé avec l'ASM
// DOIT avoir #[repr(C)] pour garantir la layout mémoire.
#[repr(C, align(64))]
pub struct ThreadControlBlock { ... }

// RÈGLE NS-06 : Vérifications compile-time pour les layouts critiques
const _: () = assert!(size_of::<ThreadControlBlock>() == 256);
const _: () = assert!(align_of::<ThreadControlBlock>() == 64);
const _: () = assert!(offset_of!(ThreadControlBlock, cr3_phys) == 56);
```

---

## 5. Matrice des Erreurs Silencieuses les Plus Dangereuses

> Ces erreurs **compilent et démarrent sans problème** mais produisent des comportements catastrophiques en production.

| # | Erreur silencieuse | Symptôme tardif | Comment l'éviter |
|---|-------------------|----------------|-----------------|
| S-01 | `static u64` (vs `AtomicU64`) pour une variable modifiée après boot | Valeur jamais mise à jour ou UB en SMP | Toujours `AtomicU64` pour les globales modifiables |
| S-02 | `Vec` en ISR context | Crash OOM aléatoire ou corruption heap | `heapless::Vec<T, N>` uniquement en ISR |
| S-03 | Missing `_irq_guard = irq_save()` avant un lock | Deadlock CPU si IRQ survient pendant lock | Toujours `irq_save()` avant write locks sur structures accédées en ISR |
| S-04 | `compare_exchange_weak` sur ARM/RISC-V | Spurious failures → drops arbitraires | Toujours `compare_exchange` (strong) en MPSC |
| S-05 | `Ordering::Relaxed` sur le success d'un CAS qui publie une valeur | Valeur non visible pour les autres CPUs | Success ordering = `Release` pour publier |
| S-06 | Accéder à GS.base comme user_gs_base **sans SWAPGS** | Accès aux données kernel depuis userspace | Comprendre le modèle SWAPGS (voir GI-02) |
| S-07 | `#[repr(packed)]` + référence à un champ non-aligné | UB silencieux E0793 (Rust 1.72+) | `[u8; 8]` + accesseurs comme dans EpollEventAbi |
| S-08 | Pas de `tss_set_rsp0()` dans context_switch | Prochaine IRQ empile sur la mauvaise pile | Toujours mettre à jour TSS.RSP0 après chaque switch |
| S-09 | Init IOMMU queue après activation IRQs IOMMU | Premières fautes IOMMU perdues, queue inutilisable | Séquence boot stricte : init() AVANT enable_interrupts() |
| S-10 | `BootInfo` passé par adresse physique à init_server | `#PF` immédiat à l'accès (paging actif en Ring 1) | Mapper la page BootInfo en VMA virtuelle (V7-C-01) |
| S-11 | `ZERO_BLOB_ID_4K` passé à `blob_refcount::increment()` | Corruption du système de déduplication ExoFS | Guard explicite : `if p_blob_id != ZERO_BLOB_ID_4K` |
| S-12 | switch_asm.s sans `CR0.TS=1` après switch | Thread suivant utilise FPU sans #NM → corruption FPU | Toujours `set_cr0_ts()` dans context_switch (V7-C-02) |
| S-13 | Pas de `fd.mark_stale()` pendant restore Phoenix | Threads bloqués sur fds invalides → deadlock permanent | Voir CORR-50 |
| S-14 | `do_exit()` sans purge des handlers IRQ orphelins | Limite MAX_HANDLERS_PER_IRQ épuisée après crash driver | `irq::revoke_all_irq(pid)` obligatoire dans do_exit() |
| S-15 | Nonce ChaCha20 non reseeded après restore Phoenix | Réutilisation de nonces → chiffrement cassé | CORR-12 : `phoenix_reseed()` avant tout crypto post-restore |

---

## 6. Séquence Globale d'Implémentation

```
Phase 0 — Infrastructure (GI-01)
  ├── Workspace Cargo.toml + target JSON
  ├── libs/exo-types (TCB, SSR, IpcMessage, types partagés)
  ├── libs/exo-ipc (SpscRing, ipc_send/receive)
  └── Assertions compile-time (size_of, offset_of)

Phase 1 — Kernel minimal (GI-02)
  ├── early_init.rs étapes 1-10 (boot, mémoire, APIC)
  ├── calibrate_tsc_khz() + BOOT_TSC_KHZ AtomicU64
  ├── IOMMU_FAULT_QUEUE.init()
  ├── enable_interrupts()
  ├── context_switch() + switch_asm.s + FPU lazy
  └── TSS.RSP0 mis à jour à chaque switch

Phase 2 — Drivers (GI-03)
  ├── IRQ routing + dispatch_irq (tableau fixe, pas Vec)
  ├── DMA/IOMMU : sys_dma_map + IommuDomainRegistry
  ├── PCI topology + do_exit() ordre strict
  └── IRQ watchdog

Phase 3 — ExoFS (GI-04)
  ├── posix_bridge/ : truncate, fallocate, msync, copy_range
  ├── io/reader.rs avec ZERO_BLOB_ID_4K ghost blob
  └── vfs_server Ring 1 (ops/, compat/)

Phase 4 — ExoPhoenix (GI-05)
  ├── SSR mapping + constantes canoniques
  ├── Séquence PrepareIsolation par server
  ├── handle_freeze_ipi() avec timeout
  └── phoenix_wake_sequence() avec reseed

Phase 5 — Servers Ring 1 (GI-06)
  ├── Ordre de démarrage (13 étapes)
  ├── CapToken + verify_cap_token() constant-time
  ├── panic handler Ring 1
  └── CI checks complets
```

---

## 7. Tests de Validation par Phase

```bash
# Phase 0 — Validation types
cargo build --workspace
# Doit compiler sans warning
cargo test --lib -- layout::  # Tests de layout compile-time

# Phase 1 — Validation boot minimal
qemu-system-x86_64 -kernel target/kernel.elf \
  -serial stdio -display none \
  -m 256M -smp 4
# ATTENDU : "Kernel booted, halt_cpu()"

# Phase 2 — Validation IRQ
# Injecter des IRQs QEMU et vérifier dispatch + ACK
qemu-system-x86_64 ... -device ioapic,gsi_base=0

# Phase 3 — Validation ExoFS
# Test des syscalls ExoFS 500-518 depuis un binaire userspace test
./tools/exofs-fuse --test-mode

# Phase 4 — Validation Phoenix
# Cycle complet gel/restore QEMU
qemu-system-x86_64 ... -snapshot
# Déclencher gel via IPI 0xF3 → vérifier restore

# Phase 5 — Validation Ring 1
# Démarrer tous les servers dans l'ordre canonique
# Vérifier PrepareIsolationAck complet
```

---

*ExoOS — Guide d'Implémentation Master (GI-00) — Mars 2026*
