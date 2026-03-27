# ExoOS — Corrections Kernel Types & Structures Partagées
**Couvre : CORR-01, CORR-02, CORR-03, CORR-07, CORR-29, CORR-30**  
**Sources IAs : Z-AI (INCOH-01/02/05), Kimi (§1), MiniMax (ES-01..04), Grok4 (SYN-05), Claude**

---

## CORR-01 🔴 — TCB Layout : Architecture v7 est canonique

### Problème
Architecture v7 et ExoPhoenix v6 définissent deux TCB 256B incompatibles.  
Kernel B introspecte les structures de Kernel A → les deux doivent être **identiques**.

**Champs manquants dans ExoPhoenix v6 §3 :**
- `cr3_phys` [56] — adresse PML4 physique lue par switch_asm.s
- `fpu_state_ptr` [232] — pointeur XSaveArea (Lazy FPU)
- `rq_next` [240] / `rq_prev` [248] — chaînage intrusive RunQueue
- `_pad` [52] : 4B dans v7, 12B dans v6 → décalage de 8 octets sur tout le reste

**Décision** : Architecture v7 est la source canonique (plus complet, plus récent).

### Correction — `kernel/src/scheduler/core/task.rs`

```rust
// TCB LAYOUT CANONIQUE v8 — SOURCE UNIQUE DE VÉRITÉ
// Valide pour Kernel A et Kernel B (introspection ExoPhoenix)
// Double-vérifié par : Z-AI INCOH-01, Kimi §1, Grok4 SYN-05, Claude

#[repr(C, align(64))]
pub struct ThreadControlBlock {
    // ═══════════════════════════════════════════════════════
    // CACHE LINE 1 [0..63] — Données chaudes scheduler
    // ═══════════════════════════════════════════════════════

    /// [0]  8B — Pointeur vers la CapTable partagée du processus.
    /// PARTAGÉ entre tous les threads du même processus.
    /// Kernel B : lire pour valider region, NE PAS déréférencer depuis B.
    pub cap_table_ptr:  u64, // *const CapTable (stocké en u64 pour #[repr(C)])

    /// [8]  8B — RSP Ring 0, source de vérité pour TSS.RSP0 (V7-C-03).
    pub kstack_ptr:     u64,

    /// [16] 8B — Thread ID global unique.
    pub tid:            u64,

    /// [24] 8B — AtomicU8 logiquement : RUNNING(0)/BLOCKED(1)/ZOMBIE(2)/DEAD(3).
    /// Stocké en u64 pour alignement cache line.
    pub sched_state:    u64,

    /// [32] 8B — MSR 0xC0000100 (FS.base) — TLS userspace.
    /// Sauvegardé via RDMSR / restauré via WRMSR dans switch.rs (pas switch_asm.s).
    pub fs_base:        u64,

    /// [40] 8B — MSR 0xC0000101 (GS.base) — valeur USERSPACE uniquement.
    /// Le kernel utilise SWAPGS ; cette valeur est restaurée au retour Ring 3.
    /// Nommé user_gs_base pour éviter la confusion avec GS.base kernel.
    pub user_gs_base:   u64,

    /// [48] 4B — Intel PKS 32b (zéro sur AMD).
    pub pkrs:           u32,

    /// [52] 4B — Alignement cache line 1.
    pub _pad_cl1:       [u8; 4],

    /// [56] 8B — Adresse physique PML4 — lu par switch_asm.s pour MOV CR3.
    /// CRITIQUE : absent de ExoPhoenix v6 → KPTI impossible.
    pub cr3_phys:       u64,

    // ═══════════════════════════════════════════════════════
    // CACHE LINES 2-3 [64..191] — GPRs (128B = 16×8)
    // ═══════════════════════════════════════════════════════

    /// [64..183] — 15 GPRs × 8B : rax, rbx, rcx, rdx, rsi, rdi, rbp, r8..r14.
    /// NB : switch_asm.s (yield coopératif) sauvegarde uniquement les 6 callee-saved
    /// (rbx, rbp, r12..r15). Les caller-saved sont dans le frame du caller.
    /// En context préemptif (IRQ), TOUS les GPRs sont empilés par le CPU/handler.
    /// Ce champ représente l'état COMPLET pour restore depuis contexte IRQ/preempt.
    pub gpr:            [u64; 15], // rax, rbx, rcx, rdx, rsi, rdi, rbp, r8..r14

    /// [184] 8B — Alignement zone GPR (r15 logiquement [176], padding [184]).
    pub _pad_gpr:       [u8; 8],

    // ═══════════════════════════════════════════════════════
    // CACHE LINE 4 [192..255] — Registres Ring 3
    // ═══════════════════════════════════════════════════════

    /// [192] 8B — Instruction pointer Ring 3.
    pub rip:            u64,

    /// [200] 8B — Stack pointer Ring 3 (distinct de kstack_ptr).
    pub rsp_user:       u64,

    /// [208] 8B — EFLAGS étendu.
    pub rflags:         u64,

    /// [216] 8B — cs<<32 | ss (segment selectors compactés).
    pub cs_ss:          u64,

    /// [224] 8B — Page fault address — DIAGNOSTIC ExoPhoenix UNIQUEMENT.
    /// Jamais restauré via MOV CR2 (non supporté par CPU x86_64).
    pub cr2:            u64,

    /// [232] 8B — Pointeur vers XSaveArea allouée dynamiquement.
    /// null si le thread n'a jamais utilisé la FPU (Lazy FPU).
    /// Libéré dans release_thread_resources() ET dans do_exit().
    pub fpu_state_ptr:  u64, // *mut XSaveArea

    /// [240] 8B — RunQueue intrusive : next. null si thread BLOCKED.
    pub rq_next:        u64, // *mut ThreadControlBlock

    /// [248] 8B — RunQueue intrusive : prev. null si thread BLOCKED.
    pub rq_prev:        u64, // *mut ThreadControlBlock
}

// Vérifications compile-time obligatoires
const _: () = assert!(core::mem::size_of::<ThreadControlBlock>() == 256);
const _: () = assert!(core::mem::align_of::<ThreadControlBlock>() == 64);

// Vérification des offsets critiques (doit correspondre à switch_asm.s)
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, kstack_ptr)  == 8);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, cr3_phys)    == 56);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, rip)         == 192);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, fpu_state_ptr) == 232);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, rq_next)     == 240);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, rq_prev)     == 248);
```

### Fichiers à corriger
- `kernel/src/scheduler/core/task.rs` — remplacer le TCB par le layout ci-dessus
- `ExoPhoenix_Spec_v6.docx §3` — remplacer intégralement par ce layout

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
