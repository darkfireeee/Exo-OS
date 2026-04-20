# RAPPORT D'AUDIT EXHAUSTIF — COMMIT 42a80250 (FIX 3.1)
## Vérification indépendante des findings Qwen + Claude2 + nouveaux bugs

---

## SYNTHÈSE

| ID | Sévérité | Statut | Description |
|----|----------|--------|-------------|
| FIX-CET-01 | **P0** | ✅ Confirmé | SSP (CET Shadow Stack Pointer) jamais sauvé/restauré en context switch |
| FIX-KPTI-01 | **P0** | ✅ Confirmé | `CURRENT_CR3_KERNEL/USER` globaux partagés entre tous les CPUs |
| FIX-VRUNTIME-01 | **P0** | ✅ Confirmé | Overflow non protégé dans `should_preempt_on_wakeup()` |
| FIX-SWITCH-ASM-01 | **P0** | 🆕 NOUVEAU | `switch_to_new_thread` sauve MXCSR+FCW (16B) mais `context_switch_asm` ne les restaure pas → désalignement de pile fatal |
| FIX-SLABCACHE-01 | **P1** | ✅ Confirmé | `SlabCache` sans `align(64)` → false sharing `inner`/`stats` |
| FIX-RQ-ALIGN-01 | **P1** | ✅ Confirmé | `PerCpuRunQueue` sans `align(64)` → false sharing entre CPUs adjacents |
| FIX-CANARY-01 | **P2** | ✅ Confirmé | `canary.rs::MAX_CPUS` local au lieu d'importer depuis `constants.rs` |
| INC-03 HPET | — | ❌ Faux positif | Déjà corrigé : `wrapping_sub` présent |
| OUB-09 TCB offsets | — | ❌ Faux positif | Assertions compile-time présentes et correctes |

---

## FIX-CET-01 — P0 : Shadow Stack Pointer non sauvegardé

**Fichiers** : `kernel/src/scheduler/core/switch.rs`, `kernel/src/scheduler/asm/switch_asm.s`

**Preuve** : `context_switch()` dans `switch.rs` ne lit/écrit jamais `MSR_IA32_PL0_SSP` (0x6A4).
`switch_asm.s` est totalement silencieux sur les instructions CET (aucun SAVEPREVSSP, RSTORSSP, SETSSBSY).

Le TCB a bien `shadow_stack_token` en `_cold_reserve[0..7]` (offset 144), mais ce token n'est utilisé
que par `exocage.rs` lors de l'activation initiale, pas à chaque context switch.

**Impact** : Dès que CET est activé sur un CPU, le premier context switch entre deux threads
avec des shadow stacks distinctes provoque une exception #CP ou une corruption de shadow stack.

**Correction** : `FIX-CET-01_switch.rs.patch`

---

## FIX-KPTI-01 — P0 : CR3 KPTI partagés globalement

**Fichiers** : `kernel/src/arch/x86_64/spectre/kpti.rs`

**Preuve** :
```rust
static CURRENT_CR3_KERNEL: AtomicU64 = AtomicU64::new(0); // ← UN seul pour 256 CPUs
static CURRENT_CR3_USER:   AtomicU64 = AtomicU64::new(0); // ← idem
```

`kpti_switch_to_user()` et `kpti_switch_to_kernel()` lisent ces globaux. Sur SMP, CPU1 écrase
les valeurs de CPU0, qui charge ensuite le CR3 du mauvais thread.

`kpti_split.rs` a bien une structure per-CPU (`KptiTable.states[cpu_id]`) mais `kpti.rs` ne l'utilise
pas — les deux mécanismes coexistent de façon incohérente.

**Correction** : `FIX-KPTI-01_kpti.rs.patch`

---

## FIX-VRUNTIME-01 — P0 : Overflow dans `should_preempt_on_wakeup()`

**Fichiers** : `kernel/src/scheduler/policies/cfs.rs`

**Preuve** :
```rust
if woken_vr + CFS_WAKEUP_PREEMPT_NS < running_vr {  // ← addition standard = panic en debug
```

`CFS_WAKEUP_PREEMPT_NS = 1_000_000`. Si un thread tourne depuis longtemps,
`woken_vr` peut approcher `u64::MAX`. L'addition déborde.

**Correction** : `FIX-VRUNTIME-01_cfs.rs.patch`

---

## FIX-SWITCH-ASM-01 — P0 🆕 NOUVEAU : Désalignement pile dans `switch_to_new_thread`

**Fichiers** : `kernel/src/scheduler/asm/switch_asm.s`

**Preuve** : Dans `switch_to_new_thread`, la sauvegarde du thread SORTANT est :
```asm
pushq %r15 / %r14 / %r13 / %r12 / %rbp / %rbx  ; 48 octets
subq $16, %rsp      ; ← 16 octets SUPPLÉMENTAIRES pour MXCSR+FCW
stmxcsr 0(%rsp)
fstcw   8(%rsp)
movq %rsp, (%rdi)   ; kstack_ptr sauvé ici, 16 octets TROP BAS
```

Quand ce thread est restauré via `context_switch_asm` :
```asm
movq %rsi, %rsp   ; rsp pointe sur la zone MXCSR (pas sur %rbx)
popq %rbx         ; charge MXCSR dans rbx  ← CORROMPU
popq %rbp         ; charge FCW dans rbp    ← CORROMPU
popq %r12..r15    ; charge les anciens rbx,rbp,r12,r13
ret               ; saute vers l'adresse dans r14 original  ← CRASH
```

Le commentaire dans `create.rs` dit explicitement `(SANS MXCSR+FCW)` — contrat documenté
mais violé dans l'ASM. Ce bug est silencieux jusqu'au premier fork/kthread-creation
suivi d'un retour au thread parent.

**Correction** : `FIX-SWITCH-ASM-01_switch_asm.s.patch`

---

## FIX-SLABCACHE-01 — P1 : False sharing SlabCache

**Fichiers** : `kernel/src/memory/physical/allocator/slab.rs`

**Preuve** :
```rust
pub struct SlabCache {           // ← aucun #[repr(align(64))]
    inner:   Mutex<SlabCacheInner>,
    pub stats:   SlabCacheStats,  // ← sur la même cache line que inner
```

`SlabHeader` est bien `#[repr(C, align(64))]` mais `SlabCache` ne l'est pas.
Sous charge SMP, l'acquisition du mutex `inner` par CPU0 invalide la cache line
des `stats` sur CPU1 → contention silencieuse.

**Correction** : `FIX-SLABCACHE-01_slab.rs.patch`

---

## FIX-RQ-ALIGN-01 — P1 : False sharing PerCpuRunQueue

**Fichiers** : `kernel/src/scheduler/core/runqueue.rs`

**Preuve** :
```rust
pub struct PerCpuRunQueue {   // ← aucun #[repr(C, align(64))]
    pub cpu: CpuId,
    rt:      RtRunQueue,      // struct massive
    ...
}
static mut PER_CPU_RQ: [MaybeUninit<PerCpuRunQueue>; MAX_CPUS]
// ← CPUs adjacents peuvent partager des cache lines dans ce tableau
```

Si `sizeof(PerCpuRunQueue)` n'est pas un multiple de 64 bytes, CPU0 et CPU1 partagent
une cache line en fin/début de structure → false sharing garanti à 256 CPUs.

**Correction** : `FIX-RQ-ALIGN-01_runqueue.rs.patch`

---

## FIX-CANARY-01 — P2 : MAX_CPUS local dans canary.rs

**Fichiers** : `kernel/src/memory/integrity/canary.rs`

**Preuve** :
```rust
const MAX_CPUS: usize = 256;  // ← local, pas importé de memory::core::constants
// ...
unsafe { core::mem::transmute::<[u8; MAX_CPUS * 64], [CanarySlot; MAX_CPUS]>(...) }
```

Si `memory::core::constants::MAX_CPUS` passe à 512 pour une future config,
`canary.rs` reste à 256 → le transmute devient UB silencieux.

**Correction** : `FIX-CANARY-01_canary.rs.patch`
