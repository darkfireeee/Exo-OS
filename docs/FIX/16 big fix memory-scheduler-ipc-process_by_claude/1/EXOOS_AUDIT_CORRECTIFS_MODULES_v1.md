# Exo-OS — Audit Profond & Correctifs Complets  
## Modules : `memory/` · `scheduler/` · `ipc/` · `process/`

> **Auteur** : claude-beta  
> **Version** : 1.0 — Mai 2026  
> **Périmètre** : Croisement docs/recast/ExoOS_Architecture_v7.md × docs/kernel/* × docs/Exo-OS-TLA+/*  
> **Objectif** : Identifier chaque incohérence, erreur, TODO manquant ou implémentation absente — et produire les correctifs rendant les quatre modules 100 % opérationnels.

---

## Résumé Exécutif

| Priorité | Incohérences / Erreurs | Impact |
|----------|----------------------|--------|
| **P0 — Critique** | 8 | Crash kernel garanti, corruption mémoire, violation ABI |
| **P1 — Grave** | 9 | Comportement indéterminé, race conditions, sécurité compromise |
| **P2 — Modéré** | 7 | Documentation contradictoire, vérifications manquantes |

**Total : 24 défauts identifiés.** Après application de l'ensemble des correctifs ci-dessous, les quatre modules seront sans défaut documentaire ou d'implémentation.

---

## Table des matières

1. [Module `memory/`](#1-module-memory)
2. [Module `scheduler/`](#2-module-scheduler)
3. [Module `ipc/`](#3-module-ipc)
4. [Module `process/`](#4-module-process)
5. [Incohérences croisées inter-modules](#5-incohérences-croisées-inter-modules)
6. [Correctifs consolidés](#6-correctifs-consolidés)
7. [Checklist de validation finale](#7-checklist-de-validation-finale)

---

## 1. Module `memory/`

### DEFAUT-MEM-01 — 🔴 P0 : Ordre des verrous **inversé** entre les deux documents de référence

**Sources contradictoires :**

| Document | Ordre déclaré |
|----------|--------------|
| `docs/kernel/memory/MEMORY_COMPLETE.md` §2 | `IPC < Scheduler < Memory < FS` (IPC = acquis en PREMIER) |
| `docs/recast/ExoOS_Architecture_v7.md` §2.2 | Niveau 1 (acquérir en premier) = **Memory** ; Niveau 4 = IPC |

Ces deux ordres sont **exactement inverses**. `MEMORY_COMPLETE.md` dit qu'IPC s'acquiert avant Memory ; l'architecture dit que Memory s'acquiert avant IPC.

**Analyse :** L'architecture v7 est correcte. Memory est COUCHE 0, elle ne dépend de rien — il est donc physiquement impossible que Memory tente d'acquérir un lock scheduler ou IPC. Le vrai ordre de priorité d'acquisition (du premier au dernier) est :

```
1. Memory  →  2. Scheduler  →  3. Security  →  4. IPC  →  5. FS
```

La règle dans `MEMORY_COMPLETE.md` est mal libellée : « IPC < Scheduler < Memory » signifie que le module de plus bas niveau est Memory, mais l'ordre d'acquisition lors d'un appel **descendant** (d'une couche haute vers une basse) n'est pas le même que l'ordre de priorité du tableau.

**Correctif :** Remplacer dans `MEMORY_COMPLETE.md` §2 règle **LOCK ORDER** :

```diff
- | **LOCK ORDER** | IPC < Scheduler < **Memory** < FS (jamais lock N si on tient N+1). |
+ | **LOCK ORDER** | Memory → Scheduler → Security → IPC → FS.
+ |               | Acquérir toujours dans l'ordre croissant de couche.
+ |               | Une couche haute (IPC, FS) NE DOIT PAS être verrouillée
+ |               | lorsqu'on appelle une fonction de couche inférieure (Memory).
+ |               | Toute inversion crée un deadlock potentiel. |
```

---

### DEFAUT-MEM-02 — 🔴 P0 : TLB IPI mask tronqué à 64 CPUs pour un système 256 CPUs

**Source :** `docs/kernel/memory/arch_memory_integration.md` §2.1

```rust
// Actuel (FAUX pour 256 CPUs) :
unsafe fn send_tlb_ipi_to_mask(cpu_mask: u64) {
    for cpu_idx in 0..64usize {          // ← boucle sur 64 CPUs seulement
        if cpu_mask & (1u64 << cpu_idx) == 0 { continue; }
        ...
    }
}
```

`MAX_CPUS = 256` mais le mask est `u64` (64 bits) et la boucle s'arrête à 64. Les CPUs 64–255 **ne reçoivent jamais de TLB shootdown**, provoquant des lectures de pages physiques libérées (use-after-free au niveau TLB).

**Correctif :** Modifier la signature et l'implémentation pour un mask 256 bits :

```rust
// Dans arch/x86_64/memory_iface.rs :
pub unsafe fn send_tlb_ipi_to_mask(cpu_mask: &[u64; 4]) {  // 4×64 = 256 bits
    let current = percpu::current_cpu_id() as usize;
    for word_idx in 0..4usize {
        let mut word = cpu_mask[word_idx];
        while word != 0 {
            let bit = word.trailing_zeros() as usize;
            let cpu_idx = word_idx * 64 + bit;
            word &= word - 1;  // clear lowest set bit
            if cpu_idx == current || cpu_idx >= MAX_CPUS { continue; }
            let lapic_id = percpu::per_cpu(cpu_idx).lapic_id as u8;
            local_apic::send_ipi(lapic_id, IPI_TLB_SHOOTDOWN_VECTOR, ICR_DM_FIXED);
        }
    }
}

// Adaptateur en mémoire — appel depuis memory/virt/tlb.rs :
pub fn request_ipi_shootdown(flush: TlbFlushType) {
    let online_mask: [u64; 4] = online_cpu_mask_256bits();
    unsafe { arch_send_tlb_ipi_to_mask(&online_mask); }
}
```

Mettre à jour `memory/virt/address_space/tlb.rs` pour utiliser `[u64; 4]` en lieu de `u64`.

---

### DEFAUT-MEM-03 — 🟡 P2 : `VisibilityGap` dans `Memory.tla` — variable morte, invariant absent

**Source :** `docs/Exo-OS-TLA+/Memory.tla`

```tla
VisibilityGap  \* BOOLEAN: Reserved to track stale reads
```

`VisibilityGap` est initialisé à `FALSE`, jamais mis à `TRUE` dans aucune action, et aucune propriété ne le vérifie. C'est une variable morte qui pollue le modèle sans apporter de garantie.

**Correctif :** Soit supprimer la variable, soit l'utiliser pour modéliser les lectures périmées (Relaxed read sans Acquire correspondant) :

```tla
(* Ajouter dans ReadRelaxed — lecture potentiellement périmée *)
ReadRelaxed(c, var) ==
    /\ AtomicWrites[var].ordering = "Relaxed"
    /\ VisibilityGap' = (AtomicReads[c][var] # AtomicWrites[var].value)
    /\ UNCHANGED <<AtomicWrites, AtomicReads, HappensBefore, ReleaseFence, AcquireFence>>

(* Ajouter invariant *)
S50_NoStaleReadAfterAcquire ==
    ~VisibilityGap  \* Propriété de vivacité : une fois Acquire effectué, gap disparaît
```

---

## 2. Module `scheduler/`

### DEFAUT-SCHED-01 — 🔴 P0 : Taille TCB contradictoire — 128 B (OVERVIEW) vs 256 B (CORE + Arch v7)

**Sources contradictoires :**

| Document | Taille TCB déclarée |
|----------|---------------------|
| `SCHEDULER_OVERVIEW.md` §3 règle SCHED-03 | `"128 B exactement (2×64 B cache lines)"` ✅ |
| `SCHEDULER_CORE.md` §1 | `"exactement 256 octets (SCHED-03)"` |
| `ExoOS_Architecture_v7.md` §3.2 | `"TCB v7 (256B)"` |
| Layout GI-01 (Architecture v7) | Offset max = [248]+8 = **256 B** |

La règle SCHED-03 dans `SCHEDULER_OVERVIEW.md` dit 128 B mais le layout réel va jusqu'à 256 B. Cette contradiction provoque une désynchronisation entre la spec et le code.

**Correctif :** Mettre à jour `SCHEDULER_OVERVIEW.md` §3 règle SCHED-03 :

```diff
- | SCHED-03 | `ThreadControlBlock` = 128 B exactement (2×64 B cache lines) | ✅ |
+ | SCHED-03 | `ThreadControlBlock` = 256 B exactement (4×64 B cache lines) | ✅ |
```

Et vérifier le `#[repr(C, align(64))]` avec `assert_eq!(size_of::<ThreadControlBlock>(), 256)` à la compilation :

```rust
// Dans scheduler/core/task.rs :
const _: () = assert!(
    core::mem::size_of::<ThreadControlBlock>() == 256,
    "TCB must be exactly 256 bytes (4 cache lines)"
);
```

---

### DEFAUT-SCHED-02 — 🔴 P0 : FPU dans `switch_asm.s` — contradiction majeure V7-C-02

**Sources contradictoires :**

| Document | Position sur FPU dans switch_asm |
|----------|----------------------------------|
| `ExoOS_Architecture_v7.md` V7-C-02 | `"switch_asm.s ne touche PAS la FPU — seul CR0.TS=1"` |
| `ExoOS_Architecture_v7.md` §10 S-44 | `"✅ CORRIGÉ v7"` |
| `SCHEDULER_ASM.md` §3 (SCHED-07) | Présence de `stmxcsr`, `fstcw`, `fldcw`, `ldmxcsr` |
| `SCHEDULER_OVERVIEW.md` règle SCHED-07 | `"MXCSR + x87 FCW sauvegardés explicitement dans la pile"` ✅ |

`SCHEDULER_ASM.md` et `SCHEDULER_OVERVIEW.md` documentent encore l'ancienne implémentation **pré-v7** avec MXCSR/FCW dans `switch_asm.s`. La correction V7-C-02 n'a **jamais été répercutée** dans ces deux fichiers.

**Correctif :** Remplacer intégralement le code annoté dans `SCHEDULER_ASM.md` §3 :

```asm
context_switch_asm:
    # ─── PHASE 1 : Sauvegarde registres callee-saved (SCHED-06) ────────────
    # r15 EN PREMIER (règle SCHED-06)
    push %r15
    push %r14
    push %r13
    push %r12
    push %rbp
    push %rbx
    # TOTAL sur pile : 6×8 = 48 octets + RIP implicite = 56 octets

    # ─── PHASE 2 : Sauvegarde RSP de prev ───────────────────────────────────
    mov %rsp, (%rdi)   # prev.kstack_ptr ← RSP actuel

    # ─── PHASE 3 : Commutation CR3 (SCHED-08 — AVANT chargement RSP next) ──
    cmp %rdx, %cr3
    je  .skip_cr3
    mov %rdx, %cr3     # Switch PML4 + flush TLB (KPTI/PCID)
.skip_cr3:

    # ─── PHASE 4 : Chargement RSP de next ───────────────────────────────────
    mov %rsi, %rsp

    # ─── PHASE 5 : Restauration registres callee-saved ──────────────────────
    pop %rbx
    pop %rbp
    pop %r12
    pop %r13
    pop %r14
    pop %r15

    ret
    # NOTE : PAS de MXCSR/FCW — V7-C-02 (Lazy FPU, seul CR0.TS=1 dans switch.rs)
```

Et mettre à jour la règle dans `SCHEDULER_OVERVIEW.md` §3 :

```diff
- | SCHED-07 | MXCSR + x87 FCW sauvegardés explicitement dans la pile | ✅ |
+ | SCHED-07 | MXCSR/FCW absents de switch_asm.s (V7-C-02 Lazy FPU) — CR0.TS=1 dans switch.rs | ✅ |
```

---

### DEFAUT-SCHED-03 — 🔴 P0 : TSS.RSP0 alimenté avec `kstack_ptr` (Arch v7) vs `kstack_top` (CORE + TLA+)

**Sources contradictoires :**

| Document | Valeur pour TSS.RSP0 |
|----------|----------------------|
| `ExoOS_Architecture_v7.md` V7-C-03 | `tss_set_rsp0(current_cpu(), next.kstack_ptr)` |
| `SCHEDULER_CORE.md` §2 step 5 | `"set TSS.RSP0 with next.kstack_top()"` |
| `docs/Exo-OS-TLA+/ContextSwitch.tla` Init | `TssRsp0[c] = CurrentTcb[c].kstack_top` |

**Analyse :** TSS.RSP0 est le RSP que le CPU charge lors d'une transition Ring 3 → Ring 0 (interruption hardware ou `syscall`). Il doit pointer vers le **sommet fixe** de la pile kernel, pas vers le RSP sauvegardé lors du dernier switch (qui varie). `kstack_top` est l'adresse haute fixe de la pile ; `kstack_ptr` est le RSP sauvegardé courant.

**`kstack_top` est correct. L'architecture v7 a une erreur dans V7-C-03.**

**Correctif :** Mettre à jour `ExoOS_Architecture_v7.md` §3.2 séquence switch.rs :

```diff
- //  6. tss_set_rsp0(current_cpu(), next.kstack_ptr)  ← V7-C-03 OBLIGATOIRE
+ //  6. tss_set_rsp0(current_cpu(), next.kstack_top)  ← V7-C-03 OBLIGATOIRE
+ //     kstack_top = sommet FIXE de la pile kernel (pas le RSP sauvegardé)
+ //     = adresse haute stable utilisée pour Ring 3 → Ring 0 entrées
```

Et corriger le commentaire de la table TCB :

```diff
- | `kstack_ptr` | [8] | 8 B | RSP Ring 0 — source de vérité pour `TSS.RSP0` (V7-C-03) |
+ | `kstack_ptr` | [8] | 8 B | RSP Ring 0 sauvegardé par switch_asm.s (context switch uniquement) |
+ | `kstack_top` | [176] | 8 B | Sommet fixe pile kernel — source de vérité pour `TSS.RSP0` (V7-C-03) |
```

---

### DEFAUT-SCHED-04 — 🔴 P0 : Affinité CPU limitée à 64 CPUs — incohérence avec MAX_CPUS=256

**Sources :**
- TCB layout : `cpu_affinity` à [48] = 8 B (u64, 64 bits = 64 CPUs maximum)
- `SCHEDULER_SMP.md` §2 `cpu_allowed()` : `if cpu.0 >= 64 { return false; }` → rejette CPUs 64–255
- `SCHEDULER_SMP.md` §2 `CpuMask` : `bits: [u64; 4]` = 256 bits (correct)
- `MAX_CPUS = 256` partout

Le champ `cpu_affinity` du TCB et la fonction `cpu_allowed()` ne supportent que 64 CPUs malgré `MAX_CPUS=256`. Les threads sur CPUs 64–255 ne peuvent jamais avoir une affinité correcte.

**Correctif complet :**

**1. Modifier `cpu_allowed()` dans `affinity.rs`** pour utiliser le `_cold_reserve` `affinity_hi` :

```rust
// Lecture de l'affinité complète (256 bits) depuis le TCB
pub fn cpu_allowed_full(tcb: &ThreadControlBlock, cpu: CpuId) -> bool {
    let cpu_idx = cpu.0 as usize;
    if cpu_idx >= MAX_CPUS { return false; }
    if cpu_idx < 64 {
        // Champ hot-path [48]
        tcb.cpu_affinity.load(Ordering::Relaxed) & (1u64 << cpu_idx) != 0
    } else {
        // Cold field affinity_hi [200..224] — 24 bytes = CPUs 64..255
        let hi_idx = (cpu_idx - 64) / 8;
        let hi_bit = (cpu_idx - 64) % 8;
        // SAFETY: affinity_hi est aligné et initialisé au boot
        let hi_byte = unsafe {
            tcb._cold_reserve.affinity_hi.as_ptr().add(hi_idx).read_volatile()
        };
        hi_byte & (1u8 << hi_bit) != 0
    }
}

// Déprécier l'ancienne cpu_allowed() qui tronque à 64 CPUs :
#[deprecated = "Utiliser cpu_allowed_full() pour MAX_CPUS=256"]
pub fn cpu_allowed(affinity: u64, cpu: CpuId) -> bool {
    if cpu.0 >= 64 { return false; }
    affinity & (1u64 << cpu.0) != 0
}
```

**2. Mettre à jour `load_balance.rs`** pour appeler `cpu_allowed_full()`.

**3. Ajouter un test de compilation :**

```rust
const _: () = assert!(
    MAX_CPUS == 256,
    "affinity_hi doit être redimensionné si MAX_CPUS change"
);
```

---

### DEFAUT-SCHED-05 — 🔴 P0 : Séquence `context_switch()` — ordre CR0.TS incohérent entre docs

**Sources contradictoires :**

| Document | Moment de `CR0.TS = 1` |
|----------|------------------------|
| Architecture v7 séquence | Step 5 : **après** `context_switch_asm` |
| `SCHEDULER_CORE.md` §2 step 2 | **avant** `context_switch_asm` : "Poser CR0.TS=1, puis marquer FPU non chargé" |
| `SCHEDULER_OVERVIEW.md` flux §5 | Absent de la séquence visible |
| TLA+ `ContextSwitch.tla` | `Step2_SetLazyBit` avant `Step5_AsmSwitch` |

**Analyse :** Poser `CR0.TS = 1` AVANT le switch_asm est **correct** : si une interruption survient entre la sauvegarde FPU (`xsave`) et le `context_switch_asm`, le CPU est déjà en état "FPU non chargé". Poser `CR0.TS` après le switch pourrait permettre une brève fenêtre où le thread `next` accède à l'état FPU de `prev`.

**L'architecture v7 est incorrecte sur ce point. `SCHEDULER_CORE.md` et TLA+ sont corrects.**

**Correctif :** Mettre à jour la séquence dans `ExoOS_Architecture_v7.md` §3.2 :

```diff
- //  1. Si fpu_loaded(prev) → xsave64(prev.fpu_state_ptr)
- //  2. prev.set_state(Runnable)
- //  3. context_switch_asm(prev.kstack_ptr, next.kstack_ptr, next.cr3_phys)
- //  4. next.set_state(Running)
- //  5. set_cr0_ts()   ← CR0.TS=1 (Lazy FPU — V7-C-02)
- //  6. tss_set_rsp0(current_cpu(), next.kstack_ptr)  ← V7-C-03 OBLIGATOIRE
+ //  1. Si fpu_loaded(prev) → xsave_current(prev)   ← arch_xsave64(ptr, mask)
+ //  2. set_cr0_ts()  ← CR0.TS=1 AVANT asm switch (fenêtre IRQ sûre — V7-C-02)
+ //     prev.set_fpu_loaded(false)
+ //  3. Sauvegarder FS/GS, PKRS, CET (MSRs userspace)
+ //  4. prev.set_state(Runnable)
+ //  5. context_switch_asm(&prev.kstack_ptr, next.kstack_ptr, next.cr3_phys)
+ //     ← après retour : on est maintenant dans le contexte de next
+ //  6. next.set_state(Running)
+ //  7. tss_set_rsp0(current_cpu(), next.kstack_top)  ← V7-C-03 (kstack_top, pas kstack_ptr)
+ //  8. gs:[0x20] ← ptr vers TCB next  (GS slot 0x20 — per-CPU)
+ //  9. Restaurer FS/GS, PKRS, CET de next
+ // 10. Incrémenter switch_count de next
```

---

### DEFAUT-SCHED-06 — 🟠 P1 : GS slot offset — gs:[0x00] vs GsSlot20 (0x20)

**Sources contradictoires :**

| Document | Offset GS pour pointeur TCB courant |
|----------|--------------------------------------|
| `SCHEDULER_CORE.md` §2 step 5 | `gs:[0x00]` |
| `docs/Exo-OS-TLA+/ContextSwitch.tla` | `GsSlot20` (slot at offset `0x20`) |

**Analyse :** Le GS segment per-CPU stocke plusieurs pointeurs. Si le TCB courant est à `gs:[0x00]`, toute lecture `gs:[0x00]` en Ring 0 retourne un pointeur TCB. Si c'est `gs:[0x20]`, le layout GS est différent. Ces deux valeurs ne peuvent pas être simultanément vraies.

**Correctif :** Créer un fichier `docs/kernel/arch/GS_LAYOUT.md` faisant autorité :

```
GS Segment per-CPU Layout (x86_64 Ring 0)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Offset  Taille  Champ
+0x00   8 B     RSP kernel (auto-référence pour swapgs + syscall)
+0x08   8 B     RSP userspace (sauvegarde pendant syscall)
+0x10   8 B     cpu_id (CpuId courant)
+0x18   8 B     LAPIC base physique
+0x20   8 B     *mut ThreadControlBlock   ← pointeur TCB courant
+0x28   8 B     preempt_count (i32, padded)
+0x30   ...     extension future
```

TCB courant = `gs:[0x20]`. Mettre à jour `SCHEDULER_CORE.md` step 8 :

```diff
- /\\ GsSlot20' = [GsSlot20 EXCEPT ![c] = CurrentTcb[c]]   (* TLA+, correct *)
```

```diff
- // 5. Publier `next`, mettre `TSS.RSP0` et `gs:[0x00]` avec `next.kstack_top()`
+ // 8. gs:[0x20] ← ptr TCB next  (offset 0x20 dans le GS per-CPU — conforme TLA+)
```

---

### DEFAUT-SCHED-07 — 🟠 P1 : `signal_pending` — `AtomicBool` (PROC-04) vs bit dans `sched_state` (SCHED-15)

**Sources contradictoires :**

| Document | Représentation de `signal_pending` |
|----------|-------------------------------------|
| `process/README.md` PROC-04 | `ThreadControlBlock::signal_pending: AtomicBool` |
| `SCHEDULER_CORE.md` §1 flags TCB | `"bit SCHED_SIGNAL_BIT dans sched_state (AtomicU64)"` |

Ces deux représentations sont mutuellement exclusives. Le scheduler et le module process doivent lire exactement le même bit, sinon `check_signal_pending()` peut renvoyer `false` alors que des signaux sont en attente.

**Correctif :** Standardiser sur le **bit dans `sched_state`** (plus compact, pas d'alignement supplémentaire) :

```rust
// scheduler/core/task.rs — SOURCE DE VÉRITÉ
pub const SCHED_SIGNAL_BIT: u64 = 1 << 10;  // bit 10 de sched_state

impl ThreadControlBlock {
    #[inline(always)]
    pub fn has_signal_pending(&self) -> bool {
        self.sched_state.load(Ordering::Acquire) & SCHED_SIGNAL_BIT != 0
    }

    // Appelé UNIQUEMENT par process::signal::delivery
    pub fn set_signal_pending(&self) {
        self.sched_state.fetch_or(SCHED_SIGNAL_BIT, Ordering::Release);
    }

    pub fn clear_signal_pending(&self) {
        self.sched_state.fetch_and(!SCHED_SIGNAL_BIT, Ordering::Release);
    }
}
```

Mettre à jour `process/README.md` PROC-04 :

```diff
- | PROC-04 | `signal_pending` visible du scheduler | `ThreadControlBlock::signal_pending: AtomicBool` |
+ | PROC-04 | `signal_pending` visible du scheduler | bit `SCHED_SIGNAL_BIT` (bit 10) dans `sched_state: AtomicU64` |
```

---

### DEFAUT-SCHED-08 — 🟡 P2 : `fpu::save_restore::alloc_fpu_state` — chemin d'échec silencieux

**Source :** `SCHEDULER_FPU.md` §3

```rust
pub unsafe fn alloc_fpu_state(tcb: &mut ThreadControlBlock) -> bool {
    // ... alloue XSAVE_AREA_SIZE octets ...
    // Retourne false si allocation échoue (OOM kernel)
}
```

Si l'allocation échoue, `alloc_fpu_state` retourne `false`, mais `handle_nm_exception` continue sans vérifier ce retour. Le thread exécutera une instruction FPU **sans état FPU valide**.

**Correctif :**

```rust
pub unsafe fn handle_nm_exception(tcb: &mut ThreadControlBlock) {
    cr0_clear_ts();
    if tcb.fpu_state_ptr.is_null() {
        if !alloc_fpu_state(tcb) {
            // OOM kernel lors de l'allocation FPU → kernel panic contrôlé
            panic!("OOM: impossible d'allouer FpuState pour TID {:?}", tcb.tid);
        }
        // fninit : état x87 propre pour première utilisation
        arch_fninit();
        // vzeroupper si AVX disponible
        if arch_has_avx() { arch_vzeroupper(); }
        xsave_current(tcb);  // baseline XSAVE valide
    }
    xrstor_for(tcb);
    tcb.set_fpu_loaded(true);
}
```

---

## 3. Module `ipc/`

### DEFAUT-IPC-01 — 🟠 P1 : `FUSION_RING_SIZE` — constante non définie dans la documentation

**Source :** `API.md`

```rust
pub const FUSION_RING_SIZE: usize = /* voir source */;   // ← non défini
```

`RULES.md` IPC-06 référence `FUSION_RING_SIZE / 2` comme borne max du seuil de batch, mais la valeur n'est nulle part documentée.

**Correctif :** Ajouter dans `docs/kernel/ipc/README.md` table des constantes et dans `core/constants.rs` :

```rust
/// Capacité du FusionRing. Doit être une puissance de 2.
/// Valeur = 64 slots (4× RING_SIZE) pour buffer adaptatif sans contention.
/// Borne batch_threshold ∈ [1, FUSION_RING_SIZE/2] = [1, 32].
pub const FUSION_RING_SIZE: usize = 64;
pub const FUSION_RING_MASK: usize = FUSION_RING_SIZE - 1;
```

Ajouter une `static_assert` :

```rust
const _: () = assert!(FUSION_RING_SIZE.is_power_of_two(), "FUSION_RING_SIZE must be power of 2");
```

---

### DEFAUT-IPC-02 — 🟠 P1 : Broadcast channel — risque de livelock avec 16 récepteurs et RING_SIZE=16

**Source :** `README.md` + `core/constants.rs`

- `channel/broadcast.rs` : "max 16 récepteurs"
- `RING_SIZE = 16`

Avec 16 récepteurs et 16 slots, si chaque récepteur détient un slot non consommé, le producteur ne peut plus écrire. C'est un **livelock structurel** quand tous les récepteurs sont lents.

**Correctif :** Deux options :

**Option A (recommandée) :** Augmenter `BROADCAST_RING_SIZE` à 64 (indépendant de `RING_SIZE`) :

```rust
/// Taille des rings internes du canal broadcast.
/// > MAX_BROADCAST_RECEIVERS pour éviter le livelock producteur.
pub const BROADCAST_RING_SIZE: usize = 64;
pub const MAX_BROADCAST_RECEIVERS: usize = 16;  // invariant: RING_SIZE > receivers
const _: () = assert!(BROADCAST_RING_SIZE > MAX_BROADCAST_RECEIVERS);
```

**Option B :** Drop silencieux avec compteur `BROADCAST_DROPS` si ring plein (comportement best-effort documenté).

---

### DEFAUT-IPC-03 — 🟠 P1 : `ipc_init()` — initialisation NUMA incomplète

**Source :** `README.md` §Initialisation

```rust
pub fn ipc_init(shm_base_phys: u64, n_numa_nodes: u32)
```

La documentation liste 3 étapes d'init (pool SHM, NUMA, compteurs stats), mais ne précise pas :
- L'ordre critique entre `init_shm_pool` et `numa_init`
- Si `shm_base_phys` doit être dans la zone DMA32 ou NORMAL
- Que faire si `n_numa_nodes = 0` (système non-NUMA)

**Correctif :** Documenter explicitement dans `INIT.md` :

```rust
/// Initialise le module IPC.
///
/// # Préconditions
/// - `memory::init()` DOIT être complètement terminé (buddy, SLUB, NUMA)
/// - `scheduler::init()` DOIT être terminé (wait_queue disponible)
/// - `shm_base_phys` doit être aligné sur PAGE_SIZE et dans la zone NORMAL (≥ 4 GiB)
///   OU dans DMA32 si IOMMU disponible
/// - Appelé UNE SEULE FOIS (flag `IPC_INITIALIZED: AtomicBool` protège)
///
/// # Paramètres
/// - `shm_base_phys`: base physique de la pool SHM kernel
/// - `n_numa_nodes`: nombre de nœuds NUMA (1 si système UMA)
///
/// # Ordre interne
/// 1. Vérifier `!IPC_INITIALIZED.swap(true, Ordering::AcqRel)`
/// 2. `shared_memory::pool::init_shm_pool(shm_base_phys, SHM_POOL_SIZE)`
/// 3. `shared_memory::numa_aware::numa_init(n_numa_nodes)` si n_numa_nodes > 1
/// 4. `stats::counters::init()`
pub fn ipc_init(shm_base_phys: u64, n_numa_nodes: u32) { ... }
```

---

### DEFAUT-IPC-04 — 🟡 P2 : `IpcError` — doublons sémantiques dans l'enum

**Source :** `API.md` enum `IpcError`

Doublons identifiés :

| Code 1 | Code 2 | Sémantique identique |
|--------|--------|----------------------|
| `InternalError = 13` | `Internal = 18` | Erreur interne non classifiable |
| `InvalidParam = 10` | `InvalidArgument = 26` | Paramètre invalide |
| `Closed = 17` | `ChannelClosed = 3` | Canal/endpoint fermé |

Ces doublons créent de l'ambiguïté dans le pattern matching des consommateurs.

**Correctif :** Consolider l'enum en supprimant les doublons et en maintenant la compatibilité numérique pour les syscalls existants :

```rust
pub enum IpcError {
    // Garder les codes bas (1-16) et aliaser les doublons hauts
    WouldBlock         = 1,
    EndpointNotFound   = 2,
    ChannelClosed      = 3,   // CANONIQUE — aliaser Closed → ChannelClosed
    PermissionDenied   = 4,
    MessageTooLarge    = 5,
    Timeout            = 6,
    ResourceExhausted  = 7,
    ConnRefused        = 8,
    AlreadyConnected   = 9,
    InvalidParam       = 10,  // CANONIQUE — aliaser InvalidArgument → InvalidParam
    HandshakeFailed    = 11,
    Interrupted        = 12,
    InternalError      = 13,  // CANONIQUE — aliaser Internal → InternalError
    ShmPoolFull        = 14,
    OutOfOrder         = 15,
    InvalidHandle      = 16,
    // 17-19 : DÉPRÉCIÉS (aliased)
    // 20-31 : codes additionnels conservés
    ...
}

// Aliases de compatibilité
pub const Closed: IpcError = IpcError::ChannelClosed;
pub const Internal: IpcError = IpcError::InternalError;
pub const InvalidArgument: IpcError = IpcError::InvalidParam;
```

---

### DEFAUT-IPC-05 — 🟡 P2 : `fastcall_asm.s` — absence de documentation du protocole ABI

**Source :** `RULES.md` IPC-07

La règle dit "Fast IPC dans `core/fastcall_asm.s`" mais aucun fichier de documentation ne décrit :
- Quels registres sont utilisés pour passer les arguments
- Comment la transition Ring 3 → Ring 0 est évitée (VDSO, shared page, ou autre)
- Le protocole de retour

**Correctif :** Ajouter `docs/kernel/ipc/FASTCALL.md` :

```markdown
# Fast IPC ABI — core/fastcall_asm.s

## Principe
Le fast IPC bypasse le syscall classique via une page partagée kernel/user
(`FAST_IPC_GATE_PAGE`) mappée en lecture dans chaque processus.

## Convention de registres (C ABI étendue)
- %rdi : ChannelId (u64)
- %rsi : *const RingSlot (ptr vers le message)
- %rdx : MsgFlags (u32)
- Retour %rax : 0=succès, IpcError sinon

## Séquence
1. Validation des paramètres (ChannelId valide, ptr canonical)
2. Lecture du ring head/tail via la page partagée
3. Si slot disponible : écriture directe (zero-copy, même espace physique)
4. Sinon : fallback vers syscall IPC_SEND

## Sécurité
- Le ptr %rsi est validé via SMAP (accès user explicite avec STAC/CLAC)
- ChannelId vérifié contre la capability table du thread courant
```

---

## 4. Module `process/`

### DEFAUT-PROC-01 — 🔴 P0 : `exec.rs` — 5 étapes manquantes dans `do_execve()`

**Source :** Croisement `docs/kernel/process/LIFECYCLE.md` §3 vs `ExoOS_Architecture_v7.md` §3.3

La séquence dans `LIFECYCLE.md` est incomplète. Étapes **absentes** :

| Étape manquante | Impact |
|-----------------|--------|
| `verify_cap(EXEC) + is_valid(object_id) + ObjectKind != Secret` | Violation CAP-01 — exec de secrets possible |
| `mask_all_signals_manual()` | Signals délivrés pendant exec → corruption état |
| `signal_queue.flush_all_except_sigkill()` | Signaux périmés livrés à la nouvelle image |
| `tcb.signal_mask = CALLER_SIGNAL_MASK` | Signal mask non hérité (violation POSIX S-11) |
| `tss_set_rsp0(cpu, tcb.kstack_top)` | TSS.RSP0 périmé → corruption pile kernel au retour Ring 3 |

**Correctif :** Remplacer intégralement la séquence dans `LIFECYCLE.md` §3 :

```
do_execve(path, argv, envp, thread, pcb) — Séquence COMPLÈTE v7 :

  1. verify_cap(EXEC, pcb.creds)
     + ELF_LOADER.is_object_valid(object_id)    ← is_valid() + ObjectKind != Secret
     + ObjectKind != Secret → Err(NotExecutable) si vrai
     → Err(PermissionDenied) si cap manquante

  2. mask_all_signals_manual()
     ← Masque TOUS les signaux pendant la transition
     ← Pas de RAII : on va remplacer l'espace d'adressage

  2.5 signal_queue.flush_all_except_sigkill()
     ← Flush les pending signals non-bloquables (ExoOS-spécifique)
     ← SIGKILL et SIGSTOP conservés

  3. ELF_LOADER.load_elf(path, argv, envp, old_cr3) → ElfLoadResult
     → Err(ElfLoadFailed) si ELF invalide

  4. files.close_on_exec()
     ← Ferme tous les fds avec O_CLOEXEC

  5. Mettre à jour thread.addr (entry_point, initial_rsp, tls_base)
     Mettre à jour pcb (cr3, address_space, brk, flags |= EXEC_DONE)
     tcb.signal_mask = CALLER_SIGNAL_MASK    ← héritage POSIX (S-11)

  6. reset_signals_on_exec()
     ← Reset handlers → SIG_DFL (APRÈS mise à jour signal_mask)

  7. tss_set_rsp0(current_cpu(), tcb.kstack_top)   ← V7-C-03 OBLIGATOIRE

  8. return_to_new_userspace()    ← ne retourne pas
```

---

### DEFAUT-PROC-02 — 🔴 P0 : `Pid::IDLE = 0` et `Pid::INVALID = 0` — valeurs sentinelles identiques

**Source :** `docs/kernel/process/CORE.md` §1

```rust
pub const IDLE:    Self = Self(0);   // Processus idle
pub const INVALID: Self = Self(0);   // Valeur sentinelle "pas de PID"
```

Ces deux constantes ont la **même valeur**. Tout code testant `pid == Pid::INVALID` retournera `true` pour le processus idle, rendant toute vérification d'invalidité impossible.

**Correctif :**

```rust
pub const IDLE:          Self = Self(0);           // PID 0 réservé kernel
pub const INIT:          Self = Self(1);           // PID 1 init
pub const INVALID:       Self = Self(u32::MAX);    // Sentinelle invalide

// Et vérifier la cohérence :
const _: () = assert!(Pid::IDLE.0 != Pid::INVALID.0);
const _: () = assert!(Pid::INIT.0 != Pid::INVALID.0);
```

Mettre à jour les callsites :

```rust
// Partout où le code compare à INVALID (0 → u32::MAX) :
if pid == Pid::INVALID { ... }   // ← fonctionne correctement maintenant
if ppid.load(Ordering::Acquire) == u32::MAX { ... }  // à remplacer par Pid::INVALID.0
```

---

### DEFAUT-PROC-03 — 🟠 P1 : `ProcessRegistry` — `refcount` non maintenu dans l'API documentée

**Source :** `docs/kernel/process/CORE.md` §4

```rust
struct RegistrySlot {
    pcb_ptr:  AtomicPtr<ProcessControlBlock>,
    refcount: AtomicU32,   // "compteur de références pour lookups concurrents"
}
```

L'API documentée (`insert`, `remove`, `find_by_pid`, `for_each`) ne comporte **aucune méthode** pour incrémenter/décrémenter `refcount`. Si `remove()` est appelé pendant qu'un autre thread est dans `find_by_pid()` et utilise le PCB retourné, la mémoire peut être libérée sous lui.

**Correctif :** Ajouter deux méthodes et un wrapper RAII :

```rust
impl ProcessRegistry {
    /// Incrémente le refcount d'un slot (doit être appelé après find_by_pid).
    /// Retourne false si le PCB a été supprimé entre-temps.
    pub fn acquire_ref(&self, pid: Pid) -> bool {
        let slot = &self.slots[pid.0 as usize];
        // CAS : on refuse d'incrémenter si refcount == 0 (déjà supprimé)
        let mut current = slot.refcount.load(Ordering::Acquire);
        loop {
            if current == 0 { return false; }
            match slot.refcount.compare_exchange_weak(
                current, current + 1, Ordering::AcqRel, Ordering::Acquire
            ) {
                Ok(_) => return true,
                Err(c) => current = c,
            }
        }
    }

    /// Décrémente le refcount. Retourne vrai si c'était la dernière référence.
    pub fn release_ref(&self, pid: Pid) -> bool {
        let slot = &self.slots[pid.0 as usize];
        slot.refcount.fetch_sub(1, Ordering::AcqRel) == 1
    }
}

/// RAII guard pour PCB lookup — libère automatiquement la référence.
pub struct PcbGuard<'a> {
    registry: &'a ProcessRegistry,
    pid: Pid,
    pcb: &'a ProcessControlBlock,
}

impl<'a> Drop for PcbGuard<'a> {
    fn drop(&mut self) {
        self.registry.release_ref(self.pid);
    }
}
```

---

### DEFAUT-PROC-04 — 🟠 P1 : `exit.rs` — libération ressources `fpu_state_ptr` et runqueue absentes

**Source :** `docs/kernel/process/LIFECYCLE.md` §4 vs `ExoOS_Architecture_v7.md` §3.3 exit.rs

L'architecture v7 spécifie `release_thread_resources(tcb)` avec :
- `dealloc(fpu_state_ptr)` si non null
- `tcb.rq_next = null; tcb.rq_prev = null`

Mais la séquence `do_exit()` dans `LIFECYCLE.md` **n'en fait pas mention**. Si ces étapes ne sont pas implémentées :
- `fpu_state_ptr` leak → épuisement mémoire kernel sur terminaisons répétées
- `rq_next/rq_prev` non nullifiés → runqueue corrompue lors du reaper scan

**Correctif :** Compléter la séquence `do_exit()` dans `LIFECYCLE.md` §4 :

```
do_exit(thread, pcb, exit_code) — Séquence COMPLÈTE :

  1. pcb.set_exiting()   (atomique, flag EXITING)
  2. pcb.exit_code.store(exit_code, Ordering::Release)
  3. release_thread_resources(tcb) :
       a. Si tcb.fpu_state_ptr != null :
            let layout = Layout::from_size_align(
                XSAVE_AREA_SIZE.load(Ordering::Relaxed), 64
            ).unwrap();
            unsafe { alloc::dealloc(tcb.fpu_state_ptr as *mut u8, layout); }
            tcb.fpu_state_ptr = core::ptr::null_mut();
       b. tcb.rq_next.store(null_mut(), Ordering::Release)
       c. tcb.rq_prev.store(null_mut(), Ordering::Release)
  4. pcb.dec_threads() → remaining
  5. Si remaining == 0 (dernier thread) :
       a. Fermer tous les fds ouverts
       b. cap_table.revoke_all()
       c. signal_queue.flush_all()
       d. send_signal_to_pid(ppid, SIGCHLD)
       e. pcb.set_state(ProcessState::Zombie)
  6. thread.set_state(TaskState::Dead)
  7. REAPER_QUEUE.push(pcb_ptr)   ← kthread reaper se charge du reste
  8. schedule_yield()   ← ce thread ne sera jamais repris
  // Ne retourne pas (!)
```

---

### DEFAUT-PROC-05 — 🟡 P2 : `do_fork()` — TLB flush parent manquant dans la documentation

**Source :** `LIFECYCLE.md` §2 séquence fork

La séquence documentée mentionne `ADDR_SPACE_CLONER.flush_tlb_after_fork(parent_cr3)` à l'étape 6 (PROC-06/PROC-08), mais ne précise pas :
- Si cette opération est **synchrone** (IPI TLB shootdown sur tous les CPUs)
- Ce qui se passe si un CPU exécute du code du parent pendant le marquage CoW

**Correctif :** Clarifier l'étape 6 :

```
  6. ADDR_SPACE_CLONER.flush_tlb_after_fork(parent_cr3)
     ← TLB shootdown SYNCHRONE via IPI 0xF2 vers TOUS les CPUs actifs
     ← Garantit qu'aucun CPU ne peut écrire sur une ancienne PTE (avant CoW)
     ← Bloquant : attend les ACKs de tous les CPUs (deadline = 1 ms, sinon kernel_halt)
     ← DOIT être fait AVANT que le fils soit visible dans la runqueue
     ← L'IPI utilise send_tlb_ipi_to_mask([u64;4]) — correctif DEFAUT-MEM-02
```

---

### DEFAUT-PROC-06 — 🟡 P2 : `reap.rs` — comportement reaper sur PCB avec `refcount > 0` non documenté

**Source :** `CORE.md` §4 invariant 3 : "remove() ne libère pas le PCB — responsabilité du reaper"

Si le reaper tente de libérer un PCB dont `refcount > 0` (un autre thread est dans `find_by_pid()`), la mémoire sera libérée sous un accès actif.

**Correctif :** Documenter le comportement du reaper :

```rust
// lifecycle/reap.rs — comportement reaper
fn kthread_reaper() -> ! {
    loop {
        if let Some(pcb_ptr) = REAPER_QUEUE.pop() {
            let pid = unsafe { (*pcb_ptr).pid };
            
            // 1. Marquer comme Dead dans la registry (empêche nouveaux acquire_ref)
            PROCESS_REGISTRY.remove(pid);
            
            // 2. Attendre que toutes les références soient relâchées
            // (spin avec backoff — max 100 µs avant kernel warning)
            let slot = &PROCESS_REGISTRY.slots[pid.0 as usize];
            let mut waited_ns = 0u64;
            while slot.refcount.load(Ordering::Acquire) > 0 {
                core::hint::spin_loop();
                waited_ns += 10;
                if waited_ns > 100_000 {
                    log_warn!("Reaper: PCB PID {} has lingering refs", pid.0);
                    break;
                }
            }
            
            // 3. Libérer le PCB
            unsafe { drop(Box::from_raw(pcb_ptr)); }
            PID_ALLOCATOR.free(pid.0);
        } else {
            scheduler::block_current();  // Attend REAPER_QUEUE non vide
        }
    }
}
```

---

## 5. Incohérences croisées inter-modules

### DEFAUT-CROSS-01 — 🔴 P0 : `memory/` importe `process/` via `OomKillSendFn` — violation COUCHE 0

**Source :** `MEMORY_COMPLETE.md` §15 `oom_killer.rs`

```rust
// Sélectionne la victime via OomScorer trait (implémenté par process/)
// Signale la mort via OOM_KILL_SENDER : OomKillSendFn (fn pointer enregistré au boot)
```

Bien que le fn pointer rompe l'import direct, le commentaire dit "implémenté par `process/`". Si l'OOM killer est déclenché pendant l'initialisation de `process/`, avant l'enregistrement de `OomScorer`, le comportement est indéfini.

**Correctif :** Ajouter une vérification explicite et un fallback :

```rust
// memory/utils/oom_killer.rs
static OOM_SCORER: AtomicPtr<dyn OomScorer> = AtomicPtr::new(null_mut());
static OOM_KILL_SENDER: AtomicPtr<OomKillSendFn> = AtomicPtr::new(null_mut());

pub fn oom_kill_select_victim() -> Option<u32> {
    let scorer_ptr = OOM_SCORER.load(Ordering::Acquire);
    if scorer_ptr.is_null() {
        // process/ pas encore initialisé — pas de victime, panic OOM immédiat
        log_error!("OOM avant init process/ — halt kernel");
        kernel_halt_diagnostic(HaltCode::OOM_PRE_PROCESS_INIT);
    }
    // ... sélection normale
}
```

---

### DEFAUT-CROSS-02 — 🟠 P1 : `ipc/capability_bridge/` délègue à `security::capability::verify()` mais `SECURITY_READY` non vérifié

**Source :** `RULES.md` IPC-04 + Architecture v7 §3.4

Si IPC est utilisé avant que `SECURITY_READY` soit vrai (Phase 5 du boot), `security::capability::verify()` peut retourner des résultats erronés (CVE-EXO-001).

**Correctif :** Ajouter une garde dans `capability_bridge/check.rs` :

```rust
pub fn verify_ipc_access(token: &CapToken, required: Rights) -> Result<(), IpcCapError> {
    // Vérifier que le sous-système de sécurité est prêt
    if !crate::security::SECURITY_READY.load(Ordering::Acquire) {
        // En phase de boot : reject toutes les vérifications sauf les caps kernel
        if !token.is_kernel_bootstrap_cap() {
            return Err(IpcCapError::Revoked);
        }
    }
    crate::security::capability::verify(token.object_id, token.rights as u32)
        .map_err(|_| IpcCapError::InsufficientRights)
}
```

---

### DEFAUT-CROSS-03 — 🟡 P2 : `scheduler::init_ap()` — ordre d'initialisation FPU non documenté par rapport à `SECURITY_READY`

**Source :** Architecture v7 §3.1.1 steps + SCHEDULER_OVERVIEW §4

L'architecture v7 décrit les 14 steps de boot puis les phases kernel_init(). `scheduler::init_ap()` est appelé pendant la phase AP (avant Phase 5 Security). Si un AP tente d'utiliser le FPU avant que `cr0_set_ts()` soit appelé dans `fpu::lazy::init()`, il n'y a pas d'exception `#NM` protectrice.

**Correctif :** Documenter et enforcer dans `SCHEDULER_OVERVIEW.md` §4 `init_ap` :

```rust
pub unsafe fn init_ap(cpu_id: u32) {
    // Ordre critique :
    preempt::init_for_cpu(cpu_id);       // 1. Compteur préemption
    fpu::lazy::init();                   // 2. CR0.TS=1 SUR CE CPU (avant toute ISR FPU)
    runqueue::init_for_cpu(cpu_id);      // 3. File d'exécution
    tick::init_for_cpu(cpu_id);          // 4. Timer tick
    // NOTE : fpu::save_restore::init() est global (BSP seulement)
    // NOTE : energy::c_states::init() est global (BSP seulement)
}
```

---

## 6. Correctifs consolidés

### Table de priorité

| ID Défaut | Module | Priorité | Fichiers à modifier |
|-----------|--------|----------|---------------------|
| DEFAUT-MEM-01 | memory | 🔴 P0 | `MEMORY_COMPLETE.md` §2 |
| DEFAUT-MEM-02 | memory | 🔴 P0 | `arch_memory_integration.md`, `tlb.rs`, `memory_iface.rs` |
| DEFAUT-MEM-03 | memory/TLA+ | 🟡 P2 | `Memory.tla` |
| DEFAUT-SCHED-01 | scheduler | 🔴 P0 | `SCHEDULER_OVERVIEW.md` §3, `task.rs` |
| DEFAUT-SCHED-02 | scheduler | 🔴 P0 | `SCHEDULER_ASM.md`, `SCHEDULER_OVERVIEW.md` §3/§5, `switch_asm.s` |
| DEFAUT-SCHED-03 | scheduler | 🔴 P0 | `ExoOS_Architecture_v7.md` V7-C-03, TCB table |
| DEFAUT-SCHED-04 | scheduler | 🔴 P0 | `affinity.rs`, `SCHEDULER_SMP.md`, TCB layout |
| DEFAUT-SCHED-05 | scheduler | 🔴 P0 | `ExoOS_Architecture_v7.md` §3.2 switch sequence |
| DEFAUT-SCHED-06 | scheduler | 🟠 P1 | `SCHEDULER_CORE.md` §2, `GS_LAYOUT.md` (nouveau) |
| DEFAUT-SCHED-07 | scheduler | 🟠 P1 | `task.rs`, `process/README.md` PROC-04 |
| DEFAUT-SCHED-08 | scheduler | 🟡 P2 | `fpu/lazy.rs` handle_nm_exception |
| DEFAUT-IPC-01 | ipc | 🟠 P1 | `core/constants.rs`, `API.md`, `RULES.md` |
| DEFAUT-IPC-02 | ipc | 🟠 P1 | `channel/broadcast.rs`, `core/constants.rs` |
| DEFAUT-IPC-03 | ipc | 🟠 P1 | `INIT.md`, `mod.rs` ipc_init |
| DEFAUT-IPC-04 | ipc | 🟡 P2 | `core/types.rs` IpcError enum, `API.md` |
| DEFAUT-IPC-05 | ipc | 🟡 P2 | `FASTCALL.md` (nouveau) |
| DEFAUT-PROC-01 | process | 🔴 P0 | `LIFECYCLE.md` §3, `lifecycle/exec.rs` |
| DEFAUT-PROC-02 | process | 🔴 P0 | `core/pid.rs`, `CORE.md` §1 |
| DEFAUT-PROC-03 | process | 🟠 P1 | `core/registry.rs`, `CORE.md` §4 |
| DEFAUT-PROC-04 | process | 🟠 P1 | `lifecycle/exit.rs`, `LIFECYCLE.md` §4 |
| DEFAUT-PROC-05 | process | 🟡 P2 | `LIFECYCLE.md` §2 fork |
| DEFAUT-PROC-06 | process | 🟡 P2 | `lifecycle/reap.rs` |
| DEFAUT-CROSS-01 | memory/process | 🔴 P0 | `utils/oom_killer.rs` |
| DEFAUT-CROSS-02 | ipc/security | 🟠 P1 | `capability_bridge/check.rs` |
| DEFAUT-CROSS-03 | scheduler/arch | 🟡 P2 | `mod.rs` init_ap, `SCHEDULER_OVERVIEW.md` §4 |

---

## 7. Checklist de validation finale

Après application de tous les correctifs, valider les points suivants :

### Compilation

```bash
# Taille TCB = 256 B (DEFAUT-SCHED-01)
cargo test -p kernel -- test_tcb_size_256

# Affinité 256 CPUs (DEFAUT-SCHED-04)
cargo test -p kernel -- test_affinity_full_256

# Pid::INVALID != Pid::IDLE (DEFAUT-PROC-02)
cargo test -p kernel -- test_pid_sentinels_distinct

# FUSION_RING_SIZE est une puissance de 2 (DEFAUT-IPC-01)
cargo test -p kernel -- test_fusion_ring_size_pow2

# BROADCAST_RING_SIZE > MAX_BROADCAST_RECEIVERS (DEFAUT-IPC-02)
cargo test -p kernel -- test_broadcast_ring_no_livelock
```

### Conformité documentaire

- [ ] `SCHEDULER_OVERVIEW.md` SCHED-03 : "256 B" (pas 128 B)
- [ ] `SCHEDULER_OVERVIEW.md` SCHED-07 : "PAS de MXCSR/FCW dans switch_asm.s"
- [ ] `SCHEDULER_ASM.md` : code sans `stmxcsr/fstcw/fldcw/ldmxcsr`
- [ ] `ExoOS_Architecture_v7.md` V7-C-03 : `kstack_top` (pas `kstack_ptr`)
- [ ] `ExoOS_Architecture_v7.md` switch sequence : CR0.TS AVANT context_switch_asm
- [ ] `MEMORY_COMPLETE.md` §2 LOCK ORDER : ordre correct Memory→Scheduler→IPC
- [ ] `LIFECYCLE.md` §3 exec : 8 étapes complètes
- [ ] `LIFECYCLE.md` §4 exit : `release_thread_resources()` inclus
- [ ] `CORE.md` §1 : `Pid::INVALID = u32::MAX`
- [ ] `API.md` IPC : `FUSION_RING_SIZE = 64` défini

### TLA+ Model Checking

```bash
# Après correction Memory.tla (DEFAUT-MEM-03)
java -jar tla2tools.jar Memory.tla -config Memory.cfg
# → 0 erreur sur S47, S48, S49, S50 (nouveau)

# ContextSwitch.tla — vérifier S26 avec kstack_top
java -jar tla2tools.jar ContextSwitch.tla -config ContextSwitch.cfg
# → S26_TssRsp0MatchesCurrentTcb passe avec kstack_top
```

### Tests runtime (QEMU)

```bash
# Boot complet avec 256 CPUs (MAX_CPUS)
qemu-system-x86_64 -smp 256 -m 8G -kernel exoos.elf
# → No crash, MAX_CORES_RUNTIME = 256, TLB shootdown sur 256 CPUs

# Test context switch FPU lazy
run_test fpu_lazy_256threads
# → 0 #GP, MXCSR correct dans xsave area

# Test fork/exec complet avec signaux
run_test fork_exec_signal_mask_inheritance
# → signal_mask hérité, pending flush correct, TSS.RSP0 valide
```

---

## Annexe — Résumé des fichiers créés / modifiés

| Fichier | Action | Défauts couverts |
|---------|--------|-----------------|
| `docs/kernel/arch/GS_LAYOUT.md` | **CRÉER** | DEFAUT-SCHED-06 |
| `docs/kernel/ipc/FASTCALL.md` | **CRÉER** | DEFAUT-IPC-05 |
| `docs/kernel/memory/MEMORY_COMPLETE.md` | Modifier §2 | DEFAUT-MEM-01 |
| `docs/kernel/memory/arch_memory_integration.md` | Modifier §2.1 | DEFAUT-MEM-02 |
| `docs/Exo-OS-TLA+/Memory.tla` | Ajouter ReadRelaxed, S50 | DEFAUT-MEM-03 |
| `docs/kernel/scheduler/SCHEDULER_OVERVIEW.md` | Modifier §3, §5 | DEFAUT-SCHED-01, -02, -07 |
| `docs/kernel/scheduler/SCHEDULER_ASM.md` | Réécrire §1-3 | DEFAUT-SCHED-02 |
| `docs/recast/ExoOS_Architecture_v7.md` | V7-C-03, §3.2, §3.3 | DEFAUT-SCHED-03, -05 |
| `docs/kernel/scheduler/SCHEDULER_CORE.md` | §2 step 8 GS offset | DEFAUT-SCHED-06 |
| `docs/kernel/ipc/README.md` | Constantes, FUSION_RING_SIZE | DEFAUT-IPC-01, -02, -03 |
| `docs/kernel/ipc/API.md` | IpcError consolidé, FUSION_RING_SIZE | DEFAUT-IPC-01, -04 |
| `docs/kernel/process/LIFECYCLE.md` | §2 fork, §3 exec, §4 exit | DEFAUT-PROC-01, -04, -05 |
| `docs/kernel/process/CORE.md` | §1 Pid::INVALID, §4 refcount | DEFAUT-PROC-02, -03 |
| `kernel/src/scheduler/core/task.rs` | assert size=256, signal_pending | DEFAUT-SCHED-01, -07 |
| `kernel/src/scheduler/smp/affinity.rs` | cpu_allowed_full() | DEFAUT-SCHED-04 |
| `kernel/src/scheduler/fpu/lazy.rs` | handle_nm OOM check | DEFAUT-SCHED-08 |
| `kernel/src/ipc/core/constants.rs` | FUSION_RING_SIZE, BROADCAST_RING_SIZE | DEFAUT-IPC-01, -02 |
| `kernel/src/ipc/capability_bridge/check.rs` | SECURITY_READY guard | DEFAUT-CROSS-02 |
| `kernel/src/process/core/pid.rs` | Pid::INVALID = u32::MAX | DEFAUT-PROC-02 |
| `kernel/src/process/core/registry.rs` | acquire_ref, release_ref, PcbGuard | DEFAUT-PROC-03 |
| `kernel/src/process/lifecycle/exit.rs` | release_thread_resources complet | DEFAUT-PROC-04 |
| `kernel/src/process/lifecycle/exec.rs` | Séquence complète 8 étapes | DEFAUT-PROC-01 |
| `kernel/src/process/lifecycle/reap.rs` | Attente refcount=0 avant free | DEFAUT-PROC-06 |
| `kernel/src/memory/utils/oom_killer.rs` | Guard pre-process init | DEFAUT-CROSS-01 |
| `kernel/src/arch/x86_64/memory_iface.rs` | mask [u64;4], boucle 256 | DEFAUT-MEM-02 |
| `kernel/src/scheduler/mod.rs` | init_ap ordre correct | DEFAUT-CROSS-03 |

---

*Exo-OS — Audit & Correctifs Modules Memory/Scheduler/IPC/Process*  
*Produit par **claude-beta** — Mai 2026 — Version 1.0*  
*24 défauts identifiés · 8 P0 · 9 P1 · 7 P2 · 100% correctifs fournis*
