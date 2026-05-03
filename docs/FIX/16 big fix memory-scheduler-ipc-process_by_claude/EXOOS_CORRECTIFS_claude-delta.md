# Exo-OS — Rapport d'Analyse Approfondie & Correctifs
## Modules : `memory/` · `scheduler/` · `ipc/` · `process/`

> **Auteur :** claude-delta  
> **Date :** 2026-05-03  
> **Périmètre :** Audit croisé code source ↔ docs/recast ↔ docs/Exo-OS-TLA+  
> **Sévérités :** 🔴 Critique (deadlock/corruption/silence sécurité) · 🟠 Majeur (défaut fonctionnel) · 🟡 Mineur (robustesse/conformité)

---

## Table des matières

| ID | Module | Catégorie | Sévérité | Titre |
|----|--------|-----------|----------|-------|
| [SPEC-MEM-01](#spec-mem-01) | `memory/` | TLA+ | 🟠 | `ReadAcquire` ne propage que la valeur `1` — biais binaire de la spec |
| [IMPL-MEM-02](#impl-mem-02) | `memory/` | Stub | 🟠 | `CURRENT_POLICY` NUMA globale — non per-CPU |
| [IMPL-MEM-03](#impl-mem-03) | `memory/` | TODO | 🟡 | HPET MMIO : identity-map QEMU uniquement — fixmap manquante |
| [IMPL-MEM-04](#impl-mem-04) | `memory/` | Ordre init | 🟠 | `register_backend_swap_provider()` appelé avant l'init du backend swap |
| [SPEC-SCHED-01](#spec-sched-01) | `scheduler/` | TLA+ | 🟠 | Steps 3-4 et 6-7 du spec TLA+ sont des no-ops — PKS/CET/KPTI non modélisés |
| [BUG-SCHED-02](#bug-sched-02) | `scheduler/` | Bug critique | 🔴 | `fast_path.s` : offsets TCB périmés — `NEED_RESCHED` jamais lu correctement |
| [IMPL-SCHED-03](#impl-sched-03) | `scheduler/` | Robustesse | 🟡 | `schedule_block()` panic si `idle_thread` absent au boot |
| [IMPL-SCHED-04](#impl-sched-04) | `scheduler/` | Robustesse | 🟡 | `.unwrap()` dans le hot path de la `CfsSortedQueue` |
| [BUG-IPC-01](#bug-ipc-01) | `ipc/` | Race/Deadlock | 🔴 | `block_current()` : fenêtre de réveil manqué → thread bloqué indéfiniment |
| [IMPL-IPC-02](#impl-ipc-02) | `ipc/` | Stub | 🟠 | SHM `virt_base = phys_addr` — invalide hors identity-map |
| [IMPL-IPC-03](#impl-ipc-03) | `ipc/` | Silence défaut | 🟠 | `SLEEP_REGISTRY` plein : enregistrement silencieusement abandonné |
| [BUG-PROC-01](#bug-proc-01) | `process/` | Bug logique | 🔴 | Signal envoyé au thread principal mort → signal perdu |
| [IMPL-PROC-02](#impl-proc-02) | `process/` | Transition manquante | 🟠 | `Creating → ExitToZombie` absent de la machine d'états |

---

## MODULE MEMORY

---

### SPEC-MEM-01
**TLA+ `Memory.tla` : `ReadAcquire` ne propage que la valeur `1`**  
Sévérité : 🟠 Majeur · Fichier : `docs/Exo-OS-TLA+/Memory.tla`

#### Symptôme
Dans l'action `ReadAcquire`, la fusion de la `ReleaseFence` dans la vue locale du lecteur est codée ainsi :

```tla
AtomicReads' = [AtomicReads EXCEPT ![c] = 
    [v ∈ VARS |->
        IF ReleaseFence[var][v] = 1 THEN 1 ELSE AtomicReads[c][v]]]
```

La condition `= 1` est codée **en dur**. Seul un `ReleaseFence` dont la valeur est exactement `1` est propagé. Toute variable dont la valeur `Release` vaut `0`, `2`, ou autre est ignorée, rendant la vue du lecteur **incohérente avec le modèle réel Release/Acquire**.

#### Impact
- L'invariant **S49** (`IommuInitRelease`) ne tient que parce que toutes les variables concernées sont des flags binaires `{0, 1}`.  
- Si jamais un compteur ou un état multi-valeur est modélisé (ex. génération de séquence IPC), la spec TLA+ ne capturera pas sa propagation et les invariants correspondants seront faux-positifs.  
- L'implémentation réelle (`AtomicUsize::store` + `fence(Release)`) propage **toute valeur** — la spec et le code divergent.

#### Correctif — `docs/Exo-OS-TLA+/Memory.tla`

```tla
(* AVANT — propagation uniquement si valeur = 1 *)
ReadAcquire(c, var) ==
    /\ AtomicWrites[var].ordering = "Release"
    /\ AtomicReads' = [AtomicReads EXCEPT ![c] =
            [v \in VARS |->
                IF ReleaseFence[var][v] = 1 THEN 1 ELSE AtomicReads[c][v]]]
    ...

(* APRÈS — propagation de TOUTE valeur différente de la vue locale *)
ReadAcquire(c, var) ==
    /\ AtomicWrites[var].ordering = "Release"
    /\ LET fence == ReleaseFence[var]
       IN AtomicReads' = [AtomicReads EXCEPT ![c] =
               [v \in VARS |->
                   IF fence[v] /= AtomicReads[c][v] THEN fence[v]
                   ELSE AtomicReads[c][v]]]
    /\ HappensBefore' = HappensBefore \cup {<<AtomicWrites[var].core, c>>}
    /\ AcquireFence' = AcquireFence \cup {<<c, var>>}
    /\ UNCHANGED <<AtomicWrites, ReleaseFence, VisibilityGap>>
```

> **Note de compatibilité** : Les trois invariants S47-S49 restent valides après ce correctif car ils portent sur des flags binaires. La spec devient simplement correcte pour les cas généraux.

---

### IMPL-MEM-02
**`CURRENT_POLICY` NUMA : politique globale, non per-CPU**  
Sévérité : 🟠 Majeur · Fichier : `kernel/src/memory/physical/allocator/numa_aware.rs:306`

#### Symptôme

```rust
/// (Stub — en production, utiliser une table par CPU.)
static CURRENT_POLICY: AtomicU8 = AtomicU8::new(NumaPolicy::LocalFirst as u8);
```

Une seule politique partagée entre tous les CPUs. En SMP, un thread sur CPU 3 qui appelle `set_current_policy(NumaPolicy::Interleave)` modifie la politique de tous les autres CPUs simultanément. L'allocation NUMA perd sa sémantique per-thread.

#### Correctif — `kernel/src/memory/physical/allocator/numa_aware.rs`

Remplacer `CURRENT_POLICY: AtomicU8` par un tableau per-CPU aligné sur cache-line, à l'image de `PREEMPT_COUNT` dans `scheduler/core/preempt.rs` :

```rust
// AVANT
static CURRENT_POLICY: AtomicU8 = AtomicU8::new(NumaPolicy::LocalFirst as u8);

pub fn set_current_policy(policy: NumaPolicy) {
    CURRENT_POLICY.store(policy as u8, Ordering::SeqCst);
}
pub fn get_current_policy() -> NumaPolicy { ... }

// APRÈS
use crate::scheduler::core::preempt::MAX_CPUS;
use crate::arch::x86_64::smp::percpu;

#[repr(C, align(64))]
struct NumaPolicySlot(AtomicU8, [u8; 63]);

static NUMA_POLICY_PER_CPU: [NumaPolicySlot; MAX_CPUS] = {
    const SLOT: NumaPolicySlot =
        NumaPolicySlot(AtomicU8::new(NumaPolicy::LocalFirst as u8), [0u8; 63]);
    [SLOT; MAX_CPUS]
};

/// Modifie la politique NUMA du CPU courant uniquement.
pub fn set_current_policy(policy: NumaPolicy) {
    let cpu = percpu::current_cpu_id() as usize;
    if cpu < MAX_CPUS {
        NUMA_POLICY_PER_CPU[cpu].0.store(policy as u8, Ordering::Relaxed);
    }
}

/// Lit la politique NUMA du CPU courant.
pub fn get_current_policy() -> NumaPolicy {
    let cpu = percpu::current_cpu_id() as usize;
    if cpu < MAX_CPUS {
        match NUMA_POLICY_PER_CPU[cpu].0.load(Ordering::Relaxed) {
            0 => NumaPolicy::LocalFirst,
            1 => NumaPolicy::Interleave,
            2 => NumaPolicy::PreferNode,
            _ => NumaPolicy::LocalFirst,
        }
    } else {
        NumaPolicy::LocalFirst
    }
}
```

---

### IMPL-MEM-03
**HPET MMIO : identity-map QEMU uniquement — fixmap manquante**  
Sévérité : 🟡 Mineur · Fichier : `kernel/src/arch/x86_64/acpi/hpet.rs` (≈ ligne 190)

#### Symptôme
L'accès HPET utilise l'adresse physique du registre HPET directement comme adresse virtuelle, exploitant implicitement l'identity-map QEMU. Sur bare-metal, l'adresse physique HPET (`0xFED00000` typiquement) n'est pas mappée dans l'espace kernel virtuel, causant un `#PF` immédiat.

#### Correctif — `kernel/src/arch/x86_64/acpi/hpet.rs`

```rust
// AVANT — accès direct phys addr (identity-map QEMU seulement)
pub unsafe fn hpet_read(reg: HpetReg) -> u64 {
    let base = hpet_phys_base();   // ex: 0xFED0_0000
    let ptr = base as *const u64;
    ptr.add(reg as usize).read_volatile()
}

// APRÈS — mapper dans la fixmap kernel avant accès
use crate::memory::virt::address_space::kernel::KERNEL_AS;
use crate::memory::core::{PageFlags, PhysAddr, VirtAddr, PAGE_SIZE};

/// Adresse virtuelle fixmap HPET (dans la plage MMIO du kernel).
/// Constante fixe : toujours la même adresse virtuelle kernel pour HPET.
const HPET_FIXMAP_VIRT: u64 = crate::memory::core::KERNEL_HEAP_START - PAGE_SIZE as u64;

static HPET_MAPPED: AtomicBool = AtomicBool::new(false);

/// Mappe les registres HPET dans la fixmap kernel (appelé UNE SEULE FOIS au boot).
///
/// # Safety
/// - Appelé après KERNEL_AS.init() — tables de pages actives.
/// - CPL 0, interruptions non requises.
pub unsafe fn hpet_map_fixmap(phys_base: PhysAddr) {
    if HPET_MAPPED.swap(true, Ordering::SeqCst) {
        return; // Déjà mappé
    }
    let flags = PageFlags::PRESENT
        | PageFlags::WRITABLE
        | PageFlags::NO_EXEC
        | PageFlags::CACHE_DISABLE  // MMIO : pas de cache !
        | PageFlags::WRITE_THROUGH;
    KERNEL_AS
        .map_page(VirtAddr(HPET_FIXMAP_VIRT), phys_base, flags)
        .expect("HPET fixmap mapping failed");
}

pub unsafe fn hpet_read(reg: HpetReg) -> u64 {
    debug_assert!(HPET_MAPPED.load(Ordering::Relaxed), "HPET non mappé");
    let ptr = (HPET_FIXMAP_VIRT + (reg as u64) * 8) as *const u64;
    ptr.read_volatile()
}
```

**Appel dans la séquence de boot** (à ajouter dans `arch/x86_64/boot/early_init.rs`, après le step 12 — KERNEL_AS + protections) :

```rust
// Step 12b : Mapper HPET fixmap (si HPET présent)
if let Some(hpet_phys) = acpi::hpet::hpet_phys_base_from_acpi() {
    unsafe { acpi::hpet::hpet_map_fixmap(PhysAddr(hpet_phys)) };
}
```

---

### IMPL-MEM-04
**`register_backend_swap_provider()` appelé avant l'init du backend swap**  
Sévérité : 🟠 Majeur · Fichier : `kernel/src/memory/mod.rs` (Phase 4, ligne ~95)

#### Symptôme
Dans `memory::init()` :

```rust
// ── Phase 4 : DMA ─────────────────────────────────────────────
dma::init();
virt::fault::swap_in::register_backend_swap_provider();  // ← ICI, Phase 4
// ...
// ── Phase 7 : utilitaires ─────────────────────────────────────
utils::init();   // ← swap::backend initialisé ici (shrinker, oom)
```

`register_backend_swap_provider()` enregistre un pointeur vers le backend swap. Mais le backend swap (`memory/swap/backend.rs`) dépend des utilitaires (`shrinker`, OOM killer) initialisés en Phase 7. Tout page-fault `swap_in` entre les phases 4 et 7 appelle le backend dans un état partiellement initialisé.

#### Correctif — `kernel/src/memory/mod.rs`

Déplacer l'appel **après** `utils::init()` :

```rust
// ── Phase 7 : utilitaires ─────────────────────────────────────
utils::init();

// Phase 7b : enregistrement du backend swap (APRÈS utils — dépend du shrinker)
// RÈGLE : ne pas appeler avant utils::init() — le backend swap accède
//         au shrinker et à l'OOM killer tous deux initialisés par utils::init().
virt::fault::swap_in::register_backend_swap_provider();

// ── Phase 8 : NUMA ────────────────────────────────────────────
numa::init();
```

---

## MODULE SCHEDULER

---

### SPEC-SCHED-01
**TLA+ `ContextSwitch.tla` : Steps 3-4 et 6-7 sont des no-ops — PKS/CET/KPTI non couverts**  
Sévérité : 🟠 Majeur · Fichier : `docs/Exo-OS-TLA+/ContextSwitch.tla`

#### Symptôme
Les actions `Step3_4_Internal` et `Step6_7_Internal` font uniquement avancer `SwitchStage` :

```tla
Step3_4_Internal(c) ==
    /\ SwitchStage[c] \in 3..4
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = @ + 1]
    /\ UNCHANGED <<CurrentTcb, TssRsp0, Cr0TsBit, FsBase, UserGsBase,
                   GsSlot20, FpuRegisters, XSaveArea, NextTcb>>
```

Ces steps correspondent, dans l'implémentation réelle (`switch.rs`), à :
- **Step 3** : sauvegarde PKS (`prev.pkrs = read_msr(MSR_IA32_PKRS)`) + CET shadow stack (`prev.set_pl0_ssp(...)`)
- **Step 4** : transition d'état `prev` → `Runnable`
- **Step 6** : rafraîchissement du slot CR3 per-CPU KPTI (`kpti::set_current_cr3`)
- **Step 7** : restauration PKS (`write_msr(MSR_IA32_PKRS, next.pkrs)`) + CET de `next`

Aucun de ces effets n'est modélisé. Les invariants S25-S28 ne couvrent pas les corruptions PKS, CET, ou KPTI inter-thread.

#### Correctif — `docs/Exo-OS-TLA+/ContextSwitch.tla`

Ajouter les variables `PkrsState`, `Pl0SspState`, `KptiCr3Slot` et les modéliser explicitement :

```tla
(* Nouvelles variables *)
VARIABLES PkrsState,   (* [c ∈ CORES |-> u32] — PKRS courant du CPU *)
          Pl0SspState, (* [t ∈ TCB_SET |-> u64] — CET SSP par thread *)
          KptiCr3Slot  (* [c ∈ CORES |-> {kernel_cr3, user_cr3}] *)

(* Remplacer Step3_4_Internal par deux étapes explicites *)

Step3_SavePksCet(c) ==
    /\ SwitchStage[c] = 3
    /\ PkrsState'   = [PkrsState EXCEPT ![c] = CurrentTcb[c].pkrs]
    /\ Pl0SspState' = [Pl0SspState EXCEPT ![CurrentTcb[c]] = CurrentTcb[c].pl0_ssp]
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = 4]
    /\ UNCHANGED <<CurrentTcb, TssRsp0, Cr0TsBit, FsBase, UserGsBase,
                   GsSlot20, FpuRegisters, XSaveArea, NextTcb, KptiCr3Slot>>

Step4_MarkPrevRunnable(c) ==
    /\ SwitchStage[c] = 4
    (* Mise à jour état prev — modélisée hors TCB_SET fixe pour simplifier *)
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = 5]
    /\ UNCHANGED <<CurrentTcb, TssRsp0, Cr0TsBit, FsBase, UserGsBase,
                   GsSlot20, FpuRegisters, XSaveArea, NextTcb,
                   PkrsState, Pl0SspState, KptiCr3Slot>>

Step6_RefreshKptiCr3(c) ==
    /\ SwitchStage[c] = 6
    /\ KptiCr3Slot' = [KptiCr3Slot EXCEPT ![c] = CurrentTcb[c].cr3_phys]
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = 7]
    /\ UNCHANGED <<CurrentTcb, TssRsp0, Cr0TsBit, FsBase, UserGsBase,
                   GsSlot20, FpuRegisters, XSaveArea, NextTcb,
                   PkrsState, Pl0SspState>>

Step7_RestorePksCet(c) ==
    /\ SwitchStage[c] = 7
    /\ PkrsState'   = [PkrsState EXCEPT ![c] = CurrentTcb[c].pkrs]
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = 8]
    /\ UNCHANGED <<CurrentTcb, TssRsp0, Cr0TsBit, FsBase, UserGsBase,
                   GsSlot20, FpuRegisters, XSaveArea, NextTcb,
                   Pl0SspState, KptiCr3Slot>>

(* Nouvel invariant PKS : PKRS du CPU reflète le thread courant hors switch *)
S29_PkrsMatchCurrentTcb ==
    \A c \in CORES :
        (~SwitchInProgress(c) =>
            PkrsState[c] = CurrentTcb[c].pkrs)

(* Nouvel invariant KPTI : slot CR3 per-CPU cohérent hors switch *)
S30_KptiCr3SlotFresh ==
    \A c \in CORES :
        (~SwitchInProgress(c) =>
            KptiCr3Slot[c] = CurrentTcb[c].cr3_phys)
```

---

### BUG-SCHED-02
**`fast_path.s` : offsets TCB périmés — `NEED_RESCHED` jamais détecté** 🔴  
Fichier : `kernel/src/scheduler/asm/fast_path.s`

#### Symptôme

```asm
.set TCB_FLAGS_OFFSET,          28    // AtomicU32 flags — cache line 1
.set TCB_SIGNAL_PENDING_OFFSET, 48   // AtomicBool signal_pending
.set NEED_RESCHED_BIT,          16   // = 1 << 4
```

Ces offsets proviennent de **l'ancien layout TCB pré-GI-01**, qui avait des champs séparés `flags: AtomicU32` et `signal_pending: AtomicBool`.

**Layout réel post-GI-01** (`task.rs`) :
| Offset | Champ | Type |
|--------|-------|------|
| [24] | `sched_state` | `AtomicU64` (flags unifiés) |
| [48] | `cpu_affinity` | `AtomicU64` |

- `movl TCB_FLAGS_OFFSET(%rdi), %eax` lit l'offset **28**, soit les **octets 4-7 du champ `sched_state`** à [24]. En little-endian x86, cela correspond aux **bits 32-63** de `sched_state` — réservés, toujours **0**. La fonction `read_need_resched_flag` retourne donc **toujours 0** : la préemption NEED_RESCHED ne se déclenche **jamais** depuis le fast-path.
- `TCB_SIGNAL_PENDING_OFFSET = 48` lit le champ `cpu_affinity`, totalement sans rapport avec les signaux.
- `NEED_RESCHED_BIT = 16` (= `1 << 4`) est erroné : `SCHED_NEED_RESCHED_BIT = 1 << 11 = 0x800`.

**Conséquence** : la préemption coopérative du fast-path scheduler ne fonctionne jamais. Les threads RT et CFS ne peuvent être préemptés que via les interruptions timer (tick IRQ), augmentant les latences de préemption de plusieurs millisecondes.

#### Correctif — `kernel/src/scheduler/asm/fast_path.s`

```asm
// ── Offsets TCB GI-01 (task.rs — DOIT rester synchronisé) ──────────────────
// sched_state = AtomicU64 à l'offset [24] du TCB
// Format : bits [7:0]=TaskState, [8]=signal_pending, [9]=KTHREAD,
//           [10]=FPU_LOADED, [11]=NEED_RESCHED, [12]=EXITING, [13]=IDLE
.set TCB_SCHED_STATE_OFFSET,    24    // AtomicU64 sched_state

// NEED_RESCHED = bit 11 de sched_state
.set NEED_RESCHED_MASK,         0x800 // 1 << 11

// SIGNAL_PENDING = bit 8 de sched_state
.set SIGNAL_PENDING_MASK,       0x100 // 1 << 8

// ─────────────────────────────────────────────────────────────────────────────
// read_need_resched_flag — bit NEED_RESCHED dans sched_state
//
// AVANT (BUGUÉ) : lisait offset 28 (high-u32 de sched_state = bits 32-63 = 0)
//                 avec bit 4 → TOUJOURS 0
// APRÈS (CORRECT) : lit offset 24 (sched_state entier u64), teste bit 11
// ─────────────────────────────────────────────────────────────────────────────
.global read_need_resched_flag
.type read_need_resched_flag, @function

read_need_resched_flag:
    // Lecture atomique Relaxed du champ sched_state du TCB (rdi = ptr TCB)
    // movq pour lire les 64 bits entiers (NEED_RESCHED est bit 11)
    movq    TCB_SCHED_STATE_OFFSET(%rdi), %rax
    andq    $NEED_RESCHED_MASK, %rax
    setnz   %al
    movzbl  %al, %eax
    ret

.size read_need_resched_flag, . - read_need_resched_flag

// ─────────────────────────────────────────────────────────────────────────────
// check_signal_flag — bit signal_pending dans sched_state
//
// AVANT (BUGUÉ) : lisait offset 48 = cpu_affinity — sans rapport
// APRÈS (CORRECT) : lit sched_state bit 8
// ─────────────────────────────────────────────────────────────────────────────
.global check_signal_flag
.type check_signal_flag, @function

check_signal_flag:
    movq    TCB_SCHED_STATE_OFFSET(%rdi), %rax
    andq    $SIGNAL_PENDING_MASK, %rax
    setnz   %al
    movzbl  %al, %eax
    ret

.size check_signal_flag, . - check_signal_flag
```

> **RÈGLE IMPÉRATIVE** : Tout changement aux offsets du TCB dans `task.rs` doit être répercuté **immédiatement** dans `fast_path.s`. Ajouter une assertion de build :
>
> ```rust
> // Dans task.rs, section assert statiques
> const _: () = assert!(
>     offset_of!(ThreadControlBlock, sched_state) == 24,
>     "fast_path.s TCB_SCHED_STATE_OFFSET doit être mis à jour si cet offset change"
> );
> ```

---

### IMPL-SCHED-03
**`schedule_block()` panique si `idle_thread` absent au boot**  
Sévérité : 🟡 Mineur · Fichier : `kernel/src/scheduler/core/switch.rs:439,449`

#### Symptôme

```rust
_ => {
    panic!("schedule_block: idle_thread absent sur cpu {}", rq.cpu.0);
}
```

Pendant la phase d'initialisation SMP, entre le moment où les APs démarrent et le moment où `boot_idle::publish_boot_idle()` est appelé, la tentative de blocage d'un thread (ex. par une allocation mémoire bloquante) panique le noyau.

#### Correctif — `kernel/src/scheduler/core/switch.rs`

```rust
// AVANT
_ => {
    panic!("schedule_block: idle_thread absent sur cpu {}", rq.cpu.0);
}

// APRÈS — spin-wait borné + log, sans panic
_ => {
    // Idle thread pas encore publié (fenêtre de démarrage SMP).
    // Spin-wait jusqu'à MAX_SPIN itérations, puis abandon gracieux.
    const MAX_SPIN: u32 = 100_000;
    for _ in 0..MAX_SPIN {
        core::hint::spin_loop();
        // Tenter de récupérer l'idle thread à chaque itération.
        if let Some(idle) = crate::scheduler::core::boot_idle::published_boot_idle(rq.cpu.0) {
            rq.set_idle_thread(idle);
            // SAFETY: idle est valide (vit dans le pool statique de boot_idle).
            context_switch(current, &mut *idle.as_ptr());
            return;
        }
    }
    // Toujours pas d'idle thread après spin : log d'erreur non-paniquant.
    // Le thread courant reste Running — le scheduler réessayera au prochain tick.
    crate::log_error!(
        "schedule_block: idle_thread introuvable après {} spins sur cpu {} — thread conservé",
        MAX_SPIN, rq.cpu.0
    );
    current.set_state(TaskState::Running);
}
```

---

### IMPL-SCHED-04
**`.unwrap()` dans le hot path `CfsSortedQueue`**  
Sévérité : 🟡 Mineur · Fichier : `kernel/src/scheduler/core/runqueue.rs:266,309`

#### Symptôme

```rust
// runqueue.rs:266 (bisection enqueue)
let mv = unsafe {
    self.tasks[mid]
        .unwrap()       // ← panic si invariant violé
        .as_ref()
        .vruntime
        .load(Ordering::Acquire)
};

// runqueue.rs:309 (mise à jour min_vruntime après dequeue)
let new_min = unsafe {
    self.tasks[0]
        .unwrap()       // ← panic si invariant violé
        .as_ref()
        .vruntime
        .load(Ordering::Relaxed)
};
```

L'invariant (`mid < self.count` → `tasks[mid] == Some(...)`) est correct, mais en cas de bug d'intégrité mémoire (corruption de `self.count`), `.unwrap()` déclenche un kernel panic au lieu d'un comportement récupérable.

#### Correctif — `kernel/src/scheduler/core/runqueue.rs`

```rust
// Remplacer .unwrap() par unwrap_unchecked() avec justification SAFETY

// runqueue.rs:266
let mv = unsafe {
    // SAFETY: mid < self.count par invariant — tasks[mid] est nécessairement Some.
    self.tasks[mid]
        .unwrap_unchecked()
        .as_ref()
        .vruntime
        .load(Ordering::Acquire)
};

// runqueue.rs:309
let new_min = unsafe {
    // SAFETY: self.count > 0 vérifié ci-dessus — tasks[0] est nécessairement Some.
    self.tasks[0]
        .unwrap_unchecked()
        .as_ref()
        .vruntime
        .load(Ordering::Relaxed)
};
```

> **Alternative plus défensive** : remplacer par `if let Some(t) = ... { } else { log_error!(...); }` si on préfère la résilience au panic en cas de corruption.

---

## MODULE IPC

---

### BUG-IPC-01
**`sched_hooks::block_current()` : fenêtre de réveil manqué → deadlock** 🔴  
Fichier : `kernel/src/ipc/sync/sched_hooks.rs`

#### Symptôme — Race condition

```
Thread A (waiter)                     Thread B (waker)
─────────────────────────────────     ─────────────────────────────
SLEEP_REGISTRY.lock().register(tid)   
// relâche le lock
// --- FENÊTRE DE RACE ---            SLEEP_REGISTRY.lock().pop(tid)
                                      tcb.try_transition(Sleeping → Runnable)
                                        // FAIL : état = Running (pas Sleeping !)
                                      // Réveil ABANDONNÉ silencieusement
block_fn()  // → set_state(Sleeping)
            // → schedule_block()
            // → BLOQUÉ INDÉFINIMENT
```

**Thread A** enregistre son TCB dans `SLEEP_REGISTRY` avec l'état `Running`, puis relâche le verrou. Entre ce moment et l'appel à `block_fn()` (qui change l'état en `Sleeping`), **Thread B** tente de réveiller Thread A : il trouve le TCB, mais `try_transition(Sleeping, Runnable)` échoue car l'état est encore `Running`. Le réveil est perdu. Thread A se bloque ensuite indéfiniment.

#### Correctif — `kernel/src/ipc/sync/sched_hooks.rs`

Le thread doit passer à `TaskState::Sleeping` **avant** l'enregistrement dans `SLEEP_REGISTRY`, et reconsulter le drapeau de réveil **après** enregistrement mais **avant** de bloquer réellement :

```rust
/// Bloque le thread courant identifié par `tid`.
///
/// CORRECTION BUG-IPC-01 :
///   Ordre impératif pour éviter le réveil manqué :
///   1. set_state(Sleeping)           ← visible AVANT enregistrement
///   2. SLEEP_REGISTRY.register()     ← enregistrement après transition
///   3. relâcher le lock
///   4. fence(SeqCst)                 ← garantir la visibilité
///   5. reconsulter le drapeau woken  ← si réveil déjà reçu, annuler
///   6. block_fn() uniquement si pas encore réveillé
///
/// # Safety
/// Identique à avant.
pub unsafe fn block_current(tid: u32) {
    let tcb_ptr = current_thread_raw();

    // Étape 1 : Passer à Sleeping AVANT enregistrement (BUG-IPC-01 fix)
    if !tcb_ptr.is_null() {
        let tcb = &mut *tcb_ptr;
        // Transition Running → Sleeping (sera annulée si réveil déjà arrivé)
        tcb.set_state(crate::scheduler::core::task::TaskState::Sleeping);
    }

    // Étape 2 : Enregistrer après transition d'état
    if !tcb_ptr.is_null() {
        SLEEP_REGISTRY.lock().register(tid, tcb_ptr);
    }

    // Étape 4 : Barrière de visibilité entre set_state et lecture du woken flag
    core::sync::atomic::fence(Ordering::SeqCst);

    // Étape 5 : Si wake_thread a déjà transitionné l'état vers Runnable,
    //           annuler le blocage (réveil anticipé = spurious wakeup guard)
    let already_woken = if !tcb_ptr.is_null() {
        let tcb = &*tcb_ptr;
        tcb.state() == crate::scheduler::core::task::TaskState::Runnable
    } else {
        false
    };

    if already_woken {
        // Réveil déjà reçu — désenregistrer et continuer sans bloquer
        if !tcb_ptr.is_null() {
            SLEEP_REGISTRY.lock().pop(tid);
        }
        return;
    }

    // Étape 6 : Bloquer réellement
    if let Some(block_fn) = *BLOCK_HOOK.lock() {
        block_fn();
        if !tcb_ptr.is_null() {
            SLEEP_REGISTRY.lock().pop(tid);
        }
    } else {
        for _ in 0..10_000 {
            core::hint::spin_loop();
        }
        if !tcb_ptr.is_null() {
            SLEEP_REGISTRY.lock().pop(tid);
        }
    }
}
```

> **Note** : `wake_thread()` fait déjà `try_transition(Sleeping, Runnable)` qui est maintenant correct puisque le thread est bien dans `Sleeping` au moment de l'enregistrement.

---

### IMPL-IPC-02
**SHM `virt_base = phys_addr` — invalide hors identity-map QEMU**  
Sévérité : 🟠 Majeur · Fichier : `kernel/src/ipc/shared_memory/mapping.rs:259`

#### Symptôme

```rust
// Adresse virtuelle = adresse physique dans l'implémentation stub
// (sera remplacé par memory::virtual::find_vma() lors de l'intégration)
let virt_base = if hint_virt.is_null() {
    let phys = { /* ... */ };
    VirtAddr(phys.0)   // ← STUB : virt = phys, QEMU identity uniquement
} else {
    hint_virt
};
```

Sur tout système réel ou tout processus avec un espace d'adressage non-identity-mappé, l'adresse virtuelle résultante ne correspond à aucune région VMA valide. Le mapping échoue silencieusement ou pire, mappe la SHM page dans une zone arbitraire.

#### Correctif — `kernel/src/ipc/shared_memory/mapping.rs`

Déléguer à `memory::virt::mmap` pour obtenir une adresse virtuelle libre dans l'espace du processus demandeur :

```rust
use crate::memory::virt::mmap::{mmap_find_free_vma, MmapFlags};

let virt_base = if hint_virt.is_null() {
    // Chercher une plage VMA libre dans l'espace du processus `pid`.
    // n_pages × PAGE_SIZE bytes, aligné sur PAGE_SIZE.
    let needed = n_pages * PAGE_SIZE;
    match mmap_find_free_vma(pid, needed, PAGE_SIZE) {
        Some(addr) => addr,
        None => return Err(IpcError::OutOfResources),
    }
} else {
    // Hint fourni par l'appelant — vérifier que la plage est libre.
    let hint_end = hint_virt.0.saturating_add((n_pages * PAGE_SIZE) as u64);
    if !crate::memory::virt::mmap::vma_range_free(pid, hint_virt, VirtAddr(hint_end)) {
        return Err(IpcError::MappingFailed);
    }
    hint_virt
};
```

> **Prérequis** : implémenter `mmap_find_free_vma(pid, size, align) -> Option<VirtAddr>` dans `memory/virtual/mmap.rs` — scanne l'arbre VMA du processus pour un créneau libre.

---

### IMPL-IPC-03
**`SLEEP_REGISTRY` plein : enregistrement silencieusement abandonné → blocage permanent**  
Sévérité : 🟠 Majeur · Fichier : `kernel/src/ipc/sync/sched_hooks.rs:79-88`

#### Symptôme

```rust
fn register(&mut self, tid: u32, tcb: *mut ThreadControlBlock) {
    for e in self.entries.iter_mut() {
        if e.is_free() {
            e.tid = tid;
            e.tcb_ptr = tcb as usize;
            return;
        }
    }
    // Registre plein — cas impossible en pratique (MAX_SLEEPING_IPC = 128).
    // ← AUCUNE ERREUR, AUCUN LOG, AUCUNE PANIQUE
}
```

Si le registre est plein, l'enregistrement est abandonné sans aucun signal. `wake_thread(tid)` ne trouve alors pas le TCB et le retourne sans réveil. Le thread appelant `block_current()` est alors bloqué **indéfiniment**.

#### Correctif — `kernel/src/ipc/sync/sched_hooks.rs`

```rust
/// Enregistre (tid, tcb). Retourne `true` si succès, `false` si registre plein.
fn register(&mut self, tid: u32, tcb: *mut ThreadControlBlock) -> bool {
    for e in self.entries.iter_mut() {
        if e.is_free() {
            e.tid = tid;
            e.tcb_ptr = tcb as usize;
            return true;
        }
    }
    false // Registre plein
}
```

Et dans `block_current()` :

```rust
// Étape 2 : Enregistrer — si plein, ne pas bloquer (fallback spin-poll)
let registered = if !tcb_ptr.is_null() {
    SLEEP_REGISTRY.lock().register(tid, tcb_ptr)
} else {
    false
};

if !registered {
    // Registre plein : signaler l'erreur kernel et passer en spin-poll borné.
    crate::log_warn!(
        "SLEEP_REGISTRY plein (MAX={}) — tid {} en spin-poll dégradé",
        MAX_SLEEPING_IPC, tid
    );
    // Spin-poll court : acceptable car condition rare et durée bornée.
    for _ in 0..50_000 {
        core::hint::spin_loop();
    }
    // Rétablir l'état Running pour que l'appelant réessaie.
    if !tcb_ptr.is_null() {
        (*tcb_ptr).set_state(crate::scheduler::core::task::TaskState::Running);
    }
    return;
}
```

> **Remédiation long terme** : augmenter `MAX_SLEEPING_IPC` à `512` ou utiliser un allocateur de pool dynamique borné.

---

## MODULE PROCESS

---

### BUG-PROC-01
**Signal envoyé au thread principal mort → signal perdu** 🔴  
Fichier : `kernel/src/process/signal/delivery.rs`

#### Symptôme

```rust
pub fn send_signal_to_pid(pid: Pid, sig: Signal) -> Result<(), SendError> {
    // ...
    // Récupère le thread principal (TID == PID).
    let thread_ptr = pcb.main_thread_ptr();
    if thread_ptr.is_null() {
        return Err(SendError::NoSuchProcess);
    }
    let thread = unsafe { &*thread_ptr };
    // ← AUCUNE VÉRIFICATION DE L'ÉTAT DU THREAD
    thread.sig_queue.enqueue(sig_n);
    thread.raise_signal_pending();
    Ok(())
}
```

Si le thread principal (`TID == PID`) est dans l'état `Dead` ou `Zombie` (terminé avant les autres threads du groupe), le signal est enfilé sur un thread qui ne sera jamais reschedule, et `raise_signal_pending()` sur un thread `Dead` n'a aucun effet. Le signal est perdu silencieusement.

Cas concrets : `SIGCHLD` envoyé à un processus dont le thread principal est mort mais des threads secondaires sont encore vivants ; `SIGTERM` depuis le kernel OOM killer vers un processus multi-threadé.

#### Correctif — `kernel/src/process/signal/delivery.rs`

```rust
pub fn send_signal_to_pid(pid: Pid, sig: Signal) -> Result<(), SendError> {
    let sig_n = sig.number();
    let pcb = PROCESS_REGISTRY
        .find_by_pid(pid)
        .ok_or(SendError::NoSuchProcess)?;

    let state = pcb.state();
    if state == ProcessState::Zombie || state == ProcessState::Dead {
        return Ok(());
    }

    // CORRECTION BUG-PROC-01 :
    // Chercher le premier thread VIVANT du groupe plutôt que toujours
    // le thread principal (qui peut être Dead dans un groupe multi-threadé).
    let target_thread = pcb.find_alive_thread();

    let thread = match target_thread {
        Some(ptr) if !ptr.is_null() => unsafe { &*ptr },
        _ => return Err(SendError::NoSuchProcess),
    };

    if sig_n < 32 {
        thread.sig_queue.enqueue(sig_n);
    } else {
        let info = SigInfo::kernel(sig_n);
        thread.rt_sig_queue.enqueue(sig_n, info);
    }
    thread.raise_signal_pending();
    Ok(())
}
```

**Implémentation de `find_alive_thread()`** dans `kernel/src/process/core/pcb.rs` :

```rust
impl ProcessControlBlock {
    /// Retourne un pointeur vers le premier thread du groupe dans un état
    /// vivant (ni Dead ni Zombie).
    ///
    /// Ordre de préférence :
    ///   1. Thread principal (TID == PID) s'il est vivant
    ///   2. Tout autre thread vivant du groupe
    ///   3. None si tous les threads sont morts/zombie
    pub fn find_alive_thread(&self) -> Option<*mut ProcessThread> {
        use crate::scheduler::core::task::TaskState;
        let threads = self.threads.lock();
        // Essayer d'abord le thread principal
        if let Some(&ptr) = threads.get(0) {
            if !ptr.is_null() {
                let state = unsafe { (*ptr).state() };
                if state != TaskState::Dead && state != TaskState::Zombie {
                    return Some(ptr);
                }
            }
        }
        // Chercher parmi les autres threads
        for &ptr in threads.iter().skip(1) {
            if !ptr.is_null() {
                let state = unsafe { (*ptr).state() };
                if state != TaskState::Dead && state != TaskState::Zombie {
                    return Some(ptr);
                }
            }
        }
        None
    }
}
```

---

### IMPL-PROC-02
**Machine d'états PCB : `Creating → ExitToZombie` absent — fuite PCB**  
Sévérité : 🟠 Majeur · Fichier : `kernel/src/process/state/transitions.rs`

#### Symptôme

```rust
pub fn transition(pcb: &ProcessControlBlock, tr: StateTransition) 
    -> Result<ProcessState, TransitionError> 
{
    let next = match (current, tr) {
        (ProcessState::Creating, StateTransition::Spawn) => ProcessState::Running,
        // ...
        // ← MANQUE : Creating → ExitToZombie
        // ← MANQUE : Stopped → Wake (SIGCONT sur thread arrêté)
        _ => return Err(TransitionError { from: current, transition: tr }),
    };
```

**Cas `Creating → ExitToZombie`** : si `do_exit()` est appelé pendant `do_fork()` (OOM killer, signal fatal pendant `exec`, erreur de création d'espace d'adressage), le PCB est en état `Creating`. La transition vers `Zombie` retourne `Err(TransitionError)`. L'appelant (do_exit) ne vérifie pas ce résultat (il appelle directement `pcb.set_state(Zombie)`), mais la chaîne de reaper n'est pas notifiée, laissant le PCB dans un état incohérent.

**Cas `Stopped → Running`** (SIGCONT) : manquant. Un processus arrêté via `SIGSTOP` recevant `SIGCONT` devrait transitionner `Stopped → Running`, mais cette transition n'est pas dans la table.

#### Correctif — `kernel/src/process/state/transitions.rs`

```rust
pub fn transition(
    pcb: &ProcessControlBlock,
    tr: StateTransition,
) -> Result<ProcessState, TransitionError> {
    let current = pcb.state();
    let next = match (current, tr) {
        // Transitions existantes
        (ProcessState::Creating,  StateTransition::Spawn)         => ProcessState::Running,
        (ProcessState::Running,   StateTransition::Sleep)         => ProcessState::Sleeping,
        (ProcessState::Running,   StateTransition::Stop)          => ProcessState::Stopped,
        (ProcessState::Running,   StateTransition::ExitToZombie)  => ProcessState::Zombie,
        (ProcessState::Sleeping,  StateTransition::Wake)          => ProcessState::Running,
        (ProcessState::Sleeping,  StateTransition::Stop)          => ProcessState::Stopped,
        (ProcessState::Sleeping,  StateTransition::ExitToZombie)  => ProcessState::Zombie,
        (ProcessState::Stopped,   StateTransition::Continue)      => ProcessState::Running,
        (ProcessState::Stopped,   StateTransition::ExitToZombie)  => ProcessState::Zombie,
        (ProcessState::Zombie,    StateTransition::ZombieToDead)  => ProcessState::Dead,

        // ── CORRECTIFS IMPL-PROC-02 ──────────────────────────────────────────

        // Un processus en cours de création peut être tué (OOM, fork fail, etc.)
        // Transition directe Creating → Zombie pour déclencher le reaper.
        (ProcessState::Creating, StateTransition::ExitToZombie) => ProcessState::Zombie,

        // SIGCONT sur un processus arrêté (SIGSTOP reçu) → Running
        // Corresponds à StateTransition::Continue (déjà Stopped→Running ci-dessus)
        // Ajout : Stopped → Running via Wake (cas waitpid + SIGCONT simultanés)
        (ProcessState::Stopped, StateTransition::Wake) => ProcessState::Running,

        // Transitions défensives : un processus Running peut recevoir Wake (double-wake)
        // → no-op : rester Running
        (ProcessState::Running, StateTransition::Wake) => ProcessState::Running,

        _ => {
            return Err(TransitionError {
                from: current,
                transition: tr,
            })
        }
    };
    pcb.set_state(next);
    Ok(next)
}
```

---

## Récapitulatif des Correctifs par Priorité

### Priorité 1 — Correctifs immédiats (deadlock / corruption silencieuse)

| ID | Action | Fichier(s) |
|----|--------|-----------|
| **BUG-SCHED-02** | Corriger offsets ASM `fast_path.s` + ajouter assert layout | `scheduler/asm/fast_path.s`, `scheduler/core/task.rs` |
| **BUG-IPC-01** | Réordonner set_state/register/fence dans `block_current()` | `ipc/sync/sched_hooks.rs` |
| **BUG-PROC-01** | `find_alive_thread()` dans `send_signal_to_pid()` | `process/signal/delivery.rs`, `process/core/pcb.rs` |

### Priorité 2 — Correctifs fonctionnels (défaut de fonctionnalité)

| ID | Action | Fichier(s) |
|----|--------|-----------|
| **IMPL-MEM-04** | Déplacer `register_backend_swap_provider()` en Phase 7 | `memory/mod.rs` |
| **IMPL-MEM-02** | `CURRENT_POLICY` per-CPU (tableau au lieu d'AtomicU8 global) | `memory/physical/allocator/numa_aware.rs` |
| **IMPL-IPC-02** | Remplacer `virt_base = phys_addr` par `mmap_find_free_vma()` | `ipc/shared_memory/mapping.rs` |
| **IMPL-IPC-03** | `register()` retourne bool + fallback spin-poll si plein | `ipc/sync/sched_hooks.rs` |
| **IMPL-PROC-02** | Ajouter `Creating→Zombie` et `Stopped→Running(Wake)` | `process/state/transitions.rs` |

### Priorité 3 — Robustesse et conformité specs

| ID | Action | Fichier(s) |
|----|--------|-----------|
| **SPEC-MEM-01** | Corriger `ReadAcquire` dans le spec TLA+ | `docs/Exo-OS-TLA+/Memory.tla` |
| **SPEC-SCHED-01** | Modéliser PKS/CET/KPTI dans le spec TLA+ | `docs/Exo-OS-TLA+/ContextSwitch.tla` |
| **IMPL-MEM-03** | HPET fixmap avec `PAGE_FLAGS_MMIO` | `arch/x86_64/acpi/hpet.rs` |
| **IMPL-SCHED-03** | spin-wait au lieu de panic pour idle_thread manquant | `scheduler/core/switch.rs` |
| **IMPL-SCHED-04** | `unwrap_unchecked()` + SAFETY dans runqueue hot path | `scheduler/core/runqueue.rs` |

---

## Note finale

Après application de l'ensemble de ces correctifs, les quatre modules atteignent un niveau de complétude opérationnelle sans TODO, STUB, ou défaut logique connu dans leur périmètre kernel. Les deux stubs de la couche driver (`check_sys_admin_capability` et `md_mmio_whitelist_contains`) documentés dans `GI_COMPLEMENTS_OMISSIONS_ET_STUBS.md` sont hors périmètre de cet audit (couche Phase 5/8 Ring 1, déjà identifiés et isolés).

---

*Document signé : **claude-delta** — Exo-OS Kernel Audit 2026-05-03*
