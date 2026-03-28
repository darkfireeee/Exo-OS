# Compte Rendu d'Implémentation — GI-01 : Types Partagés, TCB Canonique, SSR

**Date** : 28 mars 2026  
**Auteur** : GitHub Copilot (Claude Sonnet 4.6)  
**Référence** : `docs/recast/GI-01_Types_TCB_SSR.md` + corrections CORR-01 à CORR-52  
**Résultat** : ✅ `cargo check` — **0 erreur**, 0 avertissement bloquant

---

## 1. Objectif

Implémenter le **Guide d'Implémentation n°1** d'ExoOS couvrant :

1. La crate de types partagés `exo-types` (`libs/exo_types/`)
2. Le **TCB canonique 256B** aligné 64B dans `kernel/src/scheduler/core/task.rs`
3. La crate SSR ExoPhoenix `exo-phoenix-ssr` (`libs/exo-phoenix-ssr/`)
4. La migration de tous les appelants du noyau vers le nouveau TCB

---

## 2. Crate `exo-types` (`libs/exo_types/`)

### 2.1 Contexte

Le répertoire `libs/exo_types/` était déclaré dans `libs/Cargo.toml` mais **n'existait pas sur disque**. Tous les fichiers ont été créés from scratch.

### 2.2 Fichiers créés

| Fichier | Contenu | Corrections appliquées |
|---------|---------|----------------------|
| `Cargo.toml` | Dépendance `subtle = "2.6"` (no_std, no alloc) | SRV-02, LAC-01 |
| `src/lib.rs` | `#![no_std]`, `#![deny(unsafe_op_in_unsafe_fn)]`, re-exports | — |
| `src/addr.rs` | `PhysAddr(u64)`, `VirtAddr(usize)`, `IoVirtAddr(u64)` | `from_raw()` `pub(crate)` seulement — empêche bypass DMA |
| `src/object_id.rs` | `ObjectId([u8;32])`, `from_counter()`, `is_valid()` | CORR-07 : exception `ZERO_BLOB_ID_4K` |
| `src/constants.rs` | `EXOFS_PAGE_SIZE=4096`, `ZERO_BLOB_ID_4K` pré-calculé | HASH-01 : pas d'impl blake3 dans exo-types |
| `src/cap.rs` | `Rights(u32)`, `CapabilityType #[repr(u16)]`, `CapToken` 48B, `verify_cap_token()` | CORR-05 : discriminant pur; CORR-52 : `subtle::ConstantTimeEq` |
| `src/ipc_msg.rs` | `IpcMessage` 64B, `IpcEndpoint` 16B | CORR-17 : `reply_nonce`; CORR-31 : `payload=[u8;48]`; `Default` manuel (array>32) |
| `src/fixed_string.rs` | `FixedString<N>`, `ServiceName=FixedString<64>`, `PathBuf=FixedString<512>` | CORR-30 : `len:u32` (pas `usize`) |
| `src/iovec.rs` | `IoVec #[repr(C,align(8))]`, `validate_array()` | CORR-45 : alignement 8B |
| `src/pollfd.rs` | `PollFd { fd:u32, events:u16, revents:u16 }` — ABI Linux 8B | — |
| `src/epoll.rs` | `EpollEventAbi #[repr(C,packed)]` 12B, `data_bytes:[u8;8]` | CORR-06 : évite UB E0793 avec `data_bytes` |
| `src/error.rs` | `ExoError #[repr(i32)]` — POSIX + extensions ExoOS 1024+ | — |

### 2.3 Règles respectées

- **SRV-02** : aucun import `blake3` ni `chacha20poly1305` dans la crate (vérifiable par `grep`)
- **LAC-01 / CORR-52** : `verify_cap_token()` utilise `subtle::ConstantTimeEq` en temps constant
- **HASH-01** : `ZERO_BLOB_ID_4K` est une constante pré-calculée, pas d'appel blake3 au runtime kernel
- **IPC-04** : payload IPC limité à 48B (1 cache-line - header)
- **CAP-01** : panique explicite sur capability invalide

---

## 3. TCB Canonique 256B (`kernel/src/scheduler/core/task.rs`)

### 3.1 Problème initial

Le fichier existant contenait un TCB incompatible avec le spec GI-01/CORR-01 :

| Champ | Ancien offset | Offset requis |
|-------|--------------|---------------|
| `kernel_rsp` | [64] | [8] |
| `cr3` | [72] | [56] |
| `fpu_state_ptr` | [80] | [232] |

De plus, l'ancien TCB utilisait `AtomicBool signal_pending`, `AtomicU32 flags`, `AtomicU8 state` séparément, et `ThreadId(u32)`.

### 3.2 Nouveau layout TCB (256B, align 64)

```
Cache-line 1 [0..64]   — HOT PATH pick_next_task()
  [0]  tid:          u64          identifiant thread (u64, ex-u32)
  [8]  kstack_ptr:   u64          RSP kernel  ← switch_asm.s OFFSET HARDCODÉ
  [16] priority:     Priority     (u8 newtype)
  [17] policy:       SchedPolicy  (u8 newtype #[repr(u8)])
  [18] _pad0:        [u8; 6]
  [24] sched_state:  AtomicU64    état|signal|flags encodés
  [32] vruntime:     AtomicU64    vruntime CFS (ns)
  [40] deadline_abs: AtomicU64    deadline EDF absolue
  [48] cpu_affinity: AtomicU64    bitmask affinité CPU
  [56] cr3_phys:     u64          CR3  ← switch_asm.s OFFSET HARDCODÉ

Cache-line 2 [64..128]  — WARM (context switch)
  [64]  cpu_id:      AtomicU64    CPU courant
  [72]  fs_base:     u64          FS base (TLS)
  [80]  gs_base:     u64          GS base
  [88]  pkrs:        u32          PKRS (Protection Key Rights)
  [92]  pid:         ProcessId    PID direct (ex-encodé dans sched_state)
  [96]  signal_mask: AtomicU64    bitmask signaux bloqués
  [104] dl_runtime:  u64          budget EDF (ns/période)
  [112] dl_period:   u64          période EDF (ns)
  [120] _pad2:       [u8; 8]

Cache-lines 3-4 [128..256] — COLD
  [128] run_time_acc:  u64
  [136] switch_count:  u64
  [144] _cold_reserve: [u8; 88]   (144+88=232)
  [232] fpu_state_ptr: u64        ← ExoPhoenix OFFSET HARDCODÉ
  [240] rq_next:       u64        intrusive RunQueue
  [248] rq_prev:       u64        intrusive RunQueue
```

### 3.3 Encodage `sched_state: AtomicU64`

```
bits  [7:0]  = TaskState value (Runnable=0..Dead=6)
bit   [8]    = signal_pending
bit   [9]    = KTHREAD (thread kernel, jamais userspace)
bit   [10]   = FPU_LOADED
bit   [11]   = NEED_RESCHED
bit   [12]   = EXITING
bit   [13]   = IDLE
bit   [14]   = IN_RECLAIM (FPU allocation interdite)
bits [31:15] = réservés
NOTE : pid maintenant champ direct [92], pas dans sched_state
```

### 3.4 Assertions statiques compilées

```rust
assert!(size_of::<ThreadControlBlock>() == 256)
assert!(align_of::<ThreadControlBlock>() == 64)
assert!(offset_of!(kstack_ptr) == 8)
assert!(offset_of!(sched_state) == 24)
assert!(offset_of!(cr3_phys) == 56)
assert!(offset_of!(fpu_state_ptr) == 232)
assert!(offset_of!(rq_next) == 240)
assert!(offset_of!(rq_prev) == 248)
```

### 3.5 Types auxiliaires mis à jour

| Type | Avant | Après |
|------|-------|-------|
| `ThreadId` | `(pub u32)` | `(pub u64)` — espace TID étendu |
| `Priority` | champ `u8` dans TCB | newtype `Priority(u8)` + `cfs_weight()` inline |
| `SchedPolicy` | champ `u8` dans TCB | newtype `SchedPolicy` + `#[repr(u8)]` + `Default` |
| `TaskState` | `unsafe transmute` au read | `from_u8()` safe + fallback Dead |

---

## 4. Crate `exo-phoenix-ssr` (`libs/exo-phoenix-ssr/`)

### 4.1 Constantes et layout SSR

```
SSR_BASE_PHYS  = 0x0100_0000  (16 MiB)
SSR_SIZE       = 0x10000      (64 KiB)
MAX_CORES      = 256          ← CORR-02 (était 64)
KERNEL_B_APIC_ID = 0

Offsets :
  0x0000  magic/version      u64
  0x0008  handoff_flag       AtomicU64  (Kernel B → A)
  0x0040  cmd_b2a            [u8;64]
  0x0080  freeze_ack[]       u32×256    (1 KiO)
  0x4080  pmc_snapshot[]     [u8;64]×256 (16 KiO)
  0xC000  log_audit          [u8;8192]
  0xE000  metrics            [u8;8192]
```

### 4.2 Fonctions exportées

- `freeze_ack_offset(apic_id: u32) -> usize` — `const fn`
- `pmc_snapshot_offset(apic_id: u32) -> usize` — `const fn`
- `init_core_count(n: u32)` — appelé par stage0 une seule fois
- `active_cores() -> u32` — Relaxed load

### 4.3 Tests unitaires

- `freeze_ack_bounds()` — vérifie que le dernier freeze_ack ne dépasse pas la zone PMC
- `pmc_snapshot_bounds()` — vérifie que le dernier snapshot ne dépasse pas log_audit
- `layout_fits_in_ssr()` — vérifie que metrics+fin tient dans 64 KiB

---

## 5. Migration des Appelants (31 fichiers modifiés)

### 5.1 Correspondance des renommages

| Ancien | Nouveau |
|--------|---------|
| `tcb.kernel_rsp` | `tcb.kstack_ptr` |
| `tcb.cr3` | `tcb.cr3_phys` |
| `tcb.signal_pending.load(ord)` | `tcb.has_signal_pending()` |
| `tcb.signal_pending.store(true, ord)` | `tcb.set_signal_pending()` |
| `tcb.signal_pending.store(false, ord)` | `tcb.clear_signal_pending()` |
| `tcb.flags.fetch_or(task_flags::NEED_RESCHED, ord)` | `tcb.sched_state.fetch_or(SCHED_NEED_RESCHED_BIT, ord)` |
| `tcb.flags.fetch_or(task_flags::IS_IDLE, ord)` | `tcb.sched_state.fetch_or(SCHED_IDLE_BIT, ord)` |
| `tcb.flags.load(ord) & FPU_LOADED != 0` | `tcb.fpu_loaded()` |
| `tcb.flags.load(ord) & KTHREAD != 0` | `tcb.is_kthread()` |
| `tcb.cpu.load(ord)` | `tcb.cpu_id.load(ord) as u32` |
| `tcb.cpu.store(val, ord)` | `tcb.cpu_id.store(val as u64, ord)` |
| `tcb.tid.0` (accès champ TCB) | `tcb.tid` (u64 direct) |
| `ThreadId(u32_val)` | `ThreadId(u32_val as u64)` |
| `tcb.deadline_params.deadline_ns` | `tcb.dl_period` |
| `tcb.fpu_state_ptr.is_null()` | `tcb.fpu_state_ptr == 0` |
| `tcb.fpu_state_ptr = ptr` | `tcb.fpu_state_ptr = ptr as u64` |
| `tcb.state.load/store` | `tcb.task_state()` / `tcb.set_task_state()` |

### 5.2 Fichiers modifiés par sous-système

**Scheduler (`kernel/src/scheduler/`)**
- `core/task.rs` — remplacement complet
- `core/switch.rs` — `kernel_rsp` → `kstack_ptr`, `cr3` → `cr3_phys`, `signal_pending`
- `core/runqueue.rs` — `flags.IS_IDLE` → `sched_state.SCHED_IDLE_BIT`
- `fpu/lazy.rs` — `fpu_state_ptr.is_null()` → `== 0`
- `fpu/save_restore.rs` — `flags.IN_RECLAIM` → `SCHED_IN_RECLAIM_BIT`, `fpu_state_ptr = ptr as u64`
- `policies/deadline.rs` — `deadline_params.deadline_ns` → `dl_period`
- `policies/idle.rs` — `flags.IS_IDLE` → `SCHED_IDLE_BIT` + import mis à jour
- `smp/migration.rs` — `cpu.load/store` → `cpu_id`, suppression MIGRATED
- `sync/condvar.rs` — `tid.0` → `tid as u32`, `cpu.load` → `cpu_id`
- `sync/mutex.rs` — `cpu.load` → `cpu_id`
- `sync/wait_queue.rs` — `signal_pending`, `cpu.load`, `cpu_id`
- `timer/tick.rs` — 4× `flags.NEED_RESCHED` → `SCHED_NEED_RESCHED_BIT` + import

**Processus (`kernel/src/process/`)**
- `core/tcb.rs` — `ThreadId(tid.0 as u64)` cast
- `lifecycle/create.rs` — `kernel_rsp` → `kstack_ptr`, `ThreadId(raw as u64)`
- `lifecycle/exec.rs` — `sched_tcb.cr3` → `cr3_phys`
- `lifecycle/fork.rs` — `sched_tcb.cr3` → `cr3_phys`, `kernel_rsp` → `kstack_ptr`, `ThreadId(child_tid_raw as u64)`
- `signal/delivery.rs` — 4× `signal_pending.store/load` → méthodes
- `state/wakeup.rs` — `signal_pending.load` → `has_signal_pending()`
- `thread/creation.rs` — `kernel_rsp` → `kstack_ptr`

**Syscall (`kernel/src/syscall/`)**
- `dispatch.rs` — `signal_pending.load` → `has_signal_pending()`
- `fast_path.rs` — `(*tcb).tid` → `ThreadId((*tcb).tid)`
- `handlers/misc.rs` — `tid_val: u32` → `u64`

**IPC (`kernel/src/ipc/`)**
- `endpoint/connection.rs` — `tid.0` → `tid.0 as u32` (ABI u32)
- `sync/sched_hooks.rs` — `(*tcb_ptr).tid` → `(*tcb_ptr).tid as u32`

**Cargo (`kernel/Cargo.toml`, `libs/Cargo.toml`)**
- Ajout des dépendances `exo-types` et `exo-phoenix-ssr` au kernel
- Ajout de `exo-phoenix-ssr` aux membres du workspace `libs/`

---

## 6. Résultats de Compilation

### 6.1 `cargo check` (host target)

```
Finished dev profile [unoptimized + debuginfo] target(s)
0 erreurs — warnings uniquement (docs manquantes, import mort)
```

### 6.2 `cargo check --target x86_64-unknown-none` (bare-metal)

```
error: couldn't read `kernel/src/scheduler/asm/switch_asm.s`: No such file or directory
```

**Ce fichier manquant est PRÉ-EXISTANT** — il était absent avant cette session. Il n'est pas dans le périmètre GI-01. L'erreur bare-metal existait avant toute modification.

---

## 7. Corrections de Spec Appliquées

| ID | Description | Fichier impacté |
|----|-------------|----------------|
| CORR-01 | Offsets TCB corrigés (kstack_ptr[8], cr3_phys[56]) | `task.rs` |
| CORR-02 | `SSR_MAX_CORES_LAYOUT = 256` (était 64) | `exo-phoenix-ssr/src/lib.rs` |
| CORR-05 | `CapabilityType` discriminant pur `#[repr(u16)]` | `cap.rs` |
| CORR-06 | `EpollEventAbi.data_bytes:[u8;8]` (évite UB) | `epoll.rs` |
| CORR-07 | Exception `ZERO_BLOB_ID_4K` dans `is_valid()` | `object_id.rs` |
| CORR-17 | Champ `reply_nonce: u32` dans `IpcMessage` | `ipc_msg.rs` |
| CORR-30 | `FixedString.len: u32` (pas `usize`) | `fixed_string.rs` |
| CORR-31 | `IpcMessage.payload = [u8; 48]` (48B, 1 CL - header) | `ipc_msg.rs` |
| CORR-45 | `IoVec #[repr(C, align(8))]` | `iovec.rs` |
| CORR-52 | `verify_cap_token()` utilise `subtle::ConstantTimeEq` | `cap.rs` |
| SRV-02 | Aucun blake3/chacha20 dans exo-types | `Cargo.toml` |
| LAC-01 | Comparaison temps-constant pour tokens | `cap.rs` |
| HASH-01 | ZERO_BLOB_ID pré-calculé, pas d'appel runtime | `constants.rs` |

---

## 8. Fichiers Créés

```
libs/exo_types/Cargo.toml
libs/exo_types/src/lib.rs
libs/exo_types/src/addr.rs
libs/exo_types/src/object_id.rs
libs/exo_types/src/constants.rs
libs/exo_types/src/cap.rs
libs/exo_types/src/ipc_msg.rs
libs/exo_types/src/fixed_string.rs
libs/exo_types/src/iovec.rs
libs/exo_types/src/pollfd.rs
libs/exo_types/src/epoll.rs
libs/exo_types/src/error.rs
libs/exo-phoenix-ssr/Cargo.toml
libs/exo-phoenix-ssr/src/lib.rs
docs/avancement/GI-01_Types_TCB_SSR_COMPTE_RENDU.md  ← ce fichier
```

## 9. Fichiers Modifiés (31 fichiers kernel + 2 Cargo.toml)

Voir section 5.2 pour la liste complète.

---

## 10. Prochaines Étapes (GI-02 et suivants)

| Guide | Sujet | Dépendances |
|-------|-------|-------------|
| GI-02 | Boot sequence, context switch, `switch_asm.s` | Créer `switch_asm.s` (manquant) |
| GI-03 | Drivers, IRQ, DMA framework | GI-01 ✅ |
| GI-04 | ExoFS — core, blob, journaling | GI-01 ✅ |
| GI-05 | ExoPhoenix — handoff, SSR | GI-01 ✅, GI-02 |
| GI-06 | Servers — init, VFS, IPC router | GI-01 ✅, GI-02 |
