# Audit Froid — Modules `arch`, `memory`, `scheduler`
## ExoOS — Analyse cross-docs TLA+ + source Rust

**Auteur** : claude-alpha  
**Date** : 2026-05-03  
**Dépôt** : `https://github.com/darkfireeee/Exo-OS.git` (HEAD, cloné en direct)  
**Périmètre** : `kernel/src/arch/`, `kernel/src/memory/`, `kernel/src/scheduler/`, `libs/exo-phoenix-ssr/`, `kernel/src/exophoenix/`

---

## Méthodologie

Lecture ligne à ligne des fichiers source, croisement avec :

- `docs/kernel/arch/OVERVIEW.md`, `INIT.md`, `API.md`
- `docs/kernel/scheduler/SCHEDULER_CORE.md`
- `docs/kernel/memory/MEMORY_COMPLETE.md`
- `docs/Exo-OS-TLA+/Memory.tla`, `ContextSwitch.tla`
- Architecture canonique v7, Kernel Types v10, ExoPhoenix Spec v6

Chaque bug est classé **GRV** (crash garanti / corruption mémoire silencieuse) ou **SIL** (compile et tourne mais comportement erroné).

---

## Index des correctifs

| ID | Fichier | Sévérité | Titre |
|----|---------|----------|-------|
| ALPHA-01 | `arch/x86_64/idt.rs` | **GRV** | #DF déclaré TRAP\_GATE au lieu d'INTERRUPT\_GATE |
| ALPHA-02 | `arch/x86_64/tss.rs` | SIL | 48 KiB de piles IST allouées et jamais utilisées par CPU |
| ALPHA-03 | `scheduler/core/switch.rs` | **GRV** | `update_rsp0(next.kstack_ptr)` — RSP0 invalide pour IRQ Ring3 |
| ALPHA-04 | `scheduler/core/task.rs` + doc | SIL | `ThreadId(u64)` vs doc `ThreadId(u32)` — désynchronisation |
| ALPHA-05 | `scheduler/core/task.rs` + doc | SIL | `TaskState::try_transition()` documentée mais absente du code |
| ALPHA-06 | `scheduler/core/task.rs` + doc | SIL | `TaskState::Blocked` dans la doc vs `Sleeping`/`Uninterruptible` dans le code |
| ALPHA-07 | `scheduler/core/switch.rs` | SIL | Commentaire FFI `context_switch_asm` mentionne MXCSR/FCW alors que l'ASM n'y touche pas |
| ALPHA-08 | `scheduler/core/runqueue.rs` | **GRV** | Double décrémentation `nr_running` si `pick_next()` + `dequeue_highest_rt()` appelés sur le même thread |
| ALPHA-09 | `memory/physical/frame/emergency_pool.rs` | SIL | `SCHED_POOL_SIZE = 64` insuffisant face à `EMERGENCY_POOL_SIZE = 256` |
| ALPHA-10 | `exophoenix/forge.rs` | **GRV** | `Box::leak()` dans la boucle Forge — fuite mémoire physique non bornée |

---

## ALPHA-01 — IDT : #DF déclaré TRAP_GATE

**Fichier** : `kernel/src/arch/x86_64/idt.rs`  
**Ligne** : dans `init_idt()`, entrée `EXC_DOUBLE_FAULT`

### Problème

```rust
idt.set_handler(
    EXC_DOUBLE_FAULT,
    exc_double_fault_handler as *const () as u64,
    IST_DOUBLE_FAULT as u8 + 1,
    IdtEntryFlags::TRAP_GATE,   // ← BUG : TRAP_GATE laisse IF intact
);
```

`TRAP_GATE` (Type = 0xF) ne modifie **pas** le flag IF (Interrupt Flag) lors de l'entrée. Si une interruption survient pendant le handler de Double Fault, le CPU déclenchera un **Triple Fault** → reset machine instantané.

Le SDM Intel vol. 3A §6.15 prescrit que le handler #DF s'exécute en INTERRUPT\_GATE (Type = 0xE, IF=0).

### Correction

```rust
idt.set_handler(
    EXC_DOUBLE_FAULT,
    exc_double_fault_handler as *const () as u64,
    IST_DOUBLE_FAULT as u8 + 1,
    IdtEntryFlags::INTERRUPT_GATE,  // ← CORRECT : IF=0 pendant le handler
);
```

---

## ALPHA-02 — TSS : 48 KiB de piles IST mortes par CPU

**Fichier** : `kernel/src/arch/x86_64/tss.rs`

### Problème

`PerCpuStacks` déclare 7 piles de 16 KiB chacune :

```rust
struct PerCpuStacks {
    df_stack:   [u8; IST_STACK_SIZE],  // utilisé → IST4 (ist[3])
    nmi_stack:  [u8; IST_STACK_SIZE],  // ← JAMAIS assigné à aucun IST
    mc_stack:   [u8; IST_STACK_SIZE],  // utilisé → IST5 (ist[4])
    db_stack:   [u8; IST_STACK_SIZE],  // utilisé → IST6 (ist[5])
    ist5_stack: [u8; IST_STACK_SIZE],  // ← JAMAIS assigné
    ist6_stack: [u8; IST_STACK_SIZE],  // ← JAMAIS assigné
    ist7_stack: [u8; IST_STACK_SIZE],  // utilisé → IST7 (ist[6])
}
```

`init_tss_for_cpu()` n'assigne **jamais** `nmi_stack`, `ist5_stack` ni `ist6_stack` à un slot IST. La NMI utilise `nmi_fallback_top` depuis `EARLY_IST_POOL`.

Sur 256 CPUs : **3 × 16 KiB × 256 = 12 MiB de BSS gaspillés**.

### Correction

Supprimer les champs morts et renommer pour correspondre aux slots réellement utilisés :

```rust
struct PerCpuStacks {
    ist4_df_stack:  [u8; IST_STACK_SIZE],  // IST4 = Double Fault
    ist5_mc_stack:  [u8; IST_STACK_SIZE],  // IST5 = Machine Check
    ist6_db_stack:  [u8; IST_STACK_SIZE],  // IST6 = Debug
    ist7_rsv_stack: [u8; IST_STACK_SIZE],  // IST7 = réserve
}
```

Adapter les références dans `init_tss_for_cpu()` en conséquence.

---

## ALPHA-03 — Context Switch : RSP0 invalide pour IRQ Ring3

**Fichier** : `kernel/src/scheduler/core/switch.rs`  
**Ligne** : `context_switch()`, step post-switch

### Problème

```rust
// Dans context_switch(), après l'ASM :
unsafe {
    tss::update_rsp0(next.current_cpu().0 as usize, next.kstack_ptr);
    //                                               ^^^^^^^^^^^^^^
    //  kstack_ptr = RSP sauvegardé lors du switch précédent de `next`
    //             = MILIEU de la pile kernel, pas le SOMMET
}
```

`next.kstack_ptr` contient le RSP au moment où `next` a précédemment appelé `context_switch_asm()` — c'est-à-dire une position **dans** la pile kernel avec 48 octets de registres callee-saved empilés en-dessous.

Quand `next` s'exécute en Ring3 et qu'une interruption matérielle se produit :
- Le CPU charge RSP = RSP0 = `next.kstack_ptr` (valeur sous-estimée)
- Le handler ISR empile son frame en-dessous de RSP0
- Si `next` était dans le kernel et que RSP actuel > RSP0 : **overlap et corruption du frame du thread**

### Racine du problème

Le TCB ne stocke pas le sommet initial de la pile kernel (`kstack_top`). Il n'existe que `kstack_ptr` qui représente le pointeur sauvegardé courant.

### Correction

Ajouter un champ `kstack_top: u64` au TCB (offset libre dans `_cold_reserve`) initialisé à la création du thread et **jamais modifié** :

```rust
// Dans task.rs — à placer dans la zone cold_reserve [144..232]
// À l'offset [144] par exemple (premier u64 libre de _cold_reserve)
pub kstack_top: u64,   // [144] Sommet initial de la pile kernel — INVARIANT
```

Dans `context_switch()` :

```rust
unsafe {
    // CORR-ALPHA-03 : utiliser le sommet initial, pas le RSP sauvegardé
    tss::update_rsp0(next.current_cpu().0 as usize, next.kstack_top);
    percpu::set_kernel_rsp(next.kstack_top);
}
```

Dans la création de thread (`create_kthread`, `do_fork`) :

```rust
tcb.kstack_top = kernel_stack_top_addr;  // top fixe pour toute la vie du thread
tcb.kstack_ptr = kernel_stack_top_addr - INITIAL_FRAME_SIZE;  // frame initial
```

> **Impact layout TCB** : `kstack_top` peut occuper l'offset [144] de `_cold_reserve`
> (actuellement `[u8; 88]`). Le tableau se réduit à `[u8; 80]`.
> Taille TCB = 256 octets INCHANGÉE. Vérification compile-time existante maintenue.

---

## ALPHA-04 — ThreadId : `u64` dans le code, `u32` dans la doc

**Fichier** : `kernel/src/scheduler/core/task.rs`  
**Doc** : `docs/kernel/scheduler/SCHEDULER_CORE.md`

### Problème

```rust
// task.rs (source de vérité)
pub struct ThreadId(pub u64);  // ← u64
```

```markdown
<!-- SCHEDULER_CORE.md -->
pub struct ThreadId(pub u32);  // ← u32 (FAUX)
```

L'architecture v7 §3.2 spécifie 64 bits pour les TIDs. La doc est en retard.

### Correction dans la doc

```markdown
pub struct ThreadId(pub u64);  // identifiant unique 64-bit
```

---

## ALPHA-05 — `TaskState::try_transition()` absente du code

**Fichier** : `kernel/src/scheduler/core/task.rs`  
**Doc** : `docs/kernel/scheduler/SCHEDULER_CORE.md`

### Problème

La doc décrit :
```
Transitions atomiques via task.try_transition(from, to) (compare-and-swap).
```

Cette méthode **n'existe pas** dans `task.rs`. Les transitions d'état se font
directement via `sched_state` bits + `set_state()`.

### Correction

Implémenter la méthode dans `ThreadControlBlock` :

```rust
/// Tente une transition d'état atomique (CAS sur sched_state bits [7:0]).
/// Retourne `true` si la transition a réussi, `false` si l'état courant
/// ne correspondait pas à `from`.
#[inline(always)]
pub fn try_transition(&self, from: TaskState, to: TaskState) -> bool {
    let from_bits = from as u64;
    let to_bits   = to as u64;
    // Lire, masquer les bits [7:0], comparer et remplacer.
    let mut old = self.sched_state.load(Ordering::Acquire);
    loop {
        if (old & SCHED_STATE_MASK) != from_bits {
            return false;
        }
        let new = (old & !SCHED_STATE_MASK) | to_bits;
        match self.sched_state.compare_exchange_weak(
            old, new,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_)    => return true,
            Err(cur) => old = cur,
        }
    }
}
```

---

## ALPHA-06 — `TaskState::Blocked` dans la doc, inexistant dans le code

**Fichier** : `kernel/src/scheduler/core/task.rs`  
**Doc** : `docs/kernel/scheduler/SCHEDULER_CORE.md`

### Problème

La doc mentionne l'état `Blocked` :
```
Blocked,   // En attente d'un événement (wait_queue)
```

Le code définit :
```rust
pub enum TaskState {
    Runnable = 0,
    Running = 1,
    Sleeping = 2,        // équivalent POSIX de "interruptible sleep"
    Uninterruptible = 3, // équivalent de POSIX SIGKILL-resistant wait
    Stopped = 4,
    Zombie = 5,
    Dead = 6,
}
```

`Blocked` n'existe pas. Les docs doivent être mises à jour.

### Correction dans la doc

```markdown
| État | Valeur | Description |
|------|--------|-------------|
| `Runnable`         | 0 | Prêt, dans la run queue |
| `Running`          | 1 | En exécution sur un CPU |
| `Sleeping`         | 2 | Bloqué, interruptible (signal réveille) |
| `Uninterruptible`  | 3 | Bloqué, non interruptible (I/O critique) |
| `Stopped`          | 4 | Stoppé (SIGSTOP) |
| `Zombie`           | 5 | Terminé, en attente de reap |
| `Dead`             | 6 | Totalement terminé |
```

---

## ALPHA-07 — Commentaire FFI `context_switch_asm` faux (MXCSR/FCW)

**Fichier** : `kernel/src/scheduler/core/switch.rs`

### Problème

```rust
extern "C" {
    /// Context switch ASM complet.
    ///
    /// Sauvegarde les registres callee-saved (rbx, rbp, r12-r15) + MXCSR + x87 FCW
    ///                                                              ^^^^^^^^^^^^^^^^^^^
    ///                                                              MENSONGE — l'ASM n'y touche pas
    fn context_switch_asm(old_kernel_rsp: *mut u64, new_kernel_rsp: u64, new_cr3: u64);
}
```

Le fichier `switch_asm.s` ne touche **pas** MXCSR ni x87 FCW. Ces états sont gérés par `scheduler/fpu/save_restore.rs` via XSAVE/XRSTOR, conformément à la règle V7-C-02.

Ce commentaire trompeur peut pousser un futur développeur à supprimer le chemin XSAVE, croyant l'ASM déjà en charge.

### Correction

```rust
extern "C" {
    /// Context switch ASM — sauvegarde/restauration minimaliste (V7-C-02).
    ///
    /// Sauvegarde UNIQUEMENT les 6 registres callee-saved ABI System V AMD64 :
    ///   rbx, rbp, r12, r13, r14, r15
    ///
    /// NE touche PAS : MXCSR, x87 FCW, registres XMM/YMM/ZMM.
    /// L'état FPU complet est géré exclusivement par `scheduler::fpu::save_restore`
    /// via les instructions XSAVE / XRSTOR (appelées AVANT l'ASM, RÈGLE SWITCH-02).
    ///
    /// Si `new_cr3 != 0`, commute CR3 avant de restaurer le nouveau contexte (KPTI).
    fn context_switch_asm(old_kernel_rsp: *mut u64, new_kernel_rsp: u64, new_cr3: u64);
}
```

---

## ALPHA-08 — Double décrémentation `nr_running` dans RunQueue

**Fichier** : `kernel/src/scheduler/core/runqueue.rs`

### Problème

Deux méthodes publiques décrémantent `nr_running` pour un thread RT :

**`pick_next()`** (ligne dans la branche RT) :
```rust
if self.rt.count > 0 {
    self.stats.picks_rt.fetch_add(1, Ordering::Relaxed);
    let tcb = self.rt.dequeue_highest()?;
    self.stats.nr_running.fetch_sub(1, Ordering::Relaxed); // ← décrément #1
    return Some(tcb);
}
```

**`dequeue_highest_rt()`** (méthode publique exposée) :
```rust
pub fn dequeue_highest_rt(&mut self) -> Option<NonNull<ThreadControlBlock>> {
    let tcb = self.rt.dequeue_highest()?;
    let prev = self.stats.nr_running.fetch_sub(1, Ordering::Relaxed); // ← décrément #2
    self.update_load_avg(prev.saturating_sub(1) as u64);
    Some(tcb)
}
```

Si le load balancer (`smp/load_balance.rs`) appelle `dequeue_highest_rt()` au lieu de `pick_next()`, et que `pick_next()` est aussi appelé dans la même fenêtre, `nr_running` sera sous-évalué de 1. Cela fausse le load balancing SMP.

De plus, `rt.dequeue_highest()` est appelé des deux côtés : une fois `rt.count` est décrémenté dans `RtRunQueue::dequeue_highest`, et une seconde fois `nr_running` est décrémenté dans la RunQueue. Les deux chemins sont redondants.

### Correction

**Option A** (préférée) : supprimer `dequeue_highest_rt()` et forcer l'usage de `pick_next()`.

**Option B** : faire en sorte que `pick_next()` appelle `dequeue_highest_rt()` en interne :

```rust
pub fn pick_next(&mut self) -> Option<NonNull<ThreadControlBlock>> {
    self.stats.picks_total.fetch_add(1, Ordering::Relaxed);

    // 1. RT (O(1) via bitmap)
    if self.rt.count > 0 {
        self.stats.picks_rt.fetch_add(1, Ordering::Relaxed);
        return self.dequeue_highest_rt();  // ← délégation unique, nr_running décrémenté une seule fois
    }
    // ... DL, CFS, idle ...
}
```

---

## ALPHA-09 — `SCHED_POOL_SIZE = 64` inférieur à `EMERGENCY_POOL_SIZE = 256`

**Fichier** : `kernel/src/memory/physical/frame/emergency_pool.rs`

### Problème

```rust
const SCHED_POOL_SIZE: usize = 64;   // ← blocs pour le scheduler
```

```rust
// memory/core/constants.rs
pub const EMERGENCY_POOL_SIZE: usize = 256; // ← WaitNodes pour les wait queues générales
```

Le `SchedNodePool` fournit des blocs 64 B que le scheduler utilise pour ses `WaitNode`. Avec 256 CPUs et plusieurs wait queues simultanées (IPC, futex, mutex, condvar), 64 blocs peuvent être épuisés.

En cas d'épuisement, `emergency_pool_alloc_wait_node()` retourne `null`, ce qui provoque (selon le code appelant `wait_queue.rs`) soit un spin-wait infini, soit un panic, soit une corruption silencieuse.

### Correction

```rust
/// CORR-ALPHA-09 : doit être ≥ EMERGENCY_POOL_SIZE pour les scénarios haute densité.
/// Sur 256 CPUs avec 1+ wait par CPU : minimum 256 blocs.
const SCHED_POOL_SIZE: usize = 256;

// La contrainte du bitset AtomicU64 (max 64 bits) nécessite un bitmap étendu :
struct SchedNodePool {
    blocks:    UnsafeCell<[RawBlock64; SCHED_POOL_SIZE]>,
    free_bits: [AtomicU64; 4],  // 4 × 64 = 256 bits → 256 blocs
    initialized: AtomicBool,
    alloc_count: AtomicUsize,
    exhausted:   AtomicUsize,
}
```

Les méthodes `alloc()` et `free()` doivent être adaptées pour itérer sur les 4 mots de `free_bits`.

---

## ALPHA-10 — `Box::leak()` dans la boucle Forge : fuite mémoire non bornée

**Fichier** : `kernel/src/exophoenix/forge.rs`

### Problème

```rust
fn load_a_image_from_exofs() -> Result<&'static [u8], ForgeError> {
    let blob_id = BlobId(A_IMAGE_HASH);
    let data = BLOB_CACHE
        .get(&blob_id)
        .ok_or(ForgeError::ExoFsLoadFailed)?;
    let leaked: &'static mut [u8] = alloc::boxed::Box::leak(data);  // ← FUITE
    Ok(leaked)
}
```

`Box::leak()` transforme le `Box<[u8]>` en `&'static mut [u8]` en **ne libérant jamais** la mémoire allouée. Si ExoPhoenix réalise N cycles de Forge (reconstruction après N détections de compromission), N × `|image_A|` octets sont perdus définitivement dans le heap kernel.

Sur une image kernel A typique de ~4 MiB : après 10 reconstructions → **40 MiB perdus**.

### Correction

Conserver l'image dans un buffer statique réutilisable :

```rust
/// Buffer statique pour l'image Kernel A — réutilisé à chaque Forge.
/// RÈGLE FORGE-01 : accès sérialisé par le verrou Phoenix (un seul Forge actif à la fois).
static mut FORGE_IMAGE_BUFFER: Option<alloc::vec::Vec<u8>> = None;

fn load_a_image_from_exofs() -> Result<&'static [u8], ForgeError> {
    let blob_id = BlobId(A_IMAGE_HASH);
    let data: alloc::vec::Vec<u8> = BLOB_CACHE
        .get_owned(&blob_id)
        .ok_or(ForgeError::ExoFsLoadFailed)?;

    // SAFETY: accès sérialisé — un seul Forge actif à la fois (RÈGLE FORGE-01).
    // Le buffer précédent est libéré et remplacé.
    let buf = unsafe {
        FORGE_IMAGE_BUFFER = Some(data);
        FORGE_IMAGE_BUFFER.as_ref().unwrap().as_slice()
    };
    // SAFETY: la référence est valide pendant toute la durée du cycle Forge courant.
    // Le buffer ne sera remplacé qu'au prochain appel, après la fin de ce cycle.
    Ok(unsafe { core::mem::transmute::<&[u8], &'static [u8]>(buf) })
}
```

Alternativement, si `BLOB_CACHE` peut retourner un `Arc<[u8]>` ou un type Copy-on-map, préférer une approche sans transmute.

---

## Récapitulatif de criticité

| ID | Type | Impact |
|----|------|--------|
| ALPHA-01 | **GRV** | Triple Fault (reset machine) si IRQ pendant #DF handler |
| ALPHA-02 | SIL | Gaspillage BSS ~12 MiB (256 CPUs) |
| ALPHA-03 | **GRV** | Corruption de pile potentielle si IRQ Ring3 post-switch |
| ALPHA-04 | SIL | Désynchronisation doc/code (TID) |
| ALPHA-05 | SIL | API documentée inexistante → code appelant impossible |
| ALPHA-06 | SIL | Désynchronisation doc/code (TaskState) |
| ALPHA-07 | SIL | Commentaire trompeur FPU — risque de régression future |
| ALPHA-08 | **GRV** | `nr_running` corrompu → load balancing SMP aveugle |
| ALPHA-09 | SIL | SchedNodePool épuisé sur charge haute → spin infini ou panic |
| ALPHA-10 | **GRV** | Fuite mémoire non bornée dans la boucle ExoPhoenix Forge |

---

*— claude-alpha, audit statique direct sur sources clonées*
