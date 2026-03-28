# Divergence TCB : `ExoOS_Architecture_v7.md` vs Implémentation GI-01

**Date** : 28 mars 2026  
**Fichiers concernés** :
- Spec originale : `docs/recast/ExoOS_Architecture_v7.md` §3.2
- Source de vérité implémentation : `docs/recast/GI-01_Types_TCB_SSR.md` §7
- Implémentation : `kernel/src/scheduler/core/task.rs`

---

## 1. Résumé

Le layout TCB décrit dans `ExoOS_Architecture_v7.md` **diffère volontairement** du TCB
implémenté dans `task.rs`. Cette divergence n'est **pas un bug** : elle est la
conséquence directe de la correction **CORR-01** appliquée dans le guide
d'implémentation GI-01, qui supersède le §3.2 de `Architecture_v7.md` sur ce point précis.

> **Règle** : Pour le layout TCB, `GI-01_Types_TCB_SSR.md` est la source de vérité.
> `Architecture_v7.md` reste valide pour tout le reste (séquences, règles Ring 1, IPC, ExoFS...).

---

## 2. Tableau de divergence complet

| Offset | `Architecture_v7.md` (spec originale) | `task.rs` GI-01 (implémenté) | Raison |
|--------|--------------------------------------|------------------------------|--------|
| **[0]** | `cap_table_ptr: u64` — pointeur CapTable | `tid: u64` — Thread ID | CORR-01 : `cap_table_ptr` sort du TCB (Ring 0 n'y accède pas sur hot path) |
| **[8]** | `kstack_ptr: u64` ✅ | `kstack_ptr: u64` ✅ | Identique — offset hardcodé `switch_asm.s` |
| **[16]** | `tid: u64` | `priority: Priority` + `policy: SchedPolicy` | CORR-01 : tid déplacé en [0], prio/policy en [16-17] pour réduire la CL1 |
| **[24]** | `sched_state: AtomicU8` — 4 états seulement | `sched_state: AtomicU64` — état + 7 flags encodés | CORR-01 : encodage compact unifié (état \| signal \| KTHREAD \| FPU \| RESCHED...) |
| **[32]** | `fs_base: u64` — TLS | `vruntime: AtomicU64` — CFS vruntime | CORR-01 : champs scheduler hot en CL1, TLS déplacé en CL2 |
| **[40]** | `user_gs_base: u64` — GS userspace | `deadline_abs: AtomicU64` — EDF deadline | Même raison |
| **[48]** | `pkrs: u32` + `_pad: u32` | `cpu_affinity: AtomicU64` — bitmask affinité | Même raison |
| **[56]** | `cr3_phys: u64` ✅ | `cr3_phys: u64` ✅ | Identique — offset hardcodé `switch_asm.s` |
| **[64..175]** | 14 GPRs (`rax..r14`) embarqués dans le TCB | `cpu_id` + `fs_base` + `gs_base` + `pkrs` + `pid` + `signal_mask` + EDF | CORR-01 : GPRs **ne sont pas** dans le TCB — ils s'empilent sur la kstack lors du context switch |
| **[176..231]** | `r15`, pad, `rip`, `rsp_user`, `rflags`, `cs/ss`, `cr2` | `dl_runtime` + `dl_period` + zone cold (`run_time_acc`, `switch_count`, réserve) | Même raison |
| **[232]** | `fpu_state_ptr: u64` ✅ | `fpu_state_ptr: u64` ✅ | Identique — offset hardcodé ExoPhoenix |
| **[240]** | `rq_next: u64` ✅ | `rq_next: u64` ✅ | Identique |
| **[248]** | `rq_prev: u64` ✅ | `rq_prev: u64` ✅ | Identique |

**Offsets partagés et verrouillés** : `[8]`, `[56]`, `[232]`, `[240]`, `[248]`  
**Offsets divergents** : `[0]`, `[16..55]`, `[64..231]`

---

## 3. Justification de CORR-01

### 3.1 Problème de la spec Architecture_v7

La spec v7 plaçait les **14 GPRs (rax..r15) dans le TCB** aux offsets [64..183].
C'est une erreur de conception pour trois raisons :

**a) Redondance avec la kstack**

Le mécanisme x86_64 d'entrée en Ring 0 (interruption, syscall) pousse automatiquement
`rip, cs, rflags, rsp, ss` sur la pile kernel. Les GPRs callee-saved (rbx, rbp, r12..r15)
sont poussés par prólogo ABI. Stocker une copie dans le TCB en plus est redondant.

**b) Pollution du cache**

Avec les GPRs en [64..183], `pick_next_task()` (hot path scheduler) doit charger 3 cache-lines
pour accéder à `cr3_phys@[56]` et aux champs scheduler. Avec CORR-01, `pick_next_task()`
opère **entièrement en CL1 [0..63]** : `tid`, `kstack_ptr`, `priority`, `policy`,
`sched_state`, `vruntime`, `deadline_abs`, `cpu_affinity`, `cr3_phys`.

**c) sched_state trop étroit**

`AtomicU8` ne peut encoder que l'état de base (4 valeurs). Les flags comme
`KTHREAD`, `FPU_LOADED`, `NEED_RESCHED`, `EXITING`, `IN_RECLAIM` nécessitaient
des atomiques séparés (`AtomicBool signal_pending`, `AtomicU32 flags`, `AtomicU8 state`),
créant des problèmes de cohérence et d'atomicité composée.
`AtomicU64` à l'offset [24] unifie tout en un seul mot atomiqu.

### 3.2 Solution GI-01 (CORR-01)

```
CL1 [0..64]  — HOT PATH exclusif scheduler
  [0]  tid            — identifiant (accès fréquent IPC, syslog)
  [8]  kstack_ptr     — switch_asm.s (HARDCODÉ, inchangé)
  [16] priority       — pick_next_task() comparaison RT vs CFS
  [17] policy         — dispatch vers la bonne politique
  [24] sched_state    — AtomicU64 : état + 7 flags (lecture unique = 1 instruction)
  [32] vruntime       — CFS update à chaque tick
  [40] deadline_abs   — EDF comparaison à chaque tick
  [48] cpu_affinity   — SMP migration check
  [56] cr3_phys       — switch_asm.s (HARDCODÉ, inchangé)

CL2 [64..128] — WARM context switch (chargé au switch uniquement)
  fs_base, gs_base, pkrs, pid, signal_mask, dl_runtime, dl_period

CL3+4 [128..256] — COLD + HARDCODED
  compteurs, réserve, fpu_state_ptr[232], rq_next[240], rq_prev[248]
```

Les GPRs sont gérés **exclusivement sur la kstack** via `switch_asm.s` :
```asm
; push des callee-saved sur la kstack courante
push rbp; push rbx; push r12; push r13; push r14; push r15
mov [rdi + 8], rsp    ; kstack_ptr ← RSP courante (offset [8])
mov rsp, [rsi + 8]    ; RSP ← kstack_ptr du thread suivant
pop r15; pop r14; pop r13; pop r12; pop rbx; pop rbp
ret
```

---

## 4. Champs présents dans Architecture_v7 mais absents du TCB canonique

Ces champs existent toujours dans le code kernel, mais **hors du TCB** :

| Champ (Architecture_v7) | Emplacement réel GI-01 |
|-------------------------|----------------------|
| GPRs `rax..r15` | Kstack kernel (poussés par `switch_asm.s` / interruption) |
| `rip`, `rsp_user`, `rflags`, `cs/ss` | Frame d'interruption sur la kstack |
| `cr2` | Lu depuis `CR2` directement au moment du `#PF` |
| `user_gs_base` | Sauvegardé/restauré par `SWAPGS` + `WRMSR` (pas dans TCB) |
| `cap_table_ptr` | Dans `ProcessControlBlock` (PCB) — partagé entre threads du même process |

---

## 5. Ce qui reste valide dans Architecture_v7 §3.2

Malgré la divergence du layout, les éléments suivants de `Architecture_v7.md` sont
**entièrement corrects et respectés** dans l'implémentation :

- ✅ **Taille totale** : 256 octets, `align(64)`
- ✅ **`kstack_ptr` à l'offset [8]** — hardcodé dans `switch_asm.s`
- ✅ **`cr3_phys` à l'offset [56]** — hardcodé dans `switch_asm.s`
- ✅ **`fpu_state_ptr` à l'offset [232]** — lu par ExoPhoenix/Kernel B
- ✅ **`rq_next/rq_prev` aux offsets [240/248]** — RunQueue intrusive
- ✅ **Lazy FPU** (V7-C-02) : `switch_asm.s` NE touche pas MXCSR/FCW, seulement `CR0.TS=1`
- ✅ **TSS.RSP0 mis à jour** (V7-C-03) : via `tcb.kstack_ptr` après chaque context switch
- ✅ **exec() séquence** (V7-C-04) : signal mask hérité + pending signals flushés

---

## 6. Mise à jour à prévoir

`ExoOS_Architecture_v7.md` §3.2 ("TCB Layout v7") devra être mis à jour dans une
version **v8** pour refléter le layout GI-01. Ce travail est dans le périmètre **GI-00**
(maintenance documentation) et ne bloque aucun GI en cours.

Pour l'instant, la règle est :

> **`GI-01_Types_TCB_SSR.md` §7 prime sur `ExoOS_Architecture_v7.md` §3.2 pour le layout TCB.**
> Tous les autres paragraphes de `Architecture_v7.md` restent authoritative.
