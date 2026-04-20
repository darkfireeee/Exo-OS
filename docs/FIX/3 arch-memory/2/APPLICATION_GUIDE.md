# GUIDE D'APPLICATION DES CORRECTIFS — FIX 3.1 → FIX 3.2

## Ordre d'application recommandé (du plus critique au moins critique)

---

### ÉTAPE 1 — FIX-SWITCH-ASM-01 (P0, 3 lignes)
**Fichier** : `kernel/src/scheduler/asm/switch_asm.s`
**Patch** : `FIX-VRUNTIME-01_AND_FIX-SWITCH-ASM-01.patch` (section switch_asm)

Supprimer dans `switch_to_new_thread` :
```asm
# SUPPRIMER CES 3 LIGNES :
    subq    $16, %rsp
    stmxcsr 0(%rsp)
    fstcw   8(%rsp)
```

Vérifier : `git diff --stat` doit montrer -3 lignes dans switch_asm.s.
Test : créer un kthread, le laisser s'exécuter, vérifier que le thread parent reprend avec des registres corrects.

---

### ÉTAPE 2 — FIX-VRUNTIME-01 (P0, 1 ligne)
**Fichier** : `kernel/src/scheduler/policies/cfs.rs`
**Patch** : `FIX-VRUNTIME-01_AND_FIX-SWITCH-ASM-01.patch` (section cfs)

```rust
// AVANT :
if woken_vr + CFS_WAKEUP_PREEMPT_NS < running_vr {

// APRÈS :
let woken_vr_bumped = woken_vr.wrapping_add(CFS_WAKEUP_PREEMPT_NS);
if woken_vr_bumped < running_vr {
```

Vérifier : `cargo test --release` dans le module scheduler.

---

### ÉTAPE 3 — FIX-KPTI-01 (P0, remplacer kpti.rs)
**Fichier** : `kernel/src/arch/x86_64/spectre/kpti.rs`
**Patch** : `FIX-KPTI-01_kpti.rs.patch`

Remplacer les deux globaux et leurs usages par le tableau `CR3_PER_CPU[MAX_CPUS]`.
Ajouter l'appel `set_current_cr3(next.cr3_phys, user_cr3)` dans `context_switch()`
(voir commentaire dans le patch).

Vérifier : SMP boot sur QEMU avec ≥4 vCPUs, test de switch intensif.

---

### ÉTAPE 4 — FIX-CET-01 (P0, conditionnel à l'activation CET)
**Fichiers** :
- `kernel/src/arch/x86_64/cpu/msr.rs` : ajouter `MSR_IA32_PL0_SSP`
- `kernel/src/arch/x86_64/cpu/features.rs` : ajouter `has_cet_ss()`
- `kernel/src/scheduler/core/task.rs` : ajouter `pl0_ssp()` / `set_pl0_ssp()`
- `kernel/src/scheduler/core/switch.rs` : sauvegarder/restaurer MSR
**Patch** : `FIX-CET-01_switch_ssp.patch`

Note : sans CET activé, le `if CPU_FEATURES.has_cet_ss()` court-circuite tout.
Ce fix est non-régressif — si CET est absent du CPU, comportement identique à avant.

Vérifier : si CET est disponible sur le hardware cible, activer dans features
et exécuter un test de context switch avec CET_EN.

---

### ÉTAPE 5 — FIX-SLABCACHE-01 (P1)
**Fichier** : `kernel/src/memory/physical/allocator/slab.rs`
**Patch** : `FIX-SLABCACHE-RQ-CANARY.patch` (section SlabCache)

Ajouter `#[repr(C, align(64))]` sur `SlabCache` et le champ `_cache_line_separator`.

Vérifier : sizeof(SlabCache) via `assert_eq!(mem::size_of::<SlabCache>(), ...)`.

---

### ÉTAPE 6 — FIX-RQ-ALIGN-01 (P1)
**Fichier** : `kernel/src/scheduler/core/runqueue.rs`
**Patch** : `FIX-SLABCACHE-RQ-CANARY.patch` (section PerCpuRunQueue)

Ajouter `#[repr(C, align(64))]` sur `PerCpuRunQueue` + assertion compile-time.

Vérifier : `assert!(mem::align_of::<PerCpuRunQueue>() >= 64)`.

---

### ÉTAPE 7 — FIX-CANARY-01 (P2)
**Fichier** : `kernel/src/memory/integrity/canary.rs`
**Patch** : `FIX-SLABCACHE-RQ-CANARY.patch` (section canary)

Remplacer `const MAX_CPUS: usize = 256;` par `use crate::memory::core::constants::MAX_CPUS;`.
Envisager de remplacer le transmute par `[const { CanarySlot::uninit() }; MAX_CPUS]`.

---

## Tableau de risque résiduel après application de tous les correctifs

| Composant | Risque avant | Risque après |
|-----------|-------------|--------------|
| CET Shadow Stack | Crash #CP dès activation | Géré correctement |
| KPTI SMP | CR3 race cross-CPU → page fault | Per-CPU, isolé |
| CFS wakeup preemption | Panic debug / inversions release | Wrapping safe |
| switch_to_new_thread | Stack corrompu au retour thread parent | Layout uniforme |
| SlabCache SMP | False sharing alloc/stats | Cache lines séparées |
| PerCpuRunQueue | False sharing CPUs adjacents | Aligné 64B |
| Canary MAX_CPUS | UB silencieux si constante diverge | Import canonique |

## Commits suggérés

```
fix(sched/asm): remove spurious MXCSR/FCW save in switch_to_new_thread [FIX-SWITCH-ASM-01]
fix(sched/cfs): use wrapping_add in should_preempt_on_wakeup [FIX-VRUNTIME-01]
fix(arch/kpti): make CR3 storage per-CPU on SMP systems [FIX-KPTI-01]
fix(sched/switch): save/restore MSR_IA32_PL0_SSP when CET-SS active [FIX-CET-01]
fix(memory/slab): add align(64) to SlabCache to prevent false sharing [FIX-SLABCACHE-01]
fix(sched/runqueue): add repr(align(64)) to PerCpuRunQueue [FIX-RQ-ALIGN-01]
fix(memory/canary): import MAX_CPUS from canonical constants module [FIX-CANARY-01]
```
