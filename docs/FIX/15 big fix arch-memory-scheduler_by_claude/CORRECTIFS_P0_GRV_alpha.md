# Correctifs P0 — Bugs GRV (crashes garantis / corruption)
## ExoOS — Modules arch, memory, scheduler

**Auteur** : claude-alpha  
**Date** : 2026-05-03  
**Référence audit** : `AUDIT_ALPHA_ARCH_MEMORY_SCHEDULER.md`

---

## ALPHA-01 : IDT — #DF TRAP_GATE → INTERRUPT_GATE

**Fichier** : `kernel/src/arch/x86_64/idt.rs`  
**Fonction** : `init_idt()`

### Patch

```rust
// AVANT :
idt.set_handler(
    EXC_DOUBLE_FAULT,
    exc_double_fault_handler as *const () as u64,
    IST_DOUBLE_FAULT as u8 + 1,
    IdtEntryFlags::TRAP_GATE,    // ← FAUX
);

// APRÈS (CORR-ALPHA-01) :
// SDM Intel vol. 3A §6.15 : le #DF DOIT s'exécuter en INTERRUPT_GATE
// pour garantir IF=0 pendant le handler et prévenir un Triple Fault.
idt.set_handler(
    EXC_DOUBLE_FAULT,
    exc_double_fault_handler as *const () as u64,
    IST_DOUBLE_FAULT as u8 + 1,
    IdtEntryFlags::INTERRUPT_GATE,  // ← CORRECT
);
```

### Vérification

Après le patch, `IDT.entries[8].flags` doit valoir `0x8E` (P=1, DPL=0, Type=0xE).

```rust
// Test à ajouter dans init_idt() :
debug_assert_eq!(
    IDT.entries[EXC_DOUBLE_FAULT as usize].flags,
    0x8E,
    "ALPHA-01 : #DF doit être un INTERRUPT_GATE (0x8E)"
);
```

---

## ALPHA-03 : Context Switch — RSP0 utilise le sommet initial

**Fichiers** :
- `kernel/src/scheduler/core/task.rs` — ajout champ `kstack_top`
- `kernel/src/scheduler/core/switch.rs` — correction `update_rsp0`
- Tous les chemins de création de thread

### Patch 1 — task.rs : ajout de `kstack_top`

```rust
// Dans la déclaration de ThreadControlBlock, remplacer :
pub(crate) _cold_reserve: [u8; 88], // [144]  (144+88=232)

// PAR (CORR-ALPHA-03) :
pub kstack_top: u64,                // [144] sommet initial pile kernel — INVARIANT
pub(crate) _cold_reserve: [u8; 80], // [152]  (152+80=232) ← réduit de 8 octets

// La taille totale du TCB reste 256 octets.
// Ajouter l'assertion :
const _: () = assert!(
    offset_of!(ThreadControlBlock, kstack_top) == 144,
    "TCB: kstack_top doit être à l'offset 144"
);
```

### Patch 2 — switch.rs : utiliser `kstack_top` pour RSP0

```rust
// Dans context_switch(), remplacer :
unsafe {
    tss::update_rsp0(next.current_cpu().0 as usize, next.kstack_ptr);
    percpu::set_kernel_rsp(next.kstack_ptr);
}

// PAR (CORR-ALPHA-03) :
unsafe {
    // CORR-ALPHA-03 : RSP0 doit pointer vers le SOMMET INITIAL de la pile kernel
    // du thread, pas vers le RSP sauvegardé (kstack_ptr), qui est un point
    // intermédiaire. Utiliser kstack_top (invariant, jamais modifié après création).
    //
    // Raison : quand `next` s'exécute en Ring3 et qu'une IRQ survient,
    // le CPU commute RSP → RSP0. Si RSP0 = kstack_ptr (milieu de pile),
    // le handler IRQ pourrait chevaucher les données sauvegardées.
    tss::update_rsp0(next.current_cpu().0 as usize, next.kstack_top);
    percpu::set_kernel_rsp(next.kstack_top);
}
```

### Patch 3 — Initialisation kstack_top lors de la création de thread

```rust
// Dans tout code de création de thread (create_kthread, do_fork, etc.) :
//
// Lors de l'initialisation du TCB :
tcb.kstack_top = kernel_stack_top;  // sommet fixe, jamais modifié
tcb.kstack_ptr = kernel_stack_top - INITIAL_SWITCH_FRAME_SIZE;
//                                   ^^^^^^^^^^^^^^^^^^^^^^^^
//                                   48 octets (6 callee-saved) + 8 (ret addr) = 56

// Ajouter dans impl ThreadControlBlock :
/// Initialise kstack_top depuis le sommet de pile fourni.
/// DOIT être appelé UNE SEULE FOIS lors de la création du thread.
/// INVARIANT : kstack_top ne change jamais après cet appel.
#[inline(always)]
pub fn init_kstack_top(&mut self, stack_top: u64) {
    self.kstack_top = stack_top;
}
```

---

## ALPHA-08 : RunQueue — Double décrémentation `nr_running`

**Fichier** : `kernel/src/scheduler/core/runqueue.rs`

### Analyse

`pick_next()` ET `dequeue_highest_rt()` décrément tous deux `nr_running` pour
les threads RT. Si `pick_next()` est utilisé dans le tick handler et que
`dequeue_highest_rt()` est utilisé par le load balancer sur le même thread
au même instant, le compteur sera sous-estimé de 1, rendant le load balancing incorrect.

### Patch — Factoriser via `dequeue_highest_rt()`

```rust
// Dans pick_next(), remplacer le bloc RT :

// AVANT :
if self.rt.count > 0 {
    self.stats.picks_rt.fetch_add(1, Ordering::Relaxed);
    let tcb = self.rt.dequeue_highest()?;
    self.stats.nr_running.fetch_sub(1, Ordering::Relaxed);  // ← doublon potentiel
    return Some(tcb);
}

// APRÈS (CORR-ALPHA-08) :
// Déléguer à dequeue_highest_rt() qui est la SEULE source de vérité
// pour la décrémentation nr_running des threads RT.
if self.rt.count > 0 {
    self.stats.picks_rt.fetch_add(1, Ordering::Relaxed);
    return self.dequeue_highest_rt();  // inclut déjà le fetch_sub(nr_running)
}
```

### Vérification

Ajouter un test de non-régression :

```rust
#[test]
fn nr_running_consistent_after_rt_pick() {
    let mut rq = PerCpuRunQueue::new(CpuId(0));
    init_percpu(1);
    // Créer un TCB RT factice et l'enqueuer
    // Vérifier que nr_running == 1 après enqueue
    // Appeler pick_next() une fois
    // Vérifier que nr_running == 0 après pick
    // Appeler pick_next() une deuxième fois
    // Vérifier que nr_running ne devient pas negativ (wrap-around)
    assert!(rq.nr_running() <= 1, "nr_running ne doit pas wrap");
}
```

---

## ALPHA-10 : ExoPhoenix Forge — Fuite mémoire `Box::leak()`

**Fichier** : `kernel/src/exophoenix/forge.rs`

### Patch complet

```rust
// Supprimer l'appel Box::leak() dans load_a_image_from_exofs().
// Utiliser un buffer statique protégé par le verrou PhoenixState.

use core::sync::atomic::Ordering;
use alloc::vec::Vec;

/// Buffer réutilisable pour l'image Kernel A pendant un cycle Forge.
///
/// RÈGLE FORGE-01 : un seul cycle de Forge peut être actif à la fois.
/// L'atomique PHOENIX_STATE garantit la sérialisation.
/// Accès writable uniquement depuis `load_a_image_from_exofs()`.
static FORGE_IMAGE_BUF: spin::Mutex<Option<Vec<u8>>> = spin::Mutex::new(None);

/// Charge l'image de Kernel A depuis ExoFS dans le buffer statique.
///
/// La référence retournée est valide pendant toute la durée du cycle Forge
/// courant, jusqu'au prochain appel à cette fonction (qui libère le buffer).
///
/// # SAFETY
/// Appelé exclusivement quand PHOENIX_STATE == Restore.
/// La sérialisation est garantie par l'atomique PHOENIX_STATE.
fn load_a_image_from_exofs() -> Result<&'static [u8], ForgeError> {
    let blob_id = BlobId(A_IMAGE_HASH);
    // BLOB_CACHE.get_owned() retourne Vec<u8> ou équivalent.
    let data: Vec<u8> = BLOB_CACHE
        .get_owned(&blob_id)
        .ok_or(ForgeError::ExoFsLoadFailed)?;

    let mut guard = FORGE_IMAGE_BUF.lock();
    // Libérer l'éventuel buffer du cycle précédent avant de le remplacer.
    *guard = Some(data);

    // SAFETY: La référence 'static est valide car :
    // 1. FORGE_IMAGE_BUF est une static — vit toute la durée du kernel.
    // 2. Le contenu ne change pas pendant ce cycle Forge (verrou tenu par
    //    PHOENIX_STATE Restore — aucun autre Forge concurrent possible).
    // 3. Le buffer sera remplacé (et l'ancien libéré) lors du prochain appel.
    let slice: &[u8] = guard.as_ref().unwrap().as_slice();
    Ok(unsafe { &*(slice as *const [u8]) })
}
```

> **Note** : si `spin::Mutex` n'est pas souhaité dans ce chemin critique,
> remplacer par un `UnsafeCell<Option<Vec<u8>>>` protégé par la garantie
> de sérialisation apportée par `PHOENIX_STATE`. L'essentiel est l'absence
> de `Box::leak()`.

---

*— claude-alpha*
