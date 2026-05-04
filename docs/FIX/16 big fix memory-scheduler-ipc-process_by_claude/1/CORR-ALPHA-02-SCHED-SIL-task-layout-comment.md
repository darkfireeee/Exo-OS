# CORR-ALPHA-02 — Scheduler : Commentaire layout TCB erroné — `_pad1` fantôme à [92]

> **Auteur :** claude-alpha  
> **Date :** 2026-05-04  
> **Classe :** 🟠 SIL — Silent Wrong Documentation  
> **Fichier :** `kernel/src/scheduler/core/task.rs`  
> **Struct :** `ThreadControlBlock`  
> **Sévérité :** Majeure — toute dérivation d'outil ou d'audit basé sur ce commentaire produira un layout incorrect

---

## 1. Description du bug

Le commentaire de layout dans `task.rs` décrit l'offset [92] comme `_pad1: [u8; 4]` :

```rust
// Cache-line 2 [64..128]  — warm (context switch)
//   [64]  cpu_id:      AtomicU64  CPU courant
//   [72]  fs_base:     u64        FS base (TLS)
//   [80]  user_gs_base:u64        GS base userspace
//   [88]  pkrs:        u32        PKRS
//   [92]  _pad1:       [u8; 4]   ← BUG : ce champ n'existe PAS dans la struct
//   [96]  signal_mask: AtomicU64  bitmask signaux bloqués
```

Mais la définition réelle du struct est :

```rust
pub struct ThreadControlBlock {
    // ...
    pub pkrs: u32,              // [88]
    pub pid: ProcessId,         // [92]  ← champ réel, ABSENT du commentaire
    pub signal_mask: AtomicU64, // [96]
```

### Analyse du gap

- Le struct **n'a pas** de `_pad1: [u8; 4]` entre `pkrs` et `signal_mask`
- À l'offset [92] se trouve `pid: ProcessId` (4 bytes = u32)
- Le commentaire **omet entièrement** le champ `pid` dans la cache-line 2
- `pid` est pourtant un champ de compatibilité important (compat PCB, hot path IPC/syslog)

### Origines probables

L'erreur date probablement d'une refactorisation où `_pad1: [u8; 4]` a été remplacé par `pid: ProcessId` dans le struct mais le commentaire de layout n'a pas été mis à jour.

### Vérification formelle

L'assertion compile-time dans le code confirme l'offset réel :
```rust
// (implicite : offset_of!(pid) = 88 + 4 = 92, confirmé par size_of::<ProcessId>() = 4)
// Aucune assertion explicite pour pid — à ajouter (voir point 4)
```

La documentation Architecture v7 §3.2 est correcte :
```
| `pkrs` | [88] | 4 B | Intel PKS 32b |
| `pid`  | [92] | 4 B | ProcessId (compat PCB) |
```

---

## 2. Correctif

### Fichier : `kernel/src/scheduler/core/task.rs`

**Avant (commentaire cache-line 2) :**
```rust
// Cache-line 2 [64..128]  — warm (context switch)
//   [64]  cpu_id:      AtomicU64  CPU courant
//   [72]  fs_base:     u64        FS base (TLS)
//   [80]  user_gs_base:u64        GS base userspace
//   [88]  pkrs:        u32        PKRS
//   [92]  _pad1:       [u8; 4]
//   [96]  signal_mask: AtomicU64  bitmask signaux bloqués
//   [104] dl_runtime:  u64        budget EDF (ns/période)
//   [112] dl_period:   u64        période EDF (ns)
//   [120] _pad2:       [u8; 8]
```

**Après :**
```rust
// Cache-line 2 [64..128]  — warm (context switch)
//   [64]  cpu_id:      AtomicU64  CPU courant
//   [72]  fs_base:     u64        FS base (TLS) — MSR 0xC0000100
//   [80]  user_gs_base:u64        GS userspace  — MSR 0xC0000102
//   [88]  pkrs:        u32        Intel PKS domain register
//   [92]  pid:         ProcessId  PID processus (compat PCB, hot path IPC/debug)
//   [96]  signal_mask: AtomicU64  bitmask signaux bloqués (POSIX sigprocmask)
//   [104] dl_runtime:  u64        budget EDF (ns/période)
//   [112] dl_period:   u64        période EDF (ns)
//   [120] _pad2:       [u8; 8]
```

### Assertion compile-time à ajouter

```rust
// Après les assertions existantes dans task.rs :
const _: () = assert!(
    offset_of!(ThreadControlBlock, pid) == 92,
    "TCB: pid doit être à l'offset 92 (ProcessId après pkrs)"
);
const _: () = assert!(
    offset_of!(ThreadControlBlock, signal_mask) == 96,
    "TCB: signal_mask doit être à l'offset 96"
);
```

---

## 3. Impact scope

- **Fichier modifié :** `kernel/src/scheduler/core/task.rs`
- **Nature :** correction de commentaire + ajout d'assertions compile-time
- **Aucun changement de comportement runtime**
- **Outils d'audit externe** (ExoPhoenix Kernel B lisant le TCB par offset) : la correction de la doc évite toute confusion sur la structure du TCB
- **Tests :** les nouvelles assertions compile-time détecteront toute future régression de layout

---

## 4. Note de cohérence Architecture

Le champ `pid: ProcessId` à [92] est référencé dans :
- `Architecture_v7.md §3.2` : ✅ correct
- `GI-01_Types_TCB_SSR.md §7` : à vérifier / mettre à jour si nécessaire
- `ExoPhoenix_Spec_v7.md` : ExoPhoenix Kernel B accède au PCB, pas au TCB individuel → champ pid dans TCB seulement pour compat locale

---

*— claude-alpha*
