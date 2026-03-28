# Compte Rendu d'Implémentation — GI-02 : Boot Séquence, Context Switch, FPU Lazy

**Date** : 28 mars 2026  
**Auteur** : GitHub Copilot (Claude Sonnet 4.6)  
**Référence** : `docs/recast/GI-02_Boot_ContextSwitch.md` + `docs/recast/ExoOS_Corrections_02_Architecture.md`  
**Prérequis** : GI-01 ✅ (`exo-types`, TCB 256B, SSR — compilant sans erreur)  
**Résultat** : ✅ `cargo +nightly check -p exo-os-kernel --target x86_64-unknown-none` — **0 erreur**

---

## 1. Objectif

Implémenter et corriger la couche **context switch** du scheduler ExoOS selon le Guide d'Implémentation n°2, en appliquant rigoureusement toutes les corrections du fichier `ExoOS_Corrections_02_Architecture.md`.

Périmètre GI-02 couvert dans cette session :

| Étape GI-02 | Fichier | Statut |
|-------------|---------|--------|
| Étape 5 — `switch_asm.s` (6 GPRs, KPTI CR3) | `kernel/src/scheduler/asm/switch_asm.s` | ✅ Corrigé |
| Étape 6 — `context_switch()` (FS/GS, FPU, TSS) | `kernel/src/scheduler/core/switch.rs` | ✅ Corrigé |
| Étape 7 — FPU Lazy (commentaire MXCSR) | `kernel/src/scheduler/fpu/save_restore.rs` | ✅ Corrigé |
| Constante SMP `MAX_CPUS` | `kernel/src/scheduler/core/preempt.rs` | ✅ Corrigé |

---

## 2. Corrections Appliquées

### 2.1 V7-C-02 + CORR-18 — `switch_asm.s` : suppression MXCSR/FCW

**Problème initial :** Le fichier `switch_asm.s` sauvegardait `MXCSR` et `x87 FCW` via `stmxcsr`/`fstcw`/`fldcw`/`ldmxcsr` et réservait 16 octets supplémentaires sur la pile pour ces valeurs.

**Violation :** Le kernel ExoOS est compilé avec `-mmx,-sse,-sse2,+soft-float`. Le compilateur ne génère **aucune instruction SSE**. MXCSR ne peut donc pas être corrompu par le code kernel. Sauvegarder MXCSR/FCW dans `switch_asm.s` était à la fois inutile et dangereux (instructions SSE illégales avec ce profil de compilation).

**Règle spec :** V7-C-02 — *L'état FPU complet (MXCSR, FCW, registres x87/SSE/AVX) est géré **exclusivement** par `scheduler/fpu/save_restore.rs` via XSAVE/XRSTOR.*

**Correction :**
- Suppression des 4 instructions : `stmxcsr`, `fstcw`, `fldcw`, `ldmxcsr`
- Suppression de la réservation de 16B sur la pile (`subq $16, %rsp` / `addq $16, %rsp`)
- Mise à jour du commentaire d'en-tête (CORR-18) : *"6 registres callee-saved ABI System V UNIQUEMENT (rbx, rbp, r12-r15) — 6×8=48B + rip implicite = 56B"*

**Résultat final `switch_asm.s` :**
```asm
context_switch_asm:
    pushq   %r15
    pushq   %r14
    pushq   %r13
    pushq   %r12
    pushq   %rbp
    pushq   %rbx
    movq    %rsp, (%rdi)    // sauvegarde kstack_ptr du thread sortant
    testq   %rdx, %rdx
    jz      .L_skip_cr3
    movq    %rdx, %cr3      // KPTI : switch CR3 AVANT restauration
.L_skip_cr3:
    movq    %rsi, %rsp      // charge kstack_ptr du thread entrant
    popq    %rbx
    popq    %rbp
    popq    %r12
    popq    %r13
    popq    %r14
    popq    %r15
    ret
```

---

### 2.2 CORR-11 — `switch.rs` : sauvegarde/restauration FS/GS via rdmsr/wrmsr

**Problème initial :** `context_switch()` ne sauvegardait ni ne restaurait `FS.base` (TLS userspace) ni `user_GS.base` (valeur Ring 3) entre les threads. Résultat : corruption silencieuse du TLS après chaque switch entre threads différents (erreur silencieuse **S-06**).

**Règle spec :** CORR-11 — *FS.base et user_GS.base sont des MSR, non sauvegardés automatiquement par le CPU lors d'un context switch. Le scheduler doit les lire/écrire explicitement.*

**MSRs concernés :**
| MSR | Adresse | Contenu | Action |
|-----|---------|---------|--------|
| `MSR_FS_BASE` | `0xC000_0100` | FS.base courant (TLS userspace) | `rdmsr` avant switch, `wrmsr` après |
| `MSR_KERNEL_GS_BASE` | `0xC000_0102` | GS.base caché (valeur userspace) | `rdmsr` avant switch, `wrmsr` après |
| `MSR_GS_BASE` | `0xC000_0101` | GS.base kernel per-CPU | **NE PAS toucher** |

> ⚠️ **Erreur silencieuse S-06** : sauvegarder `MSR_GS_BASE` (0xC0000101) au lieu de `MSR_KERNEL_GS_BASE` (0xC0000102) corrompt le GS kernel et plante le scheduler.

**Champs TCB utilisés (GI-01 layout [64..128]) :**
```
[72]  fs_base:  u64   FS base (TLS) — défini en GI-01
[80]  gs_base:  u64   GS base       — défini en GI-01
```

**Code ajouté (Étape 2 — avant switch) :**
```rust
prev.fs_base = unsafe { msr::read_msr(MSR_FS_BASE) };
prev.gs_base = unsafe { msr::read_msr(MSR_KERNEL_GS_BASE) };
```

**Code ajouté (Étape 8 — après switch) :**
```rust
unsafe {
    msr::write_msr(MSR_FS_BASE,           next.fs_base);
    msr::write_msr(MSR_KERNEL_GS_BASE,    next.gs_base);
}
```

---

### 2.3 V7-C-02 — `switch.rs` : CR0.TS=1 + set_fpu_loaded(false) après switch

**Problème initial :** Après un context switch, `CR0.TS` restait à 0 (FPU accessible). Si `next` tentait d'utiliser des registres FPU sans les avoir restaurés, il lisait l'état FPU residuel du thread **précédent** — fuite silencieuse d'état (**S-12**).

**Règle spec :** V7-C-02 — *Après chaque context switch, poser `CR0.TS=1`. Le handler `#NM` (Device Not Available) dans `fpu/lazy.rs` détectera la première instruction FPU de `next`, fera `XRSTOR` de son état réel, puis remettra `CR0.TS=0`.*

**Code ajouté (Étape 6 — après switch) :**
```rust
unsafe { fpu::lazy::cr0_set_ts(); }
next.set_fpu_loaded(false);
```

---

### 2.4 V7-C-03 — `switch.rs` : TSS.RSP0 mis à jour après chaque switch

**Problème initial :** `TSS.RSP0` n'était pas mis à jour après un context switch. `TSS.RSP0` indique au CPU quelle pile kernel utiliser lors d'une interruption Ring 3 → Ring 0. S'il pointait vers la pile de l'**ancien** thread après un switch, la première interruption Ring 3 de `next` corrompait sa pile (**S-08**).

**Règle spec :** V7-C-03 — *`TSS.RSP0` **doit** être mis à jour après chaque context switch. C'est une exigence architecturale x86_64 obligatoire.*

**Code ajouté (Étape 7 — après switch) :**
```rust
unsafe {
    tss::update_rsp0(next.current_cpu().0 as usize, next.kstack_ptr);
}
```

> `tss::update_rsp0()` était déjà présent dans `kernel/src/arch/x86_64/tss.rs` (GI-01). Seul l'appel depuis `context_switch()` était manquant.

---

### 2.5 CORR-27 — `preempt.rs` : MAX_CPUS 64 → 256

**Problème initial :** `pub const MAX_CPUS: usize = 64` — incompatible avec `SSR_MAX_CORES_LAYOUT = 256` défini en GI-01 (`exo-phoenix-ssr`).

**Règle spec :** CORR-27 — *`MAX_CPUS` doit être ≥ `SSR_MAX_CORES_LAYOUT` (256). Le tableau `CURRENT_THREAD_PER_CPU[MAX_CPUS]` de `switch.rs` et le tableau `PREEMPT_COUNT[MAX_CPUS]` doivent couvrir les 256 CPUs.*

**Correction :**
```rust
// Avant :
pub const MAX_CPUS: usize = 64;

// Après :
pub const MAX_CPUS: usize = 256;  // CORR-27
```

---

### 2.6 Commentaire erroné — `save_restore.rs`

**Problème initial :** `fpu/save_restore.rs` contenait le commentaire :
> *"MXCSR et x87 FCW sont sauvegardés EXPLICITEMENT dans switch_asm.s"*

Ce commentaire était **faux** : depuis la correction V7-C-02, MXCSR et FCW sont gérés **exclusivement** par XSAVE/XRSTOR dans ce fichier. Un commentaire incorrect dans le code de sécurité d'un OS est aussi grave qu'un bug.

**Correction :** Commentaire supprimé et remplacé par la mention correcte que MXCSR/FCW sont couverts exclusivement par XSAVE/XRSTOR.

---

## 3. Imports ajoutés dans `switch.rs`

```rust
// Avant :
use crate::scheduler::fpu;

// Après (GI-02) :
use crate::scheduler::fpu;
use crate::arch::x86_64::{
    cpu::{features::CPU_FEATURES, msr::{self, MSR_FS_BASE, MSR_KERNEL_GS_BASE}},
    tss,
};
```

---

## 4. Séquence context_switch() complète (GI-02 final)

```
Étape 1 : Lazy FPU save          → XSAVE si prev.fpu_loaded() (V7-C-02)
Étape 2 : Sauvegarder PKRS       → rdmsr MSR_IA32_PKRS si CPU supporte PKS
Étape 3 : Sauvegarder FS/GS base → prev.fs_base = rdmsr(FS_BASE)
                                    prev.gs_base  = rdmsr(KERNEL_GS_BASE)  (CORR-11)
Étape 4 : prev → Runnable        → si état était Running
Étape 5 : context_switch_asm()   → 6 GPRs + CR3 KPTI (switch_asm.s)
           ─── À PARTIR D'ICI : contexte de `next` ───
Étape 6 : Restaurer PKRS         → wrmsr MSR_IA32_PKRS = next.pkrs
Étape 7 : next → Running         → + mise à jour CURRENT_THREAD_PER_CPU[]
Étape 8 : CR0.TS = 1             → fpu::lazy::cr0_set_ts() + set_fpu_loaded(false) (V7-C-02)
Étape 9 : TSS.RSP0 = next.kstack_ptr  → tss::update_rsp0() (V7-C-03)
Étape 10: Restaurer FS/GS base   → wrmsr(FS_BASE, next.fs_base)
                                    wrmsr(KERNEL_GS_BASE, next.gs_base)  (CORR-11)
```

---

## 5. Erreurs Silencieuses Éliminées

| ID | Description | Symptôme sans correction | Correction |
|----|-------------|-------------------------|------------|
| S-06 | TLS corrompu après switch | Crash aléatoire en userspace, pthread_self() retourne mauvaise valeur | CORR-11 rdmsr/wrmsr FS/GS |
| S-08 | IRQ Ring3 empile sur mauvaise pile | Corruption silencieuse, kernel panic aléatoire sous charge | V7-C-03 TSS.RSP0 |
| S-12 | Fuite état FPU entre threads | Calculs FP incorrects, NaN sporadiques | V7-C-02 CR0.TS=1 après switch |

---

## 6. Tableau Récapitulatif des Corrections

| ID Correction | Description | Fichier | Statut |
|---------------|-------------|---------|--------|
| V7-C-02 | Suppression MXCSR/FCW de switch_asm | `switch_asm.s` | ✅ |
| CORR-18 | Commentaire "6 GPRs callee-saved" | `switch_asm.s` | ✅ |
| CORR-11 | rdmsr/wrmsr FS/GS avant/après switch | `switch.rs` | ✅ |
| V7-C-02 | CR0.TS=1 + set_fpu_loaded(false) après switch | `switch.rs` | ✅ |
| V7-C-03 | TSS.RSP0 mis à jour après chaque switch | `switch.rs` | ✅ |
| CORR-27 | MAX_CPUS 64 → 256 | `preempt.rs` | ✅ |
| — | Commentaire MXCSR erroné supprimé | `save_restore.rs` | ✅ |

---

## 7. Fichiers Modifiés

```
kernel/src/scheduler/asm/switch_asm.s          ← V7-C-02 MXCSR/FCW supprimés
kernel/src/scheduler/core/switch.rs            ← CORR-11, V7-C-02, V7-C-03 ajoutés
kernel/src/scheduler/core/preempt.rs           ← CORR-27 MAX_CPUS=256
kernel/src/scheduler/fpu/save_restore.rs       ← commentaire erroné supprimé
docs/avancement/GI-02_Boot_ContextSwitch_FPU_COMPTE_RENDU.md  ← ce fichier
```

---

## 8. Résultat de Compilation

```
$ wsl -- bash -l -c "cd /mnt/c/Users/xavie/Desktop/Exo-OS && \
    cargo +nightly check -p exo-os-kernel \
    --target x86_64-unknown-none -Z build-std=core,alloc 2>&1"

   Compiling exo-os-kernel v0.1.0 (...)
   ...
   Finished `dev` profile [unoptimized + debuginfo] target(s) in 42.18s
```

**0 erreur — 0 warning bloquant.**

---

## 9. Prochaines Étapes

| Guide | Sujet | Dépendances |
|-------|-------|-------------|
| GI-03 | Drivers, IRQ framework, DMA | GI-01 ✅, GI-02 ✅ |
| GI-04 | ExoFS — core, blob, journaling | GI-01 ✅ |
| GI-05 | ExoPhoenix — handoff SSR | GI-01 ✅, GI-02 ✅ |
| GI-06 | Servers — init, VFS, IPC router | GI-01 ✅, GI-02 ✅ |
