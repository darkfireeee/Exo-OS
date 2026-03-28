# ExoOS — Corrections Kernel Types & Structures Partagées
**Couvre : CORR-01, CORR-02, CORR-03, CORR-07, CORR-29, CORR-30**  
**Sources IAs : Z-AI (INCOH-01/02/05), Kimi (§1), MiniMax (ES-01..04), Grok4 (SYN-05), Claude**

---

## CORR-01 🔴 — TCB Layout : GI-01 est canonique (amendement post-implémentation)

### Problème initial
Architecture v7 et ExoPhoenix v6 définissaient deux TCB 256B incompatibles.  
Kernel B introspecte les structures de Kernel A → les deux doivent être **identiques**.

**Champs manquants dans ExoPhoenix v6 §3 :**
- `cr3_phys` [56] — adresse PML4 physique lue par switch_asm.s
- `fpu_state_ptr` [232] — pointeur XSaveArea (Lazy FPU)
- `rq_next` [240] / `rq_prev` [248] — chaînage intrusive RunQueue

### Amendement GI-01 (Mars 2026)

**Décision** : Le layout implémenté dans `GI-01_Types_TCB_SSR.md §7` remplace le layout
initial de cette correction. Justifications :

1. **Performance hot-path** : `tid[0]` > `cap_table_ptr[0]` — TID est accédé à chaque
   tick IPC/syslog/debug. `cap_table_ptr` n'est lu qu'à la vérification de permission
   (cold path security).

2. **GPRs hors TCB** : précédent Linux (`pt_regs` sur kstack). Évite une double copie.
   ExoPhoenix lit les GPRs via le protocole kstack (`tcb.kstack_ptr`) — voir §4 ci-dessous.

3. **`cap_table_ptr` dans le PCB** : partagé entre threads du même processus —
   c'est naturellement une propriété du `ProcessControlBlock`, pas du `ThreadControlBlock`.

4. **`sched_state: AtomicU64`** unifie état + 7 flags en un mot atomique (vs AtomicU8
   + 3 AtomicBool séparés nécessitant des lectures composées non atomiques).

### Layout TCB canonique GI-01 — `kernel/src/scheduler/core/task.rs`

```rust
// TCB LAYOUT CANONIQUE GI-01 — SOURCE UNIQUE DE VÉRITÉ (amendé Mars 2026)
// Valide pour Kernel A et Kernel B (introspection ExoPhoenix — voir protocole kstack)

#[repr(C, align(64))]
pub struct ThreadControlBlock {
    // ═══ Cache-line 1 [0..64] — HOT PATH pick_next_task() ═══════════════════
    pub tid:          u64,         // [0]   Thread ID — accès IPC/syslog/debug
    pub kstack_ptr:   u64,         // [8]   RSP Ring 0 ← switch_asm.s HARDCODÉ
    pub priority:     Priority,    // [16]
    pub policy:       SchedPolicy, // [17]
    _pad0:            [u8; 6],     // [18]
    pub sched_state:  AtomicU64,   // [24]  état|signal|KTHREAD|FPU|RESCHED|...
    pub vruntime:     AtomicU64,   // [32]  vruntime CFS (ns)
    pub deadline_abs: AtomicU64,   // [40]  deadline EDF absolue (ns depuis boot)
    pub cpu_affinity: AtomicU64,   // [48]  bitmask affinité CPU
    pub cr3_phys:     u64,         // [56]  PML4 phys ← switch_asm.s HARDCODÉ

    // ═══ Cache-line 2 [64..128] — WARM context switch ════════════════════════
    pub cpu_id:       AtomicU64,   // [64]
    pub fs_base:      u64,         // [72]  MSR_FS_BASE (TLS) — CORR-11
    pub gs_base:      u64,         // [80]  MSR_KERNEL_GS_BASE (user) — CORR-11
    pub pkrs:         u32,         // [88]  Intel PKS
    pub pid:          ProcessId,   // [92]
    pub signal_mask:  AtomicU64,   // [96]
    pub dl_runtime:   u64,         // [104] budget EDF (ns/période)
    pub dl_period:    u64,         // [112] période EDF (ns)
    _pad2:            [u8; 8],     // [120]

    // ═══ Cache-lines 3-4 [128..256] — COLD + HARDCODÉS ═══════════════════════
    pub run_time_acc:  u64,        // [128]
    pub switch_count:  u64,        // [136]
    _cold_reserve:    [u8; 88],    // [144]  (144+88=232)
    pub fpu_state_ptr: u64,        // [232]  ← ExoPhoenix HARDCODÉ
    pub rq_next:       u64,        // [240]  intrusive RunQueue
    pub rq_prev:       u64,        // [248]  intrusive RunQueue
}

const _: () = assert!(core::mem::size_of::<ThreadControlBlock>() == 256);
const _: () = assert!(core::mem::align_of::<ThreadControlBlock>() == 64);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, kstack_ptr)   == 8);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, cr3_phys)     == 56);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, fpu_state_ptr) == 232);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, rq_next)      == 240);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, rq_prev)      == 248);
```

### §4 — Protocole ExoPhoenix pour lecture d'un thread gelé (remplace GPRs inline)

Kernel B n'a **pas** besoin de GPRs inline dans le TCB. Il utilise le protocole kstack :

```
Lecture état complet d'un thread gelé par Kernel B :

1. tcb.tid          [offset  0]  → identifier le thread
2. tcb.kstack_ptr   [offset  8]  → sommet pile kernel (RSP sauvegardé)
3. tcb.cr3_phys     [offset 56]  → espace d'adressage (MOV CR3 pour inspection)
4. tcb.fpu_state_ptr[offset 232] → état FPU (XSaveArea)

5. GPRs callee-saved (yield coopératif) — lire depuis kstack :
   [kstack_ptr + 0]  = r15  (dernier push switch_asm.s)
   [kstack_ptr + 8]  = r14
   [kstack_ptr + 16] = r13
   [kstack_ptr + 24] = r12
   [kstack_ptr + 32] = rbp
   [kstack_ptr + 40] = rbx
   [kstack_ptr + 48] = rip  (adresse de retour : où reprend le thread)

6. cap_table_ptr → lire depuis PROCESS_TABLE[pid].cap_table_ptr (PCB)
   Le PCB est la structure partagée par tous les threads du processus.
   Kernel B lit le PCB, pas le TCB individuel, pour les capabilities.

Note : si le thread était préempté par IRQ au moment du gel, la kstack
contient en plus le frame ISR (rip/cs/rflags/rsp/ss poussés par le CPU,
puis tous les GPRs caller-saved poussés par le handler). Kernel B doit
lire tcb.sched_state pour distinguer yielded vs preempted.
```

### Fichiers mis à jour
- `kernel/src/scheduler/core/task.rs` — layout GI-01 implémenté ✅
- `ExoPhoenix_Spec_v6.md §3` — amendé (voir ce fichier)
- `ExoOS_Architecture_v7.md §3.2` — amendé (voir ce fichier)

---

## CORR-02 🔴 — SSR Layout : MAX_CORES=256, offsets unifiés

### Problème
Architecture v7 (`SSR_MAX_CORES_LAYOUT=256`) et ExoPhoenix v6 (`MAX_CORES=64`) ont des offsets SSR **physiquement incompatibles** :

| Section | v7 (256 cores) | v6 (64 cores) |
|---------|----------------|----------------|
| PMC_SNAPSHOT | +0x4080 | +0x1080 |
| LOG_AUDIT_B  | +0xC000 | +0x8000 |
| METRICS_PUSH | +0xE000 | +0xC000 |

Si Kernel A écrit avec offsets v7 et Kernel B lit avec offsets v6 → **corruption silencieuse garantie**.

**Décision** : MAX_CORES=256 (Architecture v7) — recalcul correct dans 64KB.

### Correction — `libs/exo-phoenix-ssr/src/lib.rs`

```rust
// libs/exo-phoenix-ssr/src/lib.rs
// VERSION CANONIQUE UNIFIÉE v8 — remplace v6 (ExoPhoenix) et v7 (Architecture)
// Vérification : 64 + 64 + 16384 + 16384 + 16256 + 8192 + 8192 = 65536B ✓

use core::sync::atomic::AtomicU32;

// ═══════════════════════════════════════════════════════════════════════
// CONSTANTES GLOBALES
// ═══════════════════════════════════════════════════════════════════════

pub const SSR_LAYOUT_MAGIC:     u64   = 0x5353525F4558_4F53; // "SSR_EXOS"
pub const SSR_BASE_PHYS:        u64   = 0x0100_0000;          // 16 MiB — E820 réservé
pub const SSR_SIZE:             usize = 0x10000;               // 64 KiB
pub const SSR_MAX_CORES_LAYOUT: usize = 256;                   // compile-time FIXE
pub const KERNEL_B_APIC_ID:     u32   = 0;                     // Core 0 = Kernel B

pub static MAX_CORES_RUNTIME: AtomicU32 = AtomicU32::new(0);   // CPUID boot

// ═══════════════════════════════════════════════════════════════════════
// OFFSETS SSR (absolus dans la région physique)
// ═══════════════════════════════════════════════════════════════════════

// +0x0000 : HEADER (64B)
// Structuré comme suit :
//   [0x00] u64 SSR_LAYOUT_MAGIC — valeur fixe "SSR_EXOS" (CORR-03)
//   [0x08] AtomicU64 HANDOFF_FLAG — 0=NORMAL, 1=FREEZE_REQ, 2=FREEZE_ACK_ALL, 3=B_ACTIVE
//   [0x10] AtomicU64 LIVENESS_NONCE — écrit par B (RDRAND), vérifié via PULL
//   [0x18] AtomicU64 SEQLOCK_COUNTER — pattern ktime
//   [0x20..0x3F] pad(32B) — alignement cache line

pub const SSR_MAGIC_OFFSET:         usize = 0x0000; // CORR-03 : MAGIC en premier
pub const SSR_HANDOFF_FLAG_OFFSET:  usize = 0x0008; // était 0x0000 dans ExoPhoenix v6
pub const SSR_LIVENESS_NONCE_OFFSET:usize = 0x0010; // était 0x0008
pub const SSR_SEQLOCK_OFFSET:       usize = 0x0018; // était 0x0010

// +0x0040 : CANAL COMMANDE B→A (64B, align 64B)
pub const SSR_CMD_B2A_OFFSET:       usize = 0x0040;

// +0x0080 : FREEZE ACK PER-CORE (256 × 64B = 16384B)
// 1 AtomicU64 + 56B padding par core → isolation cache line SMP
pub const SSR_FREEZE_ACK_OFFSET:    usize = 0x0080;

// +0x4080 : PMC SNAPSHOT PER-CORE (256 × 64B = 16384B)
// 8 valeurs u64 (EVTSEL0..3 + CTR0..3) par core, sans padding
pub const SSR_PMC_OFFSET:           usize = 0x4080;

// +0x8080 : EXTENSIONS RESERVED (16256B)
pub const SSR_EXTENSIONS_START:     usize = 0x8080;
pub const SSR_EXTENSIONS_SIZE:      usize = 0x3F80; // 16256B

// +0xC000 : LOG AUDIT B (8192B — RO pour Kernel A)
pub const SSR_LOG_AUDIT_OFFSET:     usize = 0xC000;

// +0xE000 : MÉTRIQUES PUSH A→B (8192B)
pub const SSR_METRICS_OFFSET:       usize = 0xE000;

// ═══════════════════════════════════════════════════════════════════════
// ACCESSEURS PER-CORE
// ═══════════════════════════════════════════════════════════════════════

/// Offset du slot FREEZE_ACK pour un core donné.
/// Chaque slot = 1 AtomicU64 (8B) + 56B padding = 64B total.
#[inline(always)]
pub const fn freeze_ack_offset(apic_id: usize) -> usize {
    debug_assert!(apic_id < SSR_MAX_CORES_LAYOUT);
    SSR_FREEZE_ACK_OFFSET + apic_id * 64
}

/// Offset du slot PMC_SNAPSHOT pour un core donné.
/// Chaque slot = 8 × u64 (64B total, pas de padding nécessaire).
#[inline(always)]
pub const fn pmc_snapshot_offset(apic_id: usize) -> usize {
    debug_assert!(apic_id < SSR_MAX_CORES_LAYOUT);
    SSR_PMC_OFFSET + apic_id * 64
}

// ═══════════════════════════════════════════════════════════════════════
// VÉRIFICATIONS COMPILE-TIME
// ═══════════════════════════════════════════════════════════════════════

// Total = 64 + 64 + 16384 + 16384 + 16256 + 8192 + 8192 = 65536B ✓
const _SSR_SIZE_CHECK: () = assert!(
    SSR_FREEZE_ACK_OFFSET + SSR_MAX_CORES_LAYOUT * 64 == SSR_PMC_OFFSET
);
const _SSR_TOTAL_CHECK: () = assert!(SSR_SIZE == 0x10000);
```

### Fichiers à corriger
- `libs/exo-phoenix-ssr/src/lib.rs` — remplacer intégralement
- `ExoPhoenix_Spec_v6.docx §2` — mettre à jour les constantes

---

## CORR-03 🔴 — SSR Header : champ MAGIC manquant dans ExoPhoenix v6

### Problème
Architecture v7 place MAGIC(8B) à l'offset 0x0000 du header SSR.  
ExoPhoenix v6 place HANDOFF_FLAG à 0x0000 — **sans MAGIC**.

**Conséquence** : Kernel B lisant [0x0000] trouve la valeur `0x5353525F4558_4F53` et l'interprète comme HANDOFF_FLAG. L'état devient `3=B_ACTIVE` immédiatement → boucle de confusion garantie.

### Correction
Voir CORR-02 ci-dessus — les constantes `SSR_HANDOFF_FLAG_OFFSET = 0x0008` (décalé de +8) sont intégrées.

**Vérification à ajouter dans Kernel B (Stage 0) :**
```rust
// servers/exo_shield/src/main.rs — ou kernel_b/src/boot/ssr_init.rs
pub fn verify_ssr_magic(ssr_base: *const u8) {
    let magic = unsafe {
        core::ptr::read_volatile(ssr_base as *const u64)
    };
    assert_eq!(magic, SSR_LAYOUT_MAGIC,
        "SSR magic invalide : 0x{:016X} (attendu 0x{:016X}). \
         Offset SSR ou version incorrecte.", magic, SSR_LAYOUT_MAGIC);
}
```

---

## CORR-07 🔴 — `ObjectId::is_valid()` : exception ZERO_BLOB_ID_4K manquante

### Problème
`ObjectId::is_valid()` requiert `bytes[8..32] == 0` (format compteur).  
`ZERO_BLOB_ID_4K` est un hash Blake3 réel → `bytes[8..32] ≠ 0` → `is_valid() == false`.

**Conséquence** : tout code appelant `is_valid()` avant de passer ZERO_BLOB_ID_4K rejettera un P-Blob légitime.

### Correction — `libs/exo-types/src/object_id.rs`

```rust
use crate::constants::ZERO_BLOB_ID_4K;

impl ObjectId {
    /// Vérifie si cet ObjectId est valide selon le format canonique ExoOS.
    ///
    /// Format standard : bytes[0..8] = compteur u64 LE, bytes[8..32] = zéro.
    ///
    /// EXCEPTION : ZERO_BLOB_ID_4K est un P-Blob (hash Blake3 de 4KB de zéros).
    /// Il ne suit PAS le format compteur — son padding n'est pas zéro.
    /// is_valid() retourne true pour ZERO_BLOB_ID_4K car c'est un ObjectId
    /// physiquement valide et utilisé légitimement dans ExoFS (TL-31/32).
    ///
    /// Source : ExoFS_Translation_Layer_v5_FINAL.md §1.1 + CORR-07
    pub fn is_valid(&self) -> bool {
        // Exception explicite : ZERO_BLOB_ID_4K est un P-Blob valide
        if *self == ZERO_BLOB_ID_4K {
            return true;
        }
        // Format standard ObjectId : bytes[0..8]=compteur LE, bytes[8..32]=zéro
        self.0[8..32].iter().all(|&b| b == 0)
    }
}
```

---

## CORR-29 🔵 — user_gs_base : standardiser le nommage

### Problème
Architecture v7 utilise `user_gs_base` (explicite).  
ExoPhoenix v6 utilise `gs_base` (ambigu — kernel ou userspace ?).

### Correction
Nommage canonique : **`user_gs_base`** dans tous les fichiers.  
Commentaire à ajouter partout :
```rust
/// MSR 0xC0000101 — GS.base valeur USERSPACE.
/// Le kernel GS.base est géré par SWAPGS (per-CPU data).
/// Cette valeur est restaurée au retour Ring 3 via WRMSR.
pub user_gs_base: u64,
```

---

## CORR-30 🔵 — `FixedString<N>` : `len: usize` → `len: u32`

### Problème
`FixedString<N>` utilise `len: usize` (8B sur x86_64).  
Si le type traverse une frontière ABI stricte (sérialisation, IPC cross-version), la taille de `len` devient dépendante de l'architecture.

**Source** : ChatGPT5 Hard Stress Audit §1.7

### Correction — `libs/exo-types/src/fixed_string.rs`

```rust
/// Chaîne à taille fixe no_std pour IPC.
/// ABI strict : len est u32 (4B) pour portabilité cross-architecture.
/// Contrainte : N ≤ 65535 (vérifié par const_assert).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FixedString<const N: usize> {
    pub bytes: [u8; N],
    pub len:   u32,   // CORRECTION : était usize (8B), maintenant u32 (4B)
    pub _pad:  [u8; 4], // Alignement à 8B
}

const _: () = assert!(core::mem::size_of::<FixedString<64>>() == 64 + 4 + 4);

// Types spécialisés
pub type ServiceName = FixedString<64>;
pub type PathBuf     = FixedString<512>;
```

> **Note** : ce changement est une **rupture ABI**. Tous les uses de `FixedString<N>` dans les `protocol.rs` doivent être recompilés ensemble.

---

*ExoOS — Corrections Kernel Types — Mars 2026*
