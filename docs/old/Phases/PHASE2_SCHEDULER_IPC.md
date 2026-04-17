# Phase 2 — Scheduler + IPC de base

**Prérequis exo-boot · Modules : `scheduler/`, `ipc/core/`, `ipc/ring/`, `ipc/sync/futex.rs`**

> Dépend de Phase 1 (heap kernel opérationnelle, physmap, APIC remappé).  
> Condition de sortie : context switch fonctionnel, SPSC ring opérationnel, Futex actif.

---

## 1. Vue d'ensemble

Phase 2 couvre deux sous-systèmes interdépendants :

| Sous-système | Rôle | Chemin |
|---|---|---|
| Scheduler CFS | Ordonnancement déterministe des threads, context switch, FPU lazy | `kernel/src/scheduler/` |
| IPC ring | Communication inter-processus sans copie (SPSC/MPMC) | `kernel/src/ipc/ring/` |
| Futex | Synchronisation user/kernel (wait/wake par adresse) | `kernel/src/ipc/sync/futex.rs`, `memory/utils/futex_table.rs` |

---

## 2. Scheduler

### 2.1 Thread Control Block (TCB)

**Fichier** : `kernel/src/scheduler/core/task.rs`

```
TCB = 128 bytes exactement (2 cache lines × 64 bytes) — aligné 64 bytes
```

**Layout cache line 1 [bytes 0..64] — hot path `pick_next_task()`** :

```
tid(4) + pid(4) + cpu(4) + _align(4) + affinity(8) + policy(1) + prio(1) +
state(1) + _pad_state(1) + flags(4) + vruntime(8) + deadline_abs(8) +
signal_pending(1) + dma_completion_result(1) + _pad1(14) = 64 bytes
```

**Layout cache line 2 [bytes 64..128] — context switch / cold** :

```
kernel_rsp(8) + cr3(8) + fpu_state_ptr(8) + signal_mask(8) +
_pad2(8) + deadline_params(24) = 64 bytes
```

> **Note** : les 8 bytes anciennement occupés par `ThreadAiState` (modules IA supprimés en
> [commit 9e7fc65](../../refonte/ExoOS_Roadmap_Avant_ExoBoot.md)) sont désormais `_pad2` —
> disponibles pour une utilisation future (profil de thread, TLS ptr, etc.).

**Politiques d'ordonnancement supportées** :

| Politique | Valeur | Description |
|---|---|---|
| `Normal` | 0 | CFS — défaut pour threads utilisateur |
| `Batch` | 1 | CFS — calcul de fond, vruntime non boosté au wake |
| `Idle` | 2 | Ne tourne que si rien d'autre n'est prêt |
| `Fifo` | 3 | RT — FIFO, pas de timeslice |
| `RoundRobin` | 4 | RT — RR avec timeslice fixe |
| `Deadline` | 5 | EDF — runtime/deadline/period en ns |

**Invariants vérifiés statiquement** :
```rust
assert!(size_of::<ThreadControlBlock>() == 128);
assert!(align_of::<ThreadControlBlock>() == 64);
```

### 2.2 Séquence d'initialisation

**Fichier** : `kernel/src/scheduler/mod.rs`

```
scheduler::init(params) fait EXACTEMENT ces 11 étapes dans cet ordre :

  Étape 1  — preempt::init()            compteurs de préemption
  Étape 2  — runqueue::init_percpu()    run queues par CPU (max 64 CPUs)
  Étape 3  — fpu::save_restore::init()  détection XSAVE/XRSTOR vs FXSAVE
  Étape 4  — fpu::lazy::init()          CR0.TS=1 sur le BSP (FPU lazy)
  Étape 5  — timer::clock::init()       horloge TSC (fréquence fournie par caller)
  Étape 6  — timer::tick::init()        tick handler (HZ = 1000)
  Étape 7  — timer::hrtimer::init()     high-resolution timers
  Étape 8  — timer::deadline_timer::init()  EDF deadline timers
  Étape 9  — sync::wait_queue::init()   wait queues (vérifie EmergencyPool)
  Étape 10 — energy::c_states::init()   C-states ACPI
  Étape 11 — smp::topology_init()       topologie SMP
```

**Paramètres** (`SchedInitParams`) :

| Paramètre | Défaut | Description |
|---|---|---|
| `nr_cpus` | 1 | Nombre de CPUs logiques (clampé à `MAX_CPUS = 64`) |
| `nr_nodes` | 1 | Nombre de nœuds NUMA |
| `tsc_hz` | 3 000 000 000 | Fréquence TSC en Hz — 0 = fallback 3 GHz |

> **⚠️ ERR-01** : La fréquence TSC est initialisée avec la valeur passée par l'appelant
> (défaut : 3 GHz). Sur QEMU, cette valeur est correcte. Sur hardware réel, la calibration
> via HPET ou PM Timer ACPI est nécessaire (non implémentée — Phase 2 en cours).

### 2.3 Context switch

**Fichier** : `kernel/src/scheduler/core/switch.rs`

**Règles** :
- `SWITCH-01` : `check_signal_pending()` **lit** uniquement — jamais de livraison de signal dans le scheduler.
- `SWITCH-02` : FPU lazy sauvegardée **avant** le switch, marquée **après**.
- `SWITCH-ASM` : `switch_asm.s` sauvegarde `rbx, rbp, r12–r15, rsp` + `MXCSR` + `x87 FCW`. CR3 switché **avant** la restauration des registres (KPTI correct).

**Thread courant par CPU** :

```rust
pub static CURRENT_THREAD_PER_CPU: [AtomicUsize; MAX_CPUS] = ...;
```

> **⚠️ TODO SMP** : `current_thread_raw()` utilise toujours l'entrée CPU 0 en mode mono-CPU.
> Sur SMP réel, il faudra lire `rdmsr(IA32_GS_BASE)` pour obtenir l'ID CPU.
> `SWAPGS` est requis à chaque entrée/sortie Ring 0 (non implémenté — à faire avant SMP).

### 2.4 Sélection du prochain thread (`pick_next_task`)

**Fichier** : `kernel/src/scheduler/core/pick_next.rs`

**Algorithme** (O(1) en hot path, cible 100–150 cycles) :

```
1. Si thread courant est RT et reste le plus prioritaire → KeepRunning
2. Si file RT non vide → Switch vers le thread RT le plus prioritaire
3. Sinon → CFS : pick_next() (vruntime minimal)
4. Vérification état (Runnable/Running obligatoire)
5. Si aucun thread → GoIdle
```

> **Garantie de déterminisme** : les modules IA (`ai_guided.rs`) ont été supprimés.
> CFS est purement déterministe — aucune heuristique n'influence le choix.

**Compteurs d'instrumentation** :

| Compteur | Description |
|---|---|
| `PICK_NEXT_TOTAL` | Total appels depuis le boot |
| `PICK_SAME_CURRENT` | Thread courant reconduit sans switch |
| `PICK_RT_RT` | Switches RT → RT |
| `PICK_SKIP_INELIGIBLE` | Threads ignorés (zombie, stopped) |

### 2.5 FPU — XSAVE/XRSTOR

**Fichiers** : `kernel/src/scheduler/fpu/`

| Module | Rôle |
|---|---|
| `save_restore.rs` | Appels `xsave64`/`xrstor64` et `fxsave64`/`fxrstor64` |
| `state.rs` | `FpuState` (512B aligné 64B), détection de `XSAVE_AREA_SIZE` |
| `lazy.rs` | Stratégie lazy : `CR0.TS=1` au boot, exception `#NM` au premier accès FPU |

**Détection au boot** :
```
arch_has_xsave() → CPUID[0x0D] → HAS_XSAVE (AtomicBool)
Si XSAVE disponible  → xsave64/xrstor64 (couvre AVX, AVX-512, MPX…)
Sinon                → fxsave64/fxrstor64 (x87 + SSE uniquement)
```

---

## 3. IPC Ring

### 3.1 SPSC Ring

**Fichier** : `kernel/src/ipc/ring/spsc.rs`

**Structure** (anti-false-sharing, conforme `IPC-01`) :

```rust
#[repr(C, align(64))]
struct CachePad(AtomicU64, [u8; 56]);  // 64 bytes exactement

pub struct SpscRing {
    capacity: usize,
    head: CachePad,   // ← cache line PRODUCTEUR séparée
    tail: CachePad,   // ← cache line CONSOMMATEUR séparée
    ...
}
```

> **IPC-01 résolu** : `head` et `tail` sont sur des cache lines distinctes.
> Aucun false sharing possible — performant sur hardware multicore.

**Opérations** :
- `send(msg)` → enfile si espace disponible
- `recv()` → défile si message disponible
- Sémantique lock-free : seul `Ordering::Release`/`Acquire` sur les pointeurs

**Initialisation** :
```rust
ipc::ring::spsc::init_spsc_rings();  // appelé dans kernel_init() Phase 7
```

### 3.2 MPMC Ring

**Fichier** : `kernel/src/ipc/ring/mpmc.rs`

Multi-producteurs / multi-consommateurs, séquences atomiques per-slot.

### 3.3 Zero-copy

**Fichier** : `kernel/src/ipc/ring/zerocopy.rs`

Partage de pages physiques entre espaces d'adressage via capabilities.

---

## 4. Futex

### 4.1 Architecture

**Fichiers** :
- `kernel/src/ipc/sync/futex.rs` — shim de délégation, types `FutexKey`
- `kernel/src/memory/utils/futex_table.rs` — implémentation unique (Couche 0)

> **Règle IPC-02** : `ipc/sync/futex.rs` ne contient **aucune logique locale**.
> La table futex unique réside en `memory/` et est partagée par tous les sous-systèmes.

### 4.2 Table futex

**Paramètres** :

| Paramètre | Valeur | Règle |
|---|---|---|
| Buckets | 4096 | `MEM-FUTEX V-34` : ≥ 4096 obligatoire |
| Hash | SipHash-1-3 keyed | Anti-DoS par collision |
| Fallback hash | FNV-1a | Avant `init_futex_seed()` seulement |
| Lock par bucket | `spin::Mutex<BucketInner>` | — |

**Anti-DoS (IPC-02)** :

```rust
// Graine SipHash initialisée depuis security::crypto::rng — kernel_init() Phase 5
let mut seed = [0u8; 16];
if crate::security::crypto::rng_fill(&mut seed).is_ok() {
    crate::memory::utils::futex_table::init_futex_seed(seed);
}
```

Avant `init_futex_seed()` : hash FNV-1a (sûr au boot, non keyed).  
Après : SipHash-1-3 keyed — résistant aux attaques HashDoS depuis userspace.

> **ERR-05 corrigé** : la graine n'est plus `[0; 16]`. Elle est initialisée dans
> `kernel_init()` depuis `rng_fill()` (RDRAND + TSC + ChaCha20).

**Opérations** :

| Opération | Description |
|---|---|
| `futex_wait(addr, expected, tid, wake_fn)` | Si `*addr == expected` → enfile waiter, retourne `Waiting`. Sinon `ValueMismatch`. |
| `futex_wake(addr, max_wakers)` | Réveille jusqu'à `max_wakers` threads sur `addr` |
| `futex_wake_n` | Alias avec paramètre `n` |
| `futex_requeue(src, dst, max_wake, max_requeue)` | Réfile des waiters de `src` vers `dst` |
| `futex_cancel(waiter)` | Annule un waiter (timeout ou signal) |

**Injection de la fonction de réveil** :

La table futex est en Couche 0 et n'a **pas** de dépendance sur `scheduler/`.  
Le réveil est fait via un `fn pointer` (`WakeFn`) injecté par le scheduler — couplage zéro.

### 4.3 Waiter

```rust
#[repr(C)]
pub struct FutexWaiter {
    pub virt_addr:    u64,       // adresse sur laquelle on attend
    pub expected_val: u32,       // valeur attendue (vérifiée à l'enfilement)
    pub tid:          u64,       // thread ID
    pub wake_fn:      WakeFn,    // fonction de réveil injectable
    pub wake_code:    i32,       // code de retour transmis au thread
    pub woken:        AtomicBool, // signalé quand réveillé/annulé
    pub next:         Option<NonNull<FutexWaiter>>, // liste intrusive
}
```

---

## 5. État d'implémentation

### 5.1 Checklist Phase 2 (tirée du roadmap)

| # | Item | État | Détail |
|---|------|------|--------|
| ✅ | Context switch x86_64 avec XSAVE/XRSTOR | **Implémenté** | `fpu/save_restore.rs` + `switch_asm.s` |
| ✅ | RunQueue intrusive — zéro alloc dans ISR | **Implémenté** | `core/runqueue.rs`, EmergencyPool utilisé pour WaitNode |
| ✅ | SPSC ring avec `CachePadded` | **Implémenté** | `ipc/ring/spsc.rs` — `CachePad(AtomicU64, [u8;56])` |
| ✅ | Futex table avec clé SipHash depuis CSPRNG | **Implémenté** | `ERR-05` corrigé — `init_futex_seed()` dans `kernel_init()` |
| ✅ | CFS déterministe (sans heuristiques IA) | **Implémenté** | Modules IA supprimés — commit 9e7fc65 |
| ✅ | Timer hrtimer basé sur HPET calibré | **Implémenté** | Chaîne HPET→PM→CPUID→PIT→3GHz dans `arch/x86_64/time/calibration/mod.rs` (FIX TIME-02 / ERR-01 corrigé) |
| ⚠️ | SPSC testé sur QEMU `-smp 4` | **Non vérifié** | Tests unitaires multicore à ajouter (non bloquant mono-CPU) |
| ✅ | SWAPGS correct à chaque entrée/sortie Ring 0 | **Implémenté** | `arch/x86_64/syscall.rs` — `swapgs` à l'entrée (ligne 176) et sortie (ligne 239) |

### 5.2 Erreurs silencieuses — état de résolution

| ID | Description | État |
|---|---|---|
| `ERR-01` | TSC calibré à 3 GHz fallback sur hardware réel | ✅ Corrigé — chaîne complète HPET→PM→CPUID→PIT→3GHz dans `arch/x86_64/time/calibration/` |
| `ERR-04` | SPSC sans CachePadded → false sharing multicore | ✅ Corrigé — `CachePad` aligné 64 bytes |
| `ERR-05` | Futex SipHash avec clé nulle (HashDoS) | ✅ Corrigé — graine depuis `rng_fill()` au boot |

---

## 6. Dépendances inter-phases

```
Phase 1 (heap, physmap)
    ↓
Phase 2 (scheduler, IPC)
    ├── scheduler::init() appelé dans kernel_init() Phase 3
    ├── ipc::ring::spsc::init_spsc_rings() appelé dans kernel_init() Phase 6
    └── init_futex_seed() appelé dans kernel_init() Phase 5
    ↓
Phase 3 (process, signal) — scheduler requis pour fork/exec
Phase 4 (ExoFS) — scheduler requis pour GC thread / writeback thread
```

---

## 7. TODOs bloquants avant Phase 6 (exo-boot)

| Priorité | Action | Module | Règle |
|---|---|---|---|
| ✅ Résolu | SWAPGS à l'entrée/sortie Ring 0 | `arch/x86_64/syscall.rs` | Implémenté |
| ✅ Résolu | Calibration TSC via HPET ou PM Timer ACPI | `arch/x86_64/time/calibration/mod.rs` | `ERR-01` corrigé |
| 🔵 SMP futur | Lire `rdmsr(IA32_GS_BASE)` pour `current_thread_raw()` | `scheduler/core/switch.rs` | `TODO(SMP)` — non bloquant mono-CPU |
| 🔵 Test futur | SPSC ring testé sur QEMU `-smp 4` minimum | `ipc/ring/spsc.rs` | `ERR-04` vérification multicore |
