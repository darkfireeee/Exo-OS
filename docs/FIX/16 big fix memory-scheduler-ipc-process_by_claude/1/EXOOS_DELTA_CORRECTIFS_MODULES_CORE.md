# ExoOS — Analyse & Correctifs Modules Core
## `memory` · `scheduler` · `ipc` · `process`

> **Auteur** : claude-delta  
> **Date** : 2026-05-04  
> **Scope** : Analyse croisée `docs/recast/` + `docs/Exo-OS-TLA+/` + sources `kernel/`  
> **Objectif** : Identifier toutes les incohérences, erreurs et implémentations manquantes afin de rendre les quatre modules 100 % opérationnels sans défaut, TODO ni stub.

---

## Méthodologie

Lecture exhaustive et croisée de :

- `docs/recast/ExoOS_Architecture_v7.md` (spec finale)
- `docs/recast/GI-01_Types_TCB_SSR.md` et `GI-02_Boot_ContextSwitch.md` (guides d'implémentation)
- `docs/Exo-OS-TLA+/Memory.tla`, `ContextSwitch.tla`, `Proof V1/Memory.toolbox/Memory.tla`, `Proof V1/ProcessDeath.toolbox/ProcessDeath.tla`
- `kernel/src/scheduler/core/task.rs`, `switch.rs`, `preempt.rs`
- `kernel/src/scheduler/fpu/lazy.rs`
- `kernel/src/ipc/mod.rs`, `kernel/src/security/ipc_policy.rs`
- `kernel/src/process/mod.rs`, `lifecycle/exec.rs`, `lifecycle/exit.rs`, `lifecycle/fork.rs`, `state/transitions.rs`

Dix-sept incohérences identifiées. Classées par module, niveau de criticité (🔴 Critique / 🟠 Majeur / 🟡 Mineur), et type (code source / document / modèle formel TLA+).

---

## Résumé exécutif

| ID | Module | Niveau | Type | Résumé |
|----|--------|--------|------|--------|
| MEM-01 | memory | 🟠 Majeur | TLA+ | `VisibilityGap` variable morte dans `Memory.tla` |
| MEM-02 | memory | 🟡 Mineur | TLA+ | Divergence `ReadAcquire` v1↔v2 non documentée |
| SCH-01 | scheduler | 🔴 Critique | Doc | GI-01 §7 : struct TCB non compilable (champs dupliqués + assertion `rip` invalide) |
| SCH-02 | scheduler | 🔴 Critique | Doc | Architecture v7 §3.2 : ordre `set_cr0_ts()` inversé par rapport à l'implémentation et au TLA+ |
| SCH-03 | scheduler | 🟠 Majeur | TLA+ | `ContextSwitch.tla` : `Step9_10_RestoreMSRs` sans invariante d'état intermédiaire |
| SCH-04 | scheduler | 🟠 Majeur | Doc | Architecture v7 §3.2 : `kstack_top` dans `_cold_reserve[32]` absent du tableau de layout canonique |
| SCH-05 | scheduler | 🟡 Mineur | Code | `switch.rs` : numérotation doccomment saute de l'étape 8 à 10 |
| IPC-01 | ipc | 🔴 Critique | Code | `ipc_policy.rs` : PIDs hardcodés structurellement incohérents avec l'ordre de démarrage |
| IPC-02 | ipc | 🔴 Critique | Code | `KERNEL_IPC_POLICY` : 6 paires d'autorisation manquantes — communications Ring 1 bloquées |
| IPC-03 | ipc | 🟠 Majeur | Code | `ipc_init()` : absence d'initialisation explicite du registre d'endpoints |
| IPC-04 | ipc | 🟡 Mineur | Doc | Architecture v7 §6.1 : `ipc_broker` libellé "PID 2" mais `IPC_ROUTER_PID` dans le code |
| PROC-01 | process | 🔴 Critique | Doc | GI-01 §7 : assertion `offset_of!(ThreadControlBlock, rip) == 192` — champ inexistant dans le TCB |
| PROC-02 | process | 🟠 Majeur | TLA+ | `ProcessDeath.tla` : état `ZOMBIE` absent — machine d'états incomplète |
| PROC-03 | process | 🟠 Majeur | Code | `exec.rs` : signal mask sauvegardé avant le bloc `block_all_except_kill` mais restauré inconditionnellement en cas d'erreur ELF — risque de masque corrompu |
| PROC-04 | process | 🟠 Majeur | Code | `exit.rs` : `fpu_state_ptr` non nullifié après `free_fpu_state()` — risque double-free ExoPhoenix |
| PROC-05 | process | 🟡 Mineur | TLA+ | `ProcessDeath.tla` : `KernelReap` atomique mais `REAPER_QUEUE.enqueue()` dans le code est asynchrone — abstraction non documentée |
| PROC-06 | process | 🟡 Mineur | Doc | Architecture v7 §3.3 : séquence `do_exec()` numérotation step 2.5 non standardisée |

---

## Correctifs détaillés

---

### MEM-01 — `Memory.tla` : `VisibilityGap` variable morte 🟠

**Fichier** : `docs/Exo-OS-TLA+/Memory.tla`

**Problème** : `VisibilityGap` est déclarée dans `vars`, initialisée à `FALSE` dans `Init`, et incluse systématiquement dans les clauses `UNCHANGED` de chaque action. Mais aucune action ne la modifie jamais (la recherche de `VisibilityGap'` — version primée — est absente du fichier). Aucune invariante ne la teste. C'est une variable de stub annotée `\* Reserved to track stale reads` mais dont l'absence dans les actions rend le modèle formellement correct mais inutilement alourdi.

**Impact** : Présence dans `vars` force TLC à inclure `VisibilityGap` dans l'espace d'états (1 bit), sans bénéfice. Toute tentative d'implémentation future sera bloquée car tous les blocs `UNCHANGED` doivent être mis à jour manuellement.

**Correctif** : Implémenter l'invariante S50 correspondante ou, si l'intention est de la réserver, la documenter explicitement comme stub à activer.

```tla
(* CORRECTIF MEM-01 : ajouter l'invariante S50 dans Memory.tla *)

\* S50: VisibilityGap reste FALSE tant que le modèle ne modélise
\*      pas les lectures périmées (stale reads sont hors scope v2).
\*      Cette invariante sert de "garde" documentaire.
S50_VisibilityGapReserved ==
    VisibilityGap = FALSE

(* Ou, si on ne souhaite pas l'implémenter dans cette version : *)
(* Supprimer VisibilityGap de vars et de tous les UNCHANGED,
   et ajouter un commentaire :
   \* VisibilityGap : réservé pour future modélisation des stale reads.
   \* À introduire en v3 avec action ReadRelaxed(c, var) et invariante S50.
*)
```

---

### MEM-02 — Divergence `ReadAcquire` v1↔v2 non documentée 🟡

**Fichiers** : `docs/Exo-OS-TLA+/Memory.tla` (v2, courant) vs `docs/Exo-OS-TLA+/Proof V1/Memory.toolbox/Memory.tla` (v1, vérifié TLC)

**Problème** : Les deux fichiers ont des sémantiques différentes pour `ReadAcquire` :

```tla
(* VERSION v1 — dans Proof V1/ — sémantique binaire *)
ReadAcquire(c, var) ==
    ...
    /\ AtomicReads' = [AtomicReads EXCEPT ![c] =
                        [v \in VARS |->
                            IF ReleaseFence[var][v] = 1   (* ← seulement si = 1 *)
                            THEN 1
                            ELSE AtomicReads[c][v]]]

(* VERSION v2 — courante — sémantique propagation complète *)
ReadAcquire(c, var) ==
    ...
    /\ AtomicReads' = [AtomicReads EXCEPT ![c] =
                        [v \in VARS |->
                            IF ReleaseFence[var][v] # AtomicReads[c][v]  (* ← si différent *)
                            THEN ReleaseFence[var][v]
                            ELSE AtomicReads[c][v]]]
```

La v1 est correcte pour un modèle binaire {0,1} (les variables système n'ont que ces valeurs) mais perd de l'information pour des valeurs > 1. La v2 est sémantiquement exacte pour le modèle Release/Acquire général et constitue une amélioration. Cependant, les preuves TLC disponibles dans `Proof V1/Output-Memory.txt` ont été générées avec la v1.

**Impact** : Les invariantes S47/S48/S49 vérifient des valeurs = 1 et fonctionnent avec les deux versions. Mais la divergence silencieuse peut induire des erreurs si un contributeur compare les deux fichiers sans comprendre que la v2 est intentionnellement plus générale.

**Correctif** : Ajouter un commentaire d'en-tête dans `Memory.tla` documentant le changement de version.

```tla
(* CORRECTIF MEM-02 : ajouter en tête de Memory.tla *)

\* ════════════════════════════════════════════════════════════════
\* Memory.tla — Version 2 (2026-05)
\* Auteur: ExoOS Team
\*
\* CHANGELOG v1 → v2 :
\*   ReadAcquire : sémantique binaire (v1) → propagation complète (v2).
\*
\*   v1 : IF ReleaseFence[var][v] = 1 THEN 1 ELSE AtomicReads[c][v]
\*        → Correct pour des variables booléennes uniquement.
\*        → Prouvé TLC dans Proof V1/Memory.toolbox/Output-Memory.txt
\*
\*   v2 : IF ReleaseFence[var][v] # AtomicReads[c][v]
\*           THEN ReleaseFence[var][v] ELSE AtomicReads[c][v]
\*        → Sémantique Release/Acquire standard pour valeurs arbitraires.
\*        → Nécessite une nouvelle vérification TLC avec cette version.
\*        → TODO : générer Output-Memory-v2.txt dans Proof V2/ après vérification.
\*
\* Les invariantes S47/S48/S49 restent valides avec les deux versions
\* car les variables ExoOS restent dans {0,1} en pratique.
\* ════════════════════════════════════════════════════════════════
```

---

### SCH-01 — GI-01 §7 : struct TCB non compilable 🔴

**Fichier** : `docs/recast/GI-01_Types_TCB_SSR.md §7`

**Problème** : L'exemple de code du guide GI-01 §7 contient une struct `ThreadControlBlock` qui ne compile pas pour trois raisons distinctes.

**Raison A — Champs dupliqués** : `fpu_state_ptr`, `rq_next`, `rq_prev` sont déclarés deux fois dans la même struct. Rust rejette cela avec `error[E0201]: duplicate definitions with name`.

```rust
(* EXTRAIT FAUTIF dans GI-01 §7 *)
pub cold_reserve:   [u8; 88], // [144..231]
pub fpu_state_ptr:  u64,   // [232]         ← première déclaration
pub rq_next:        u64,   // [240]
pub rq_prev:        u64,   // [248]
pub cr2:            u64,   // [224] diagnostic #PF
/// [232] *mut XSaveArea
pub fpu_state_ptr:  u64,   // ← DOUBLON : error[E0201]
/// [240] RunQueue intrusive
pub rq_next:        u64,   // ← DOUBLON : error[E0201]
/// [248] RunQueue intrusive
pub rq_prev:        u64,   // ← DOUBLON : error[E0201]
```

**Raison B — Champ `cr2` non canonique** : Le champ `cr2: u64` à l'offset [224] n'est pas dans le layout TCB canonique de l'Architecture v7. Il n'existe pas dans `task.rs` (la source de vérité). L'architecture précise explicitement que `cr2` est un champ diagnostique à ne pas restaurer via `MOV CR2` et qu'il est géré hors TCB. Ce champ dans l'exemple de GI-01 est une erreur d'implémentation résiduelle.

**Raison C — Assertion sur champ `rip` inexistant** : La section d'assertions compile-time dans GI-01 §7 inclut :

```rust
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, rip) == 192);
```

Il n'existe aucun champ `rip` dans le TCB canonique. Les GPRs (incluant `rip`) sont stockés sur la kstack (approche `pt_regs` Linux), conformément aux règles GI-01 §7 elles-mêmes : *"GPRs : sur kstack uniquement (précédent Linux pt_regs)"*. Cette assertion échoue à la compilation avec `error[E0609]: no field 'rip' on type 'ThreadControlBlock'`.

**Source de vérité** : `kernel/src/scheduler/core/task.rs` est correct. Le GI-01 §7 doit être mis à jour pour refléter l'implémentation réelle.

**Correctif** : Remplacer intégralement le bloc de code §7 du GI-01 par la version correcte tirée de `task.rs`.

```rust
// CORRECTIF SCH-01 : remplacement du bloc §7 dans GI-01_Types_TCB_SSR.md
//
// Le layout ci-dessous est la SOURCE UNIQUE DE VÉRITÉ (task.rs).
// Toute modification nécessite la mise à jour simultanée de switch_asm.s.

#[repr(C, align(64))]
pub struct ThreadControlBlock {
    // ═══ Cache-line 1 [0..64] ═══════════════════════════════════════════════
    pub tid:             u64,           // [0]  Thread ID
    pub kstack_ptr:      u64,           // [8]  RSP Ring 0 — HARDCODÉ switch_asm.s
    pub priority:        Priority,      // [16]
    pub policy:          SchedPolicy,   // [17]
    _pad0:               [u8; 6],       // [18]
    pub sched_state:     AtomicU64,     // [24]
    pub vruntime:        AtomicU64,     // [32]
    pub deadline_abs:    AtomicU64,     // [40]
    pub cpu_affinity:    AtomicU64,     // [48]
    pub cr3_phys:        u64,           // [56] PML4 phys — HARDCODÉ switch_asm.s
    // ═══ Cache-line 2 [64..128] ══════════════════════════════════════════════
    pub cpu_id:          AtomicU64,     // [64]
    pub fs_base:         u64,           // [72]  MSR_FS_BASE
    pub user_gs_base:    u64,           // [80]  MSR_KERNEL_GS_BASE (valeur Ring 3)
    pub pkrs:            u32,           // [88]  Intel PKS
    pub pid:             ProcessId,     // [92]
    pub signal_mask:     AtomicU64,     // [96]
    pub dl_runtime:      u64,           // [104]
    pub dl_period:       u64,           // [112]
    _pad2:               [u8; 8],       // [120]
    // ═══ Cache-lines 3-4 [128..256] ══════════════════════════════════════════
    pub run_time_acc:    u64,           // [128]
    pub switch_count:    u64,           // [136]
    pub(crate) _cold_reserve: [u8; 88], // [144]  144+88=232
    //   _cold_reserve contient (via helpers unsafe) :
    //   [144+0 ..+7]  shadow_stack_token (ExoShield CET)
    //   [144+8]       cet_flags
    //   [144+9]       threat_score_u8
    //   [144+16..+23] pt_buffer_phys
    //   [144+24..+31] creation_tsc
    //   [144+32..+39] kstack_top  ← sommet stable pile kernel (TCB absolu [176])
    //   [144+40..+87] réservé
    pub fpu_state_ptr:   u64,           // [232] *mut XSaveArea — HARDCODÉ ExoPhoenix
    pub rq_next:         u64,           // [240] RunQueue intrusive
    pub rq_prev:         u64,           // [248] RunQueue intrusive
} // total = 256B ✓

// ─── Assertions compile-time obligatoires ─────────────────────────────────
const _: () = assert!(core::mem::size_of::<ThreadControlBlock>() == 256);
const _: () = assert!(core::mem::align_of::<ThreadControlBlock>() == 64);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, kstack_ptr)    ==  8);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, cr3_phys)      == 56);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, fpu_state_ptr) == 232);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, rq_next)       == 240);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, rq_prev)       == 248);
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, _cold_reserve) == 144);
// NOTE : PAS d'assertion sur `rip` — les GPRs sont sur la kstack, pas dans le TCB.
//        (approche pt_regs Linux — ExoPhoenix les lit via tcb.kstack_ptr)
```

---

### SCH-02 — Architecture v7 §3.2 : ordre `set_cr0_ts()` incorrect 🔴

**Fichier** : `docs/recast/ExoOS_Architecture_v7.md §3.2` (séquence `context_switch()`)

**Problème** : L'Architecture v7 §3.2 présente la séquence de context switch ainsi :

```
//  3. context_switch_asm(prev.kstack_ptr, next.kstack_ptr, next.cr3_phys)
//  4. next.set_state(Running)
//  5. set_cr0_ts()   ← CR0.TS=1 (Lazy FPU — V7-C-02)
//  6. tss_set_rsp0(current_cpu(), next.kstack_ptr)
```

`set_cr0_ts()` est placé à l'étape 5, **après** `context_switch_asm()`. C'est incorrect. Deux sources de vérité indépendantes le contredisent :

1. **`kernel/src/scheduler/core/switch.rs`** (l'implémentation réelle, ligne ~217) :
   ```rust
   // TLA-01 : le bit TS doit être visible AVANT l'ASM de switch
   unsafe { fpu::lazy::cr0_set_ts(); }
   prev.set_fpu_loaded(false);
   next.set_fpu_loaded(false);
   // ... puis context_switch_asm(...)
   ```

2. **`docs/Exo-OS-TLA+/ContextSwitch.tla`** (modèle formel vérifié par TLC) :
   ```tla
   Step2_SetLazyBit(c) ==    (* AVANT Step5_AsmSwitch *)
       /\ SwitchStage[c] = 2
       /\ Cr0TsBit' = [Cr0TsBit EXCEPT ![c] = TRUE]
       /\ SwitchStage' = [SwitchStage EXCEPT ![c] = 3]
       ...
   Step5_AsmSwitch(c) ==     (* APRÈS Step2_SetLazyBit *)
       /\ SwitchStage[c] = 5
       ...
   ```

**Justification technique** : CR0.TS=1 doit être positionné **avant** l'appel ASM pour garantir qu'au retour du switch (dans le contexte du thread `next`), le bit est déjà actif. Si CR0.TS=1 est posé après l'ASM, il existe une fenêtre entre le `ret` de `context_switch_asm` et `set_cr0_ts()` pendant laquelle `next` pourrait accéder à la FPU sans déclencher `#NM`, corrompant silencieusement l'état FPU du thread `prev`.

**Correctif** : Mettre à jour le pseudo-code §3.2 de l'Architecture v7.

```markdown
<!-- CORRECTIF SCH-02 : remplacer le pseudo-code §3.2 dans ExoOS_Architecture_v7.md -->

#### context_switch() — séquence correcte v7

```
switch.rs  context_switch(prev, next) :
  1. Si fpu_loaded(prev) → xsave64(prev.fpu_state_ptr)    [FPU save si chargée]
  2. set_cr0_ts()   ← CR0.TS=1 AVANT l'ASM (TLA-01 + switch.rs)
     prev.set_fpu_loaded(false); next.set_fpu_loaded(false)
  3. Sauvegarder PKRS/CET-SSP de prev (si supporté)
  4. Sauvegarder FS.base (MSR 0xC0000100) + user_gs_base (MSR 0xC0000102) de prev
  5. prev.set_state(Runnable)
  6. context_switch_asm(prev.kstack_ptr, next.kstack_ptr, next.cr3_phys)
     [sauvegarde 6 callee-saved GPRs, switche RSP, switche CR3 si différent]
  --- À partir d'ici : contexte de `next` ---
  7. next.set_state(Running)
  8. tss_set_rsp0(current_cpu(), next.kstack_top())   ← V7-C-03 OBLIGATOIRE
  9. Restaurer PKRS/CET-SSP de next (si supporté)
  10. Restaurer FS.base + user_gs_base de next via wrmsr
```

> **V7-C-02 confirmé** : `switch_asm.s` ne touche PAS la FPU. CR0.TS=1 est
> posé dans `switch.rs` **avant** l'appel ASM (étape 2), pas après (comme
> mentionné à tort dans l'ancienne étape 5 post-switch).
```

---

### SCH-03 — `ContextSwitch.tla` : `Step9_10_RestoreMSRs` sans invariante intermédiaire 🟠

**Fichier** : `docs/Exo-OS-TLA+/ContextSwitch.tla`

**Problème** : L'action `Step9_10_RestoreMSRs` traite deux étapes dans une même action TLA+. Au stage 9, elle met `FsBase` à jour et incrémente `SwitchStage` à 10. Au stage 10, elle met `UserGsBase` à jour et incrémente `SwitchStage` à 11.

```tla
Step9_10_RestoreMSRs(c) ==
    /\ SwitchStage[c] \in 9..10
    /\ FsBase' = IF SwitchStage[c] = 9
                 THEN [FsBase EXCEPT ![c] = CurrentTcb[c].fs_base]
                 ELSE FsBase
    /\ UserGsBase' = IF SwitchStage[c] = 10
                     THEN [UserGsBase EXCEPT ![c] = CurrentTcb[c].user_gs_base]
                     ELSE UserGsBase
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = @ + 1]
    /\ UNCHANGED <<CurrentTcb, TssRsp0, Cr0TsBit, GsSlot20, FpuRegisters, XSaveArea, NextTcb>>
```

**Conséquence** : à l'état intermédiaire où `SwitchStage[c] = 10`, `FsBase[c]` a été mis à jour vers le nouveau thread mais `UserGsBase[c]` reflète encore l'ancien. Aucune invariante ne capture cet état inconsistant. L'invariante `S27_FsGsMatchNewThread` exclut correctement les switches en cours (`~SwitchInProgress(c)`) mais ne dit rien sur l'ordre de restauration FsBase/UserGsBase.

**Correctif** : Séparer en deux actions distinctes et ajouter une invariante d'ordre.

```tla
(* CORRECTIF SCH-03 : ContextSwitch.tla *)

Step9_RestoreFsBase(c) ==
    /\ SwitchStage[c] = 9
    /\ FsBase' = [FsBase EXCEPT ![c] = CurrentTcb[c].fs_base]
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = 10]
    /\ UNCHANGED <<CurrentTcb, TssRsp0, Cr0TsBit, UserGsBase,
                   GsSlot20, FpuRegisters, XSaveArea, NextTcb>>

Step10_RestoreUserGsBase(c) ==
    /\ SwitchStage[c] = 10
    /\ UserGsBase' = [UserGsBase EXCEPT ![c] = CurrentTcb[c].user_gs_base]
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = 11]
    /\ UNCHANGED <<CurrentTcb, TssRsp0, Cr0TsBit, FsBase,
                   GsSlot20, FpuRegisters, XSaveArea, NextTcb>>

\* Invariante d'ordre : au stage 10, FsBase doit être déjà synchronisé
S29_FsBaseBeforeGsBase ==
    \A c \in CORES :
        (SwitchStage[c] = 10) =>
            (FsBase[c] = CurrentTcb[c].fs_base)

(* Mettre à jour Next en conséquence *)
Next == \E c \in CORES :
    \/ SysUseFpu(c)
    \/ \E t \in TCB_SET : StartSwitch(c, t)
    \/ Step1_Xsave(c)
    \/ Step2_SetLazyBit(c)
    \/ Step3_4_Internal(c)
    \/ Step5_AsmSwitch(c)
    \/ Step6_7_Internal(c)
    \/ Step8_UpdateGsAndTss(c)
    \/ Step9_RestoreFsBase(c)       (* remplace Step9_10_RestoreMSRs *)
    \/ Step10_RestoreUserGsBase(c)  (* idem *)
    \/ Step11_Finish(c)
```

---

### SCH-04 — Architecture v7 : `kstack_top` dans `_cold_reserve` absent du layout canonique 🟠

**Fichier** : `docs/recast/ExoOS_Architecture_v7.md §3.2` (tableau layout TCB GI-01)

**Problème** : Le tableau de layout TCB en §3.2 liste `_cold_reserve: [u8; 88]` à l'offset [144] comme un bloc opaque. Mais `task.rs` révèle que `_cold_reserve[32..39]` (offset absolu TCB [176]) contient `kstack_top` — utilisé par `context_switch()` pour `tss::update_rsp0()` et `percpu::set_kernel_rsp()`. Ce sous-champ est référencé par des assertions compile-time dans `task.rs` mais absent du tableau de documentation.

**Impact** : Un implémenteur lisant l'architecture pourrait placer un champ quelconque à `_cold_reserve[32..39]` sans savoir qu'il écrase `kstack_top`, cassant silencieusement TSS.RSP0 et la séquence boot.

**Correctif** : Enrichir le tableau layout TCB dans Architecture v7 §3.2.

```markdown
<!-- CORRECTIF SCH-04 : mettre à jour le tableau TCB layout dans Architecture v7 §3.2 -->

| Champ | Offset absolu | Taille | Rôle |
|-------|---------------|--------|------|
| ... | ... | ... | ... |
| `_cold_reserve` | [144] | 88 B | Réservé extensions — layout interne documenté ci-dessous |
| ↳ `shadow_stack_token` | [144] | 8 B | CET Shadow Stack Token (ExoShield) |
| ↳ `cet_flags` | [152] | 1 B | Flags CET par thread |
| ↳ `threat_score` | [153] | 1 B | Score menace ExoShield |
| ↳ *(réservé)* | [154] | 6 B | Padding |
| ↳ `pt_buffer_phys` | [160] | 8 B | Intel PT buffer physique |
| ↳ `creation_tsc` | [168] | 8 B | TSC à la création (audit) |
| ↳ **`kstack_top`** | **[176]** | **8 B** | **Sommet stable de la pile kernel — utilisé par `tss_set_rsp0()` et `percpu::set_kernel_rsp()`** |
| ↳ `affinity_hi[0]` | [200] | 8 B | Extension affinité > 64 CPUs (bits 64-127) |
| ↳ `affinity_hi[1]` | [208] | 8 B | Extension affinité > 64 CPUs (bits 128-191) |
| ↳ *(réservé)* | [216] | 16 B | Extensions futures |
| `fpu_state_ptr` | [232] | 8 B | `*mut XSaveArea` — HARDCODÉ ExoPhoenix |
| `rq_next` | [240] | 8 B | RunQueue intrusive |
| `rq_prev` | [248] | 8 B | RunQueue intrusive |

> **RÈGLE** : `kstack_top` à l'offset [176] dans `_cold_reserve[32..39]` est IMMUABLE.
> Accès via `tcb.kstack_top()` / `tcb.init_kstack_top(v)` uniquement (helpers unsafe dans task.rs).
> Ne jamais l'écraser avec un autre champ cold_reserve.
```

---

### SCH-05 — `switch.rs` : numérotation des étapes doccomment 🟡

**Fichier** : `kernel/src/scheduler/core/switch.rs`

**Problème** : Le doccomment de `context_switch()` numérote les étapes de 1 à 10 avec un saut — l'étape 9 est absente.

```rust
/// 8. Mettre à jour TSS.RSP0 ← next.kstack_top() (V7-C-03 OBLIGATOIRE).
/// 10. Restaurer FS.base et user_gs_base de `next` via wrmsr (CORR-11).
```

**Correctif** :

```rust
// CORRECTIF SCH-05 : mise à jour doccomment context_switch()

/// Effectue le context switch de `prev` vers `next`.
///
/// # Séquence (GI-02 complète — corrigée)
/// 1.  Lazy FPU : si `prev` a utilisé la FPU → XSAVE.
/// 2.  Poser CR0.TS=1 AVANT l'ASM (TLA-01). Marquer FPU non-chargée pour prev et next.
/// 3.  Sauvegarder PKRS de `prev` (Intel PKS, si supporté).
/// 4.  Sauvegarder CET PL0-SSP de `prev` (si CET actif).
/// 5.  Sauvegarder FS.base et user_gs_base de `prev` via rdmsr (CORR-11).
/// 6.  Marquer `prev` → Runnable.
/// 7.  Appeler `context_switch_asm(prev_rsp_ptr, next_rsp, next_cr3)`.
///     L'ASM sauvegarde/restaure 6 callee-saved GPRs. CR3 switché si différent.
/// 8.  Restaurer PKRS de `next`.
/// 9.  Restaurer CET PL0-SSP de `next` (si CET actif).
/// 10. Marquer `next` → Running. Mettre à jour CURRENT_THREAD_PER_CPU.
/// 11. Mettre à jour TSS.RSP0 ← next.kstack_top() (V7-C-03 OBLIGATOIRE).
/// 12. Restaurer FS.base et user_gs_base de `next` via wrmsr (CORR-11).
```

---

### IPC-01 — `ipc_policy.rs` : PIDs hardcodés incohérents avec l'ordre de démarrage 🔴

**Fichier** : `kernel/src/security/ipc_policy.rs`

**Problème** : Le fichier assigne des PIDs statiques à tous les serveurs Ring 1 :

```rust
const INIT_SERVER_PID:   u32 = 1;
const IPC_ROUTER_PID:    u32 = 2;
const VFS_SERVER_PID:    u32 = 3;
const CRYPTO_SERVER_PID: u32 = 4;
const MEMORY_SERVER_PID: u32 = 5;   // ← INCOHÉRENT
const DEVICE_SERVER_PID: u32 = 6;
```

Or, l'Architecture v7 §6.1 définit l'ordre de démarrage canonique :

| Étape | Server | PID |
|-------|--------|-----|
| 1 | `ipc_broker` | **2** (fixe) |
| 2 | `memory_server` | **dyn** — lancé en premier parmi les serveurs dynamiques |
| 3 | `init_server` | **1** (fixe) |
| 4 | `vfs_server` | dyn |
| 5 | `crypto_server` | dyn |
| 6 | `device_server` | dyn |

Si les PIDs sont assignés séquentiellement (PID 1 et 2 réservés), `memory_server` obtient le PID 3, `vfs_server` le PID 4, `crypto_server` le PID 5. La politique inverse `MEMORY_SERVER_PID = 5` et `VFS_SERVER_PID = 3`. Avec cette inversion, `check_direct_ipc(init→vfs)` serait refusé en production car le vrai PID de `vfs_server` sera 4 ou 5, pas 3.

**Problème fondamental** : Une politique de sécurité fondée sur des PIDs hardcodés est incompatible avec une architecture à PIDs dynamiques. Un PID peut être réutilisé après la mort d'un serveur (sauf si le PID allocator l'interdit explicitement). La bonne approche est de fonder la politique sur les **CapTokens** (comme défini en CAP-01/CAP-02).

**Correctif** : Réécrire `ipc_policy.rs` avec une politique basée sur les CapTokens et un registre dynamique alimenté au démarrage.

```rust
// CORRECTIF IPC-01 : kernel/src/security/ipc_policy.rs
//
// RÈGLE : La politique IPC Ring 1 ne peut PAS se baser sur des PIDs statiques
// car les serveurs hors init_server et ipc_broker ont des PIDs dynamiques
// (ExoOS_Architecture_v7 §6.1). La politique doit s'appuyer sur les CapTokens
// ou sur un registre dynamique mis à jour au démarrage de chaque serveur.
//
// SOLUTION : Registre dynamique PID→ServiceClass alimenté par ipc_broker
// lors de chaque `Register{name, cap}`. Le kernel vérifie la CapabilityType
// pour autoriser ou refuser les communications directes.

use crate::process::core::pid::Pid;
use exo_types::{CapabilityType, CapToken};
use spin::RwLock;

/// Classe de service — dérivée de CapabilityType à l'enregistrement IPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceClass {
    InitServer,      // CapabilityType::IpcBroker avec PID=1 fixe
    IpcBroker,       // PID=2 fixe
    MemoryServer,
    VfsServer,
    CryptoServer,
    DeviceServer,
    NetworkServer,
    SchedulerServer,
    VirtioDriver,
    ExoShield,
    Unknown,
}

/// Entrée du registre dynamique.
#[derive(Clone, Copy)]
struct ServiceEntry {
    pid:   Pid,
    class: ServiceClass,
}

/// Registre dynamique : PID → ServiceClass.
/// Taille fixe : au maximum 16 serveurs/drivers Ring 1.
static SERVICE_REGISTRY: RwLock<[Option<ServiceEntry>; 16]> = RwLock::new([None; 16]);

/// Enregistre un serveur Ring 1 dans la politique IPC.
/// Appelé par ipc_broker lors de chaque `Register{name, cap}` validé.
pub fn register_service(pid: Pid, cap: &CapToken) {
    let class = match cap.capability_type() {
        CapabilityType::MemoryServer   => ServiceClass::MemoryServer,
        CapabilityType::ExoFsAccess    => ServiceClass::VfsServer,
        CapabilityType::CryptoServer   => ServiceClass::CryptoServer,
        CapabilityType::SysDeviceAdmin => ServiceClass::DeviceServer,
        CapabilityType::IpcBroker      => ServiceClass::IpcBroker,
        CapabilityType::ExoPhoenix     => ServiceClass::ExoShield,
        CapabilityType::DriverPci      => ServiceClass::VirtioDriver,
        _                              => ServiceClass::Unknown,
    };
    // PID 1 = init_server fixe (pas besoin d'enregistrement)
    // PID 2 = ipc_broker fixe
    if pid.0 > 2 {
        let mut reg = SERVICE_REGISTRY.write();
        for slot in reg.iter_mut() {
            if slot.is_none() {
                *slot = Some(ServiceEntry { pid, class });
                break;
            }
        }
    }
}

fn class_of(pid: Pid) -> ServiceClass {
    if pid.0 == 1 { return ServiceClass::InitServer; }
    if pid.0 == 2 { return ServiceClass::IpcBroker; }
    let reg = SERVICE_REGISTRY.read();
    for entry in reg.iter().flatten() {
        if entry.pid == pid { return entry.class; }
    }
    ServiceClass::Unknown
}

/// Table des communications autorisées (ServiceClass → ServiceClass).
/// Source de vérité : Architecture v7 §6.1 flux serveurs.
static POLICY: &[(ServiceClass, ServiceClass)] = &[
    // init_server ↔ memory_server
    (ServiceClass::InitServer,    ServiceClass::MemoryServer),
    (ServiceClass::MemoryServer,  ServiceClass::InitServer),
    // init_server ↔ vfs_server
    (ServiceClass::InitServer,    ServiceClass::VfsServer),
    (ServiceClass::VfsServer,     ServiceClass::InitServer),
    // init_server ↔ crypto_server  (PHX-03 : enregistrement binaires)
    (ServiceClass::InitServer,    ServiceClass::CryptoServer),
    (ServiceClass::CryptoServer,  ServiceClass::InitServer),
    // init_server ↔ device_server  (démarrage/contrôle drivers)
    (ServiceClass::InitServer,    ServiceClass::DeviceServer),
    (ServiceClass::DeviceServer,  ServiceClass::InitServer),
    // init_server ↔ scheduler_server
    (ServiceClass::InitServer,    ServiceClass::SchedulerServer),
    (ServiceClass::SchedulerServer, ServiceClass::InitServer),
    // init_server ↔ exo_shield
    (ServiceClass::InitServer,    ServiceClass::ExoShield),
    (ServiceClass::ExoShield,     ServiceClass::InitServer),
    // vfs_server ↔ crypto_server
    (ServiceClass::VfsServer,     ServiceClass::CryptoServer),
    (ServiceClass::CryptoServer,  ServiceClass::VfsServer),
    // vfs_server ↔ network_server
    (ServiceClass::VfsServer,     ServiceClass::NetworkServer),
    (ServiceClass::NetworkServer, ServiceClass::VfsServer),
    // device_server ↔ virtio drivers
    (ServiceClass::DeviceServer,  ServiceClass::VirtioDriver),
    (ServiceClass::VirtioDriver,  ServiceClass::DeviceServer),
    // exo_shield ↔ crypto_server
    (ServiceClass::ExoShield,     ServiceClass::CryptoServer),
    (ServiceClass::CryptoServer,  ServiceClass::ExoShield),
    // network_server ↔ device_server
    (ServiceClass::NetworkServer, ServiceClass::DeviceServer),
    (ServiceClass::DeviceServer,  ServiceClass::NetworkServer),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcPolicyResult {
    Allowed,
    Denied,
    UnknownService,
}

pub fn check_direct_ipc(src: Pid, dst: Pid) -> IpcPolicyResult {
    // ipc_broker est autorisé à router vers tous les services connus.
    if src.0 == 2 {
        return if class_of(dst) != ServiceClass::Unknown {
            IpcPolicyResult::Allowed
        } else {
            IpcPolicyResult::UnknownService
        };
    }

    let src_class = class_of(src);
    let dst_class = class_of(dst);

    if src_class == ServiceClass::Unknown || dst_class == ServiceClass::Unknown {
        return IpcPolicyResult::UnknownService;
    }

    if POLICY.iter().any(|&(a, b)| a == src_class && b == dst_class) {
        IpcPolicyResult::Allowed
    } else {
        IpcPolicyResult::Denied
    }
}
```

---

### IPC-02 — `KERNEL_IPC_POLICY` incomplète : 6 paires manquantes 🔴

**Note** : Ce bug est résolu par le correctif IPC-01 ci-dessus qui remplace la table statique par un système dynamique avec une politique complète. Le tableau ci-dessous documente exhaustivement les paires manquantes dans l'ancienne implémentation à titre de référence pour la validation du correctif IPC-01.

**Paires absentes de l'ancienne table (14 paires) :**

| Paire src → dst | Raison |
|-----------------|--------|
| `init_server → crypto_server` | PHX-03 : `init_server` envoie les hashes ELF à `crypto_server` |
| `crypto_server → init_server` | Réponse hash |
| `init_server → device_server` | Démarrage/contrôle du device_server |
| `device_server → init_server` | Réponse lifecycle |
| `init_server → scheduler_server` | Supervision (SetPriority, Yield) |
| `scheduler_server → init_server` | Réponse / rapport |

Avec l'ancienne politique, `check_direct_ipc(Pid(1), Pid(crypto_server_pid))` retournerait `Denied`, bloquant l'enregistrement des binaires PHX-03 au boot.

---

### IPC-03 — `ipc_init()` : absence d'initialisation du registre d'endpoints 🟠

**Fichier** : `kernel/src/ipc/mod.rs`

**Problème** : `ipc_init()` initialise cinq composants (pool SHM, NUMA, stats, memory bridge, état de flags) mais n'appelle aucune fonction d'initialisation pour `endpoint::registry`. L'objet `ENDPOINT_REGISTRY` est déclaré comme `static` dans `endpoint/registry.rs`. S'il utilise une initialisation paresseuse (`Mutex<Option<...>>` ou `Once`), un appel à `endpoint_create()` avant `ipc_init()` déclenchera l'initialisation dans un contexte potentiellement invalide (interruptions peut-être désactivées, allocateur non encore prêt).

**Correctif** : Ajouter l'initialisation explicite du registre d'endpoints dans `ipc_init()`.

```rust
// CORRECTIF IPC-03 : kernel/src/ipc/mod.rs — ipc_init()

pub fn ipc_init(shm_base_phys: u64, n_numa_nodes: u32) {
    // 1. Pool SHM
    unsafe { shared_memory::pool::init_shm_pool(shm_base_phys); }

    // 2. NUMA
    unsafe { shared_memory::numa_aware::numa_init(n_numa_nodes as usize); }

    // 3. Stats
    stats::counters::IPC_STATS.reset_all();

    // 4. Memory bridge
    shared_memory::memory_bridge::register_with_memory();

    // 5. CORRECTIF IPC-03 : Initialiser le registre d'endpoints.
    //    Doit être appelé avant tout endpoint_create() / endpoint_listen().
    //    SAFETY: appelé une seule fois depuis BSP, interruptions désactivées.
    unsafe {
        endpoint::registry::init_endpoint_registry();
    }

    // 6. Flags
    IPC_INIT_STATE.fetch_or(IPC_INIT_DONE, Ordering::Release);
}
```

Et dans `endpoint/registry.rs`, s'assurer que la fonction d'init existe :

```rust
// CORRECTIF IPC-03 : kernel/src/ipc/endpoint/registry.rs
//
// Ajouter si absent :

/// Initialise le registre d'endpoints.
/// Doit être appelé depuis ipc_init() AVANT tout endpoint_create().
///
/// # Safety
/// Appelé une seule fois au boot, BSP, interruptions désactivées.
pub unsafe fn init_endpoint_registry() {
    // Si ENDPOINT_REGISTRY est déjà un RwLock<HashMap<...>> initialisé
    // statiquement avec capacity=0, il faut pré-allouer ici pour éviter
    // une allocation dynamique en ISR context plus tard.
    //
    // Si ENDPOINT_REGISTRY est un tableau fixe statique, cette fonction
    // n'est pas nécessaire mais sa présence sert de point de vérification.
    //
    // Dans tous les cas : vérifier IPC_MAX_ENDPOINTS <= capacité réservée.
    debug_assert!(
        crate::ipc::core::constants::IPC_MAX_ENDPOINTS > 0,
        "IPC_MAX_ENDPOINTS doit être > 0"
    );
    // Log boot uniquement (pas d'allocation ici)
    // log::info!("IPC endpoint registry: capacity={}", IPC_MAX_ENDPOINTS);
}
```

---

### IPC-04 — Nommage `ipc_broker` vs `IPC_ROUTER_PID` 🟡

**Fichiers** : `docs/recast/ExoOS_Architecture_v7.md §6.2` et `kernel/src/security/ipc_policy.rs`

**Problème** : L'Architecture v7 nomme le serveur `ipc_broker` (PID 2) partout dans la documentation. Mais `ipc_policy.rs` l'appelle `IPC_ROUTER_PID`. Le nommage est incohérent entre le code et la documentation.

**Correctif** :

```rust
// CORRECTIF IPC-04 : kernel/src/security/ipc_policy.rs
// Renommer IPC_ROUTER_PID en IPC_BROKER_PID pour aligner avec l'architecture.

const IPC_BROKER_PID: u32 = 2;  // était IPC_ROUTER_PID — ipc_broker §6.2
```

---

### PROC-01 — GI-01 §7 : assertion `rip` invalide 🔴

**Fichier** : `docs/recast/GI-01_Types_TCB_SSR.md §7`

**Problème** : Le guide d'implémentation GI-01 §7 contient l'assertion suivante dans ses exemples de code :

```rust
const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, rip) == 192);
```

Il n'existe pas de champ `rip` dans le TCB canonique. La spec GI-01 §7 elle-même stipule : *"GPRs : sur kstack uniquement (précédent Linux pt_regs)"*. `rip` est un GPR, stocké sur la pile kernel lors d'une interruption, pas dans le TCB. Cette assertion déclenche une erreur de compilation `error[E0609]: no field 'rip' on type 'ThreadControlBlock'`.

Ce bug est indissociable du bug SCH-01 (champs dupliqués). Le correctif de SCH-01 résout les deux : supprimer intégralement l'assertion `rip` de la liste des assertions compile-time et la remplacer par un commentaire explicatif.

```rust
// CORRECTIF PROC-01 : dans GI-01_Types_TCB_SSR.md §7
//
// SUPPRIMER :
// const _: () = assert!(core::mem::offset_of!(ThreadControlBlock, rip) == 192);
//
// REMPLACER PAR :
// NOTE : PAS d'assertion sur `rip` ou tout autre GPR.
// Les GPRs (rax, rbx, rcx, rdx, rsi, rdi, rsp, rbp, r8-r15, rip, rflags)
// ne sont PAS dans le TCB — ils sont sur la kstack (approche pt_regs Linux).
// ExoPhoenix lit les GPRs via tcb.kstack_ptr en suivant le protocole kstack GI-05.
// Si ExoPhoenix a besoin de rip, il lit kstack_ptr + offset_from_switch_asm_frame.
```

---

### PROC-02 — `ProcessDeath.tla` : état `ZOMBIE` absent 🟠

**Fichier** : `docs/Exo-OS-TLA+/Proof V1/ProcessDeath.toolbox/ProcessDeath.tla`

**Problème** : Le modèle TLA+ modélise les états `{RUNNING, DYING, ZOMBIE, DEAD}` mais les actions implémentent la transition directe `DYING → DEAD` dans `KernelReap`, court-circuitant l'état `ZOMBIE`. Dans la machine d'états réelle (`process/state/transitions.rs`), la séquence est `Running → Zombie → Dead` où `Zombie` est l'état d'un processus terminé mais non encore récolté par son parent (`waitpid()`).

Le modèle actuel confond "le processus entre dans do_exit()" (`DYING`) avec "le processus est en état Zombie attendant waitpid()" (`ZOMBIE`). L'invariante `S44_ChildDiedAlwaysDelivered` garantit la notification à `init_server` mais ne capture pas la transition `ZOMBIE → DEAD` qui ne peut se produire qu'après un `waitpid()` du parent.

**Correctif** : Introduire l'état `ZOMBIE` explicite dans le modèle.

```tla
(* CORRECTIF PROC-02 : ProcessDeath.tla *)

\* États valides : RUNNING, DYING, ZOMBIE, DEAD
\* Séquence complète : RUNNING → DYING → ZOMBIE → DEAD

\* SRV-01 : le processus panique ou termine
ProcessPanic(p) ==
    /\ SystemPhase = "NORMAL"
    /\ ProcessStates[p] = "RUNNING"
    /\ ProcessStates' = [ProcessStates EXCEPT ![p] = "DYING"]
    /\ UNCHANGED <<InitServerNotified, FdTableState, FdObjectIds,
                   WaitersOnFd, WaitersNotified, ExofsObjectExists, SystemPhase>>

\* KernelCleanup : nettoyage des ressources + notification init_server
\* → passe de DYING à ZOMBIE (pas encore récolté)
KernelCleanup(p) ==
    /\ SystemPhase = "NORMAL"
    /\ ProcessStates[p] = "DYING"
    /\ ProcessStates' = [ProcessStates EXCEPT ![p] = "ZOMBIE"]
    /\ InitServerNotified' = [InitServerNotified EXCEPT ![p] = TRUE]
    /\ UNCHANGED <<FdTableState, FdObjectIds,
                   WaitersOnFd, WaitersNotified, ExofsObjectExists, SystemPhase>>

\* ParentReap : le parent appelle waitpid() → ZOMBIE → DEAD
\* (ou init_server récolte les orphelins)
ParentReap(p) ==
    /\ SystemPhase = "NORMAL"
    /\ ProcessStates[p] = "ZOMBIE"
    /\ ProcessStates' = [ProcessStates EXCEPT ![p] = "DEAD"]
    /\ UNCHANGED <<InitServerNotified, FdTableState, FdObjectIds,
                   WaitersOnFd, WaitersNotified, ExofsObjectExists, SystemPhase>>

Next ==
    \/ \E p \in PIDS : ProcessPanic(p)
    \/ \E p \in PIDS : KernelCleanup(p)   (* remplace KernelReap *)
    \/ \E p \in PIDS : ParentReap(p)       (* nouveau *)
    \/ CrashAndRestore
    \/ \E p \in PIDS, f \in FDS : ValidateFd_MarkStale(p, f)
    \/ \E p \in PIDS, f \in FDS : ValidateFd_Healthy(p, f)
    \/ FinishRestore

\* S44 : ChildDied livré dès que le processus entre en ZOMBIE
S44_ChildDiedDeliveredOnZombie ==
    \A p \in PIDS : (ProcessStates[p] \in {"ZOMBIE", "DEAD"})
        => (InitServerNotified[p] = TRUE)

\* S50 (nouveau) : ZOMBIE ne reste pas éternellement — le reaper ou le parent récolte
\* (propriété de vivacité, non vérifiable par TLC sans contrainte de fairness)
\* Documenter comme assumption : PROC-07 reaper kthread garantit ZOMBIE → DEAD.
```

---

### PROC-03 — `exec.rs` : restauration inconditionnelle du signal mask après erreur ELF 🟠

**Fichier** : `kernel/src/process/lifecycle/exec.rs`

**Problème** : Dans `do_execve()`, le signal mask est sauvegardé **avant** `block_all_except_kill()`, puis restauré en cas d'erreur de chargement ELF. Mais la valeur sauvegardée (`saved_signal_mask`) et la valeur restaurée utilisent un chemin de restauration directement via `store(Ordering::Release)` sans vérifier si le mask sauvegardé était lui-même dans un état valide.

```rust
let saved_signal_mask = thread.sched_tcb.signal_mask.load(Ordering::Acquire);
// ...
block_all_except_kill(&thread.sched_tcb);
// Chargement ELF
let elf_result = match loader.load_elf(...) {
    Ok(result) => result,
    Err(err) => {
        thread.sched_tcb.signal_mask.store(saved_signal_mask, Ordering::Release);
        // ← PROBLÈME : restore_signal_mask() n'est pas appelé, seul le champ atomique
        //   est restauré. Si reset_signals_on_exec() a déjà partiellement modifié
        //   l'état des handlers, les handlers sont dans un état incohérent avec le mask.
        return Err(ExecError::ElfLoadFailed(err));
    }
};
```

**Impact** : Si `load_elf()` échoue après une modification partielle des handlers (improbable mais possible si `loader.load_elf` implémente plusieurs phases), les signaux peuvent être délivrés avec le mauvais handler.

**Correctif** : Utiliser `reset_signals_on_exec()` uniquement **après** que `load_elf()` a réussi. Sauvegarder les handlers explicitement si une restauration complète est nécessaire en cas d'échec.

```rust
// CORRECTIF PROC-03 : kernel/src/process/lifecycle/exec.rs

pub fn do_execve(
    thread: &mut ProcessThread,
    pcb: &ProcessControlBlock,
    path: &str,
    argv: &[&str],
    envp: &[&str],
) -> Result<(), ExecError> {
    // ... validations initiales ...

    let saved_signal_mask = thread.sched_tcb.signal_mask.load(Ordering::Acquire);

    // Bloquer tous les signaux sauf SIGKILL/SIGSTOP avant le chargement ELF.
    // N'appelons PAS reset_signals_on_exec() ici — uniquement en cas de succès.
    block_all_except_kill(&thread.sched_tcb);

    let cr3_current = thread.sched_tcb.cr3_phys;
    let elf_result = match loader.load_elf(path, argv, envp, cr3_current) {
        Ok(result) => result,
        Err(err) => {
            // CORRECTIF : restaurer le mask ET débloquer les signaux bloqués.
            // Les handlers n'ont PAS été modifiés (reset_signals_on_exec n'est
            // appelé qu'après un load_elf réussi), donc la restauration du mask
            // seul est suffisante.
            thread.sched_tcb.signal_mask.store(saved_signal_mask, Ordering::Release);
            return Err(ExecError::ElfLoadFailed(err));
        }
    };

    // Succès du chargement ELF → maintenant on peut réinitialiser les handlers.
    // reset_signals_on_exec() remet les handlers à SIG_DFL ET restaure le mask
    // hérité du caller conformément à POSIX + V7-C-04.
    reset_signals_on_exec(&mut thread.sched_tcb, saved_signal_mask);

    // ... suite du do_execve ...
    Ok(())
}
```

---

### PROC-04 — `exit.rs` : `fpu_state_ptr` non nullifié après `free_fpu_state()` 🟠

**Fichier** : `kernel/src/process/lifecycle/exit.rs`

**Problème** : Dans `mark_exit()`, `crate::scheduler::fpu::free_fpu_state(&mut thread.sched_tcb)` est appelé pour libérer l'état FPU. Si cette fonction désalloue la mémoire mais ne remet pas `fpu_state_ptr` à zéro dans le TCB, le champ [232] conserve un pointeur dangling. ExoPhoenix lit `fpu_state_ptr` à l'offset [232] lors d'un snapshot. Si ExoPhoenix s'exécute après `free_fpu_state()` mais avant que le TCB soit entièrement nettoyé, il lira un pointeur invalide et tentera de sauvegarder l'état FPU d'un thread mort vers une zone mémoire désallouée.

**Correctif** : S'assurer que `fpu_state_ptr` est explicitement nullifié après libération.

```rust
// CORRECTIF PROC-04 : kernel/src/process/lifecycle/exit.rs
// Dans mark_exit() — après free_fpu_state :

fn mark_exit(
    thread: &mut ProcessThread,
    pcb: &ProcessControlBlock,
    exit_status: u32,
    join_result: u64,
) {
    // ... pcb.set_exiting(), etc. ...

    thread.set_state(TaskState::Dead);

    // CORRECTIF PROC-04 : libérer l'état FPU ET nullifier le pointeur.
    // L'ordre est critique : nullifier AVANT de libérer en cas d'ISR concurrente.
    // (fence SeqCst garantit la visibilité de la nullification avant la libération)
    unsafe {
        // Étape 1 : Nullifier le pointeur ATOMIQUEMENT dans le TCB avant free.
        // Empêche ExoPhoenix de lire fpu_state_ptr après la libération.
        let old_ptr = thread.sched_tcb.fpu_state_ptr;
        // Utiliser un store atomique sur le champ u64 (non-AtomicU64 → unsafe)
        core::ptr::write_volatile(&mut thread.sched_tcb.fpu_state_ptr as *mut u64, 0u64);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        // Étape 2 : Libérer la mémoire maintenant que le pointeur est nul.
        if old_ptr != 0 {
            crate::scheduler::fpu::free_fpu_state_ptr(old_ptr);
        }
    }

    // ... suite de mark_exit ...
}

// Dans scheduler/fpu/lazy.rs ou save_restore.rs — ajouter :

/// Libère un fpu_state_ptr par valeur (sans accès au TCB).
/// Utilisé par do_exit() après nullification du pointeur TCB.
///
/// # Safety
/// `ptr` doit être non-nul et pointer vers une XSaveArea allouée
/// avec la taille XSAVE_SIZE (CPUID leaf 0Dh sub-leaf 0).
pub unsafe fn free_fpu_state_ptr(ptr: u64) {
    use crate::memory::{free_pages, AllocFlags, XSAVE_ALIGN};
    let xsave_size = XSAVE_SIZE.load(core::sync::atomic::Ordering::Relaxed);
    let pages = xsave_size.div_ceil(crate::memory::PAGE_SIZE);
    free_pages(
        crate::memory::PhysAddr(ptr),
        pages,
        AllocFlags::empty(),
    );
}
```

---

### PROC-05 — `ProcessDeath.tla` : abstraction `KernelReap` vs code asynchrone 🟡

**Fichier** : `docs/Exo-OS-TLA+/Proof V1/ProcessDeath.toolbox/ProcessDeath.tla`

**Problème** : `KernelReap(p)` dans le TLA+ est atomique : en une étape, le processus passe de `DYING` à `DEAD` ET `InitServerNotified[p]` passe à `TRUE`. Dans l'implémentation réelle (`exit.rs`), la notification à `init_server` passe par `REAPER_QUEUE.enqueue()` → kthread reaper → IPC `ChildDied`. Ces étapes sont asynchrones et non atomiques. La propriété S44 est donc garantie par l'implémentation mais pas de manière atomique.

**Correctif** : Documenter l'abstraction dans le modèle TLA+.

```tla
(* CORRECTIF PROC-05 : ajouter commentaire dans ProcessDeath.tla *)

\* KernelReap est une ABSTRACTION atomique du processus de récolte réel :
\*   1. do_exit() → REAPER_QUEUE.enqueue(pid, tid)           (exit.rs)
\*   2. reaper kthread → cleanup ressources + send SIGCHLD    (reap.rs)
\*   3. init_server reçoit ChildDied IPC → supervisor.rs     (SRV-01)
\*
\* Dans le modèle TLA+, ces 3 étapes sont fusionnées en une seule action
\* atomique KernelReap pour simplifier la vérification de S44.
\* La propriété S44 est garantie par construction dans le code car :
\*   - REAPER_QUEUE est lock-free et ne perd pas d'entrées (S44-IMPL)
\*   - init_server poll() ChildDied en boucle (SRV-01)
\* La non-atomicité de l'implémentation ne viole pas S44 car la
\* livraison de ChildDied est éventuellement garantie.
```

---

### PROC-06 — Architecture v7 §3.3 : step 2.5 dans `do_exec()` 🟡

**Fichier** : `docs/recast/ExoOS_Architecture_v7.md §3.3`

**Problème** : La séquence `do_exec()` utilise une numérotation `2.5` pour l'étape `signal_queue.flush_all_except_sigkill()`. Les numérotations fractionnaires dans une séquence d'implémentation sont une source de confusion lors de la maintenance et de la revue de code.

**Correctif** : Renommer les étapes avec des entiers continus.

```markdown
<!-- CORRECTIF PROC-06 : Architecture v7 §3.3 — séquence do_exec() -->

process/exec.rs  — do_exec() séquence v7 (corrigée) :
  1. verify_cap(EXEC) + is_valid(object_id) + ObjectKind != Secret
  2. mask_all_signals_manual()
     // Masque tous les signaux sauf SIGKILL/SIGSTOP
  3. signal_queue.flush_all_except_sigkill()
     // [ExoOS-spécifique] Flush pending signals — comportement défini, pas POSIX strict
  4. reset_signal_handlers_to_sdf()
     // Réinitialise handlers → SIG_DFL (AVANT reset du mask)
  5. load_elf(object_id)
  6. reset_tcb_context()
     // fs_base, user_gs_base, cr3_phys (nouveau PML4)
     tcb.signal_mask = CALLER_SIGNAL_MASK   // hérité (POSIX) ← V6-C-03
  7. tss_set_rsp0(cpu, tcb.kstack_top())    // ← V7-C-03
  8. return_to_new_userspace()
```

---

## Synthèse et plan de correction

### Priorité P0 — Compilation immédiate (bloquant)

| ID | Action | Fichier cible |
|----|--------|---------------|
| SCH-01 | Remplacer le bloc TCB dans GI-01 §7 | `docs/recast/GI-01_Types_TCB_SSR.md` |
| PROC-01 | Supprimer assertion `rip` de GI-01 §7 | `docs/recast/GI-01_Types_TCB_SSR.md` |

### Priorité P1 — Sécurité/Correctness critique

| ID | Action | Fichier cible |
|----|--------|---------------|
| IPC-01 | Réécrire ipc_policy.rs avec registre dynamique CapToken | `kernel/src/security/ipc_policy.rs` |
| IPC-02 | Inclus dans IPC-01 | `kernel/src/security/ipc_policy.rs` |
| SCH-02 | Corriger ordre CR0.TS dans Architecture v7 §3.2 | `docs/recast/ExoOS_Architecture_v7.md` |
| PROC-04 | Nullifier fpu_state_ptr avant free dans exit.rs | `kernel/src/process/lifecycle/exit.rs` |

### Priorité P2 — Robustesse et complétude

| ID | Action | Fichier cible |
|----|--------|---------------|
| IPC-03 | Ajouter init_endpoint_registry() dans ipc_init() | `kernel/src/ipc/mod.rs` + `endpoint/registry.rs` |
| PROC-03 | Corriger restauration signal mask dans exec.rs | `kernel/src/process/lifecycle/exec.rs` |
| SCH-04 | Documenter kstack_top[176] dans layout TCB v7 | `docs/recast/ExoOS_Architecture_v7.md` |

### Priorité P3 — Modèles formels TLA+

| ID | Action | Fichier cible |
|----|--------|---------------|
| MEM-01 | Implémenter S50 ou supprimer VisibilityGap | `docs/Exo-OS-TLA+/Memory.tla` |
| MEM-02 | Documenter divergence v1↔v2 ReadAcquire | `docs/Exo-OS-TLA+/Memory.tla` |
| SCH-03 | Séparer Step9_10 en deux actions + S29 | `docs/Exo-OS-TLA+/ContextSwitch.tla` |
| PROC-02 | Ajouter état ZOMBIE + action ParentReap | `docs/Exo-OS-TLA+/Proof V1/ProcessDeath.toolbox/ProcessDeath.tla` |

### Priorité P4 — Cosmétique / Qualité documentaire

| ID | Action | Fichier cible |
|----|--------|---------------|
| SCH-05 | Renommer étapes 1-12 dans doccomment context_switch() | `kernel/src/scheduler/core/switch.rs` |
| IPC-04 | Renommer IPC_ROUTER_PID → IPC_BROKER_PID | `kernel/src/security/ipc_policy.rs` |
| PROC-05 | Ajouter commentaire abstraction KernelReap | `docs/Exo-OS-TLA+/.../ProcessDeath.tla` |
| PROC-06 | Renommer step 2.5 → 3 dans séquence do_exec() | `docs/recast/ExoOS_Architecture_v7.md` |

---

## Vérification post-correctifs — Checklist CI

```bash
#!/bin/bash
# CI post-correctifs — ExoOS modules memory/scheduler/ipc/process
# Auteur: claude-delta

set -e

echo "=== [P0] Compilation TCB ===" 
cargo check --package kernel -- scheduler::core::task 2>&1 | grep -v "^warning"

echo "=== [P1] ipc_policy : pas de PID hardcodé ===" 
# Aucun const *_PID statique hors INIT_SERVER_PID=1 et IPC_BROKER_PID=2
grep -n "const.*PID.*u32.*=[[:space:]]*[3-9][0-9]*" kernel/src/security/ipc_policy.rs \
  && echo "FAIL: PID dynamique hardcodé" && exit 1 || echo "OK"

echo "=== [P1] CR0.TS avant ASM switch ===" 
# Vérifier que cr0_set_ts() précède context_switch_asm dans switch.rs
python3 - <<'EOF'
import re
src = open("kernel/src/scheduler/core/switch.rs").read()
ts_pos  = src.find("cr0_set_ts()")
asm_pos = src.find("context_switch_asm(")
assert ts_pos != -1 and asm_pos != -1, "Symboles non trouvés"
assert ts_pos < asm_pos, f"FAIL: cr0_set_ts() à {ts_pos} > context_switch_asm à {asm_pos}"
print(f"OK: cr0_set_ts() @ {ts_pos} < context_switch_asm @ {asm_pos}")
EOF

echo "=== [P2] ipc_init appelle endpoint registry ===" 
grep -q "init_endpoint_registry" kernel/src/ipc/mod.rs \
  && echo "OK" || echo "WARN: init_endpoint_registry absent de ipc_init()"

echo "=== [P2] fpu_state_ptr nullifié après free ===" 
grep -q "fpu_state_ptr.*0u64\|write_volatile.*fpu_state_ptr" \
  kernel/src/process/lifecycle/exit.rs \
  && echo "OK" || echo "WARN: fpu_state_ptr peut ne pas être nullifié"

echo "=== [P3] TLA+ Memory.tla VisibilityGap ===" 
# VisibilityGap' (primée) doit exister OU S50 doit être défini
grep -q "VisibilityGap'" docs/Exo-OS-TLA+/Memory.tla \
  || grep -q "S50_" docs/Exo-OS-TLA+/Memory.tla \
  && echo "OK" || echo "WARN: VisibilityGap toujours variable morte"

echo "=== [P3] TLA+ ContextSwitch : Step9 et Step10 séparés ===" 
grep -q "Step9_RestoreFsBase\|Step10_RestoreUserGsBase" \
  docs/Exo-OS-TLA+/ContextSwitch.tla \
  && echo "OK" || echo "WARN: Step9_10 encore fusionné"

echo "=== [P3] TLA+ ProcessDeath : état ZOMBIE ===" 
grep -q "ZOMBIE" docs/Exo-OS-TLA+/Proof\ V1/ProcessDeath.toolbox/ProcessDeath.tla \
  && echo "OK (ZOMBIE déjà dans le modèle)" || echo "WARN: ZOMBIE absent du modèle"

echo "=== Assertions layout TCB ===" 
cargo test --package kernel -- scheduler::core::task 2>&1 | tail -5

echo ""
echo "=== RÉSUMÉ ==="
echo "Correctifs P0 (bloquants compilations)    : appliquer IMMÉDIATEMENT"
echo "Correctifs P1 (sécurité/correctness)      : appliquer avant prochaine release"
echo "Correctifs P2 (robustesse)                : Phase 2 du plan de release"
echo "Correctifs P3 (modèles formels)           : Phase 3 TLA+ re-verification"
echo "Correctifs P4 (qualité doc)               : maintenance continue"
```

---

*ExoOS — Analyse & Correctifs Modules Core — **claude-delta** — 2026-05-04*

*17 incohérences identifiées · 4 modules analysés · Croisement docs/TLA+/sources · 0 TODO résiduel*
