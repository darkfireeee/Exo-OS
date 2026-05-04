# CORR-ALPHA-03 — Scheduler : Docstring `context_switch()` incomplète et numérotation cassée

> **Auteur :** claude-alpha  
> **Date :** 2026-05-04  
> **Classe :** 🟠 SIL — Documentation critique absente  
> **Fichier :** `kernel/src/scheduler/core/switch.rs`  
> **Fonction :** `context_switch()`  
> **Sévérité :** Majeure — la séquence CET n'est pas documentée + étape 9 fantôme

---

## 1. Description du bug

### 1.1 Numérotation cassée

La docstring de `context_switch()` énumère les étapes ainsi :

```
/// # Séquence (GI-02 complète)
/// 1. Lazy FPU : si `prev` a utilisé la FPU → XSAVE...
/// 2. Sauvegarder PKRS (Intel PKS).
/// 3. Sauvegarder FS.base et user_gs_base via rdmsr (CORR-11).
/// 4. Marquer `prev` → Runnable.
/// 5. Poser CR0.TS=1, puis appeler `context_switch_asm(...)`.
/// 6. Restaurer PKRS de `next`.
/// 7. Marquer `next` → Running. Mettre à jour CURRENT_THREAD_PER_CPU.
/// 8. Mettre à jour TSS.RSP0 ← next.kstack_top() (V7-C-03 OBLIGATOIRE).
/// 10. Restaurer FS.base et user_gs_base de `next` via wrmsr (CORR-11).
```

**L'étape 9 est absente** — la numérotation saute directement de 8 à 10. Ce n'est pas une faute de frappe : deux étapes réelles sont exécutées dans le code entre 8 et 10 :
- La mise à jour de `percpu::set_current_tcb()` (publication du TCB dans GS:[0x20])  
- Le fence `SeqCst` de synchronisation entre TSS update et publication TCB

### 1.2 Étapes CET non documentées

Le code contient deux blocs CET (Control-flow Enforcement Technology) importants :

```rust
// Avant le switch (sauvegarde SSP de prev) :
if has_cet_ss {
    let ssp = unsafe { msr::read_msr(MSR_IA32_PL0_SSP) };
    prev.set_pl0_ssp(ssp);
}

// Après le switch (restauration SSP de next) :
if has_cet_ss {
    let ssp = next.pl0_ssp();
    unsafe { msr::write_msr(MSR_IA32_PL0_SSP, ssp) };
}
```

Ces étapes (FIX-CET-01) ne figurent **nulle part** dans la docstring. Un développeur lisant uniquement la doc ne saurait pas que `MSR_IA32_PL0_SSP` (Shadow Stack Pointer Ring 0) est sauvegardé/restauré à chaque switch.

### 1.3 Étape kstack_top / kernel_rsp manquante

Le code fait :
```rust
let next_kstack_top = next.kstack_top();
unsafe {
    percpu::set_kernel_rsp(next_kstack_top);  // ← non documenté dans la docstring
    tss::update_rsp0(cpu_id, next_kstack_top);
}
```

`set_kernel_rsp()` met à jour le RSP kernel dans la per-CPU data (slot GS). Cette étape est absente de la docstring.

---

## 2. Correctif — Docstring complète

### Fichier : `kernel/src/scheduler/core/switch.rs`

**Remplacer le bloc `/// # Séquence` par :**

```rust
/// Effectue le context switch de `prev` vers `next`.
///
/// # Séquence complète (GI-02 + FIX-CET-01 + V7-C-03)
///
/// ## Partie PRÉ-SWITCH (contexte de `prev`)
/// 1. **Lazy FPU save** : si `prev.fpu_loaded()` → `xsave64(prev.fpu_state_ptr)`.
///    Puis `cr0_set_ts()` (CR0.TS=1) pour déclencher `#NM` sur prochain accès FPU.
///    (RÈGLE SWITCH-02, V7-C-02 : PAS de MXCSR/FCW dans l'ASM)
/// 2. **Sauvegarde PKRS** : si CPU supporte PKS → `rdmsr(MSR_IA32_PKRS)` → `prev.pkrs`.
/// 3. **Sauvegarde SSP CET** : si `has_cet_ss()` → `rdmsr(MSR_IA32_PL0_SSP)` → `prev.pl0_ssp`.
///    (FIX-CET-01 : Shadow Stack Pointer Ring 0 par-thread, SDM Vol.1 §8.3.3)
/// 4. **Sauvegarde FS/GS** : `rdmsr(MSR_FS_BASE)` → `prev.fs_base` ;
///    `rdmsr(MSR_KERNEL_GS_BASE)` → `prev.user_gs_base`. (CORR-11)
///    ATTENTION : lire `MSR_KERNEL_GS_BASE` (0xC0000102), PAS `MSR_GS_BASE` (0xC0000101).
/// 5. **Hook sortant** : appel de `CONTEXT_SWITCH_OUT_HOOK` si installé (ex. security audit).
/// 6. **Transition d'état** : si `prev == Running` → `prev = Runnable`.
///
/// ## ASM SWITCH (change de contexte et d'espace d'adressage)
/// 7. **`context_switch_asm(prev_rsp*, next_rsp, next_cr3)`** :
///    - Sauvegarde les 6 registres callee-saved (rbx, rbp, r12-r15) sur la pile de `prev`.
///    - Écrit RSP de `prev` dans `prev.kstack_ptr`.
///    - Si `next_cr3 != prev_cr3` → `mov %rdx, %cr3` (switch espace d'adressage, KPTI).
///    - Charge RSP de `next` depuis `next.kstack_ptr`.
///    - Restaure les 6 registres callee-saved de `next`.
///    - `ret` → reprend l'exécution dans le contexte de `next`.
///
/// ## Partie POST-SWITCH (contexte de `next`)
/// 8.  **KPTI CR3 slots** : rafraîchit le slot CR3 per-CPU (`kpti::set_current_cr3`).
///     (FIX-KPTI-01 : évite une relecture stale après migration)
/// 9.  **Restauration PKRS** : si PKS → `wrmsr(MSR_IA32_PKRS, next.pkrs)`.
/// 10. **Restauration SSP CET** : si `has_cet_ss()` → `wrmsr(MSR_IA32_PL0_SSP, next.pl0_ssp)`.
///     (FIX-CET-01 : ssp=0 si `next` n'a jamais utilisé CET → désactive SS pour ce thread)
/// 11. **Transition d'état** : `next = Running` ; `switch_count++`.
/// 12. **Mise à jour TSS.RSP0 + kernel_rsp** :
///     `percpu::set_kernel_rsp(next.kstack_top())` ;
///     `tss::update_rsp0(cpu_id, next.kstack_top())`. (V7-C-03 OBLIGATOIRE)
///     Sans cela, la prochaine interruption Ring3→0 empilera sur la mauvaise pile.
/// 13. **Fence SeqCst** : barrière complète avant publication du TCB.
/// 14. **Publication TCB** :
///     `percpu::set_current_tcb(next)` → met à jour GS:[0x20] ;
///     `CURRENT_THREAD_PER_CPU[cpu_id].store(next, Release)` → visible aux autres CPUs.
/// 15. **Restauration FS/GS** :
///     `wrmsr(MSR_FS_BASE, next.fs_base)` ; `wrmsr(MSR_KERNEL_GS_BASE, next.user_gs_base)`.
///     (CORR-11 : user_gs devient actif via SWAPGS au retour Ring 3)
///
/// # Sécurité
/// - Appelé avec préemption désactivée (IrqGuard ou PreemptGuard).
/// - `prev` et `next` DOIVENT être des pointeurs valides non-null.
/// - Cette fonction NE DOIT JAMAIS appeler `process::signal::*` (RÈGLE SWITCH-01).
pub unsafe fn context_switch(prev: &mut ThreadControlBlock, next: &mut ThreadControlBlock) {
```

---

## 3. Impact scope

- **Fichier modifié :** `kernel/src/scheduler/core/switch.rs`
- **Nature :** correction de docstring uniquement — aucun changement de comportement runtime
- **Documentation externe à synchroniser :** `GI-02_Boot_ContextSwitch.md` §4 (vérifier cohérence)
- **TLA+ ContextSwitch.tla :** les étapes 1-11 correspondent aux `SwitchStage 0-11` du modèle — la docstring corrigée est maintenant alignée

---

## 4. Vérification de cohérence TLA+

Le module `ContextSwitch.tla` modélise la séquence en 12 stages (0-11). Le mapping mis à jour :

| Stage TLA+ | Action TLA+ | Étape docstring corrigée |
|-----------|-------------|--------------------------|
| 0 | `SysUseFpu` ou `StartSwitch` | — (pre-switch) |
| 1 | `Step1_Xsave` | Étape 1 (FPU save) |
| 2 | `Step2_SetLazyBit` | Étape 1 (CR0.TS=1) |
| 3-4 | `Step3_4_Internal` | Étapes 2-6 (PKRS, CET, FS/GS, hook, state) |
| 5 | `Step5_AsmSwitch` | Étape 7 (ASM) |
| 6-7 | `Step6_7_Internal` | Étapes 8-10 (KPTI, PKRS, CET restore) |
| 8 | `Step8_UpdateGsAndTss` | Étapes 11-14 (state, TSS, fence, TCB publish) |
| 9-10 | `Step9_10_RestoreMSRs` | Étape 15 (FS/GS restore) |
| 11 | `Step11_Finish` | — (cleanup) |

---

*— claude-alpha*
