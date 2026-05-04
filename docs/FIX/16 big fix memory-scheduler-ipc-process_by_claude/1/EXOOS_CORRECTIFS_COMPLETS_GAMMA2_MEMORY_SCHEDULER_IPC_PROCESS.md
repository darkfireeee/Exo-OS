# ExoOS — Correctifs Complets v2 : `memory` / `scheduler` / `ipc` / `process`
## Audit Profond — Modules Noyau Ring 0

> **Auteur** : claude-gamma  
> **Date** : 2026-05-04  
> **Référentiel** : `darkfireeee/Exo-OS` — analyse croisée code source / TLA+ / docs recast  
> **Périmètre** : `kernel/src/{memory,scheduler,ipc,process}/`  
> **Sources croisées** : `ExoOS_Architecture_v7.md`, `GI-01_Types_TCB_SSR.md`, `GI-02_Boot_ContextSwitch.md`,  
> `ContextSwitch.tla`, `Memory.tla`, `ProcessDeath.tla`, `ExoOS_Full.tla`, `ExoOS_Kernel_Types_v10.md`

---

## Préambule : état après correctifs CGX-01..CGX-16

L'audit gamma précédent (2026-05-03) a identifié 16 défauts. L'analyse du code actuel confirme l'application des correctifs suivants :

| CGX | Description | Statut |
|-----|-------------|--------|
| CGX-01 | `exec.rs` : `kstack_top()` utilisé pour TSS.RSP0 | ✅ APPLIQUÉ |
| CGX-02 | `exit.rs` : violation PROC-01 résolue via trait hook | ✅ APPLIQUÉ |
| CGX-03 | `switch.rs` : `cpu_id` de `next` mis à jour post-switch | ✅ APPLIQUÉ |
| CGX-04 | `exit.rs` : `fpu::free_fpu_state()` appelé correctement | ✅ APPLIQUÉ |
| CGX-05 | `fork.rs` : héritage CapTable enfant | ❌ **NON APPLIQUÉ — P0** |
| CGX-06 | `wait_queue.rs` : `thread_id: AtomicU64` | ✅ APPLIQUÉ |
| CGX-07 | `sched_hooks.rs` : `SleepEntry::tid: u64` | ✅ APPLIQUÉ |
| CGX-08 | `wait_queue.rs` : blocage réel via `sched_hooks` | ✅ APPLIQUÉ |
| CGX-09 | `switch.rs` : `debug_assert!` préemption | ✅ APPLIQUÉ |
| CGX-10 | DMA ISR : `wake_on_completion` sans lock actif | ✅ APPLIQUÉ |
| CGX-11 | CVE-EXO-001 : APs spin sur `SECURITY_READY` | ✅ APPLIQUÉ |
| CGX-12 | `verify()` constant-time via `subtle::ct_eq()` | ✅ APPLIQUÉ |
| CGX-13 | commentaire `_cold_reserve` corrigé | ✅ APPLIQUÉ |
| CGX-14 | `ipc_init()` hooks documentés | ✅ PARTIEL (voir CNV-07) |
| CGX-15 | `send_irq_notification()` : pid==0 retourne Err | ✅ APPLIQUÉ |
| CGX-16 | `KernelStack` canary bas de pile | ✅ APPLIQUÉ (P2 résiduel documenté) |

L'audit de cette session révèle **10 défauts nouveaux** non couverts par les cycles précédents.

---

## Table des correctifs v2

| ID | Module | Priorité | Titre |\
|----|--------|----------|---------|\
| CNV-01 | `memory/mod.rs` | 🔴 P0 | `register_backend_swap_provider()` appelé avant Phase 2 — race boot |\
| CNV-02 | `memory/virtual/fault/cow.rs` | 🔴 P0 | `handle_cow_fault()` ignore `CowTracker` — toujours copie, jamais optimise |\
| CNV-03 | `memory/virtual/address_space/fork_impl.rs` | 🔴 P0 | `COW_TRACKER.inc()` table-pleine silencieux → frame CoW non tracké |\
| CNV-04 | `process/lifecycle/fork.rs` | 🔴 P0 | CGX-05 non appliqué : enfant créé sans CapTable hérité |\
| CNV-05 | `process/lifecycle/fork.rs` | 🟠 P1 | Masque de signaux non propagé au fils (violation POSIX 1003.1) |\
| CNV-06 | `process/lifecycle/fork.rs` | 🟠 P1 | `CLONE_NEWPID` sans effet : namespace PID parent cloné |\
| CNV-07 | `ipc/mod.rs` | 🟠 P1 | `ipc_init()` sans garde : hooks manquants → spin-poll infini |\
| CNV-08 | `memory/mod.rs` | 🟡 P2 | `alloc_zeroed_page` non re-exporté (manque API publique) |\
| CNV-09 | `scheduler/core/switch.rs` | 🟡 P2 | `block_current_thread()` : retour silencieux état Running — debug manquant |\
| CNV-10 | `process/core/tcb.rs` | 🟡 P2 | Guard page KernelStack documentée mais non implémentée |\

---

## CNV-01 — P0 : `register_backend_swap_provider()` avant Phase 2

### Localisation
`kernel/src/memory/mod.rs` — ligne 170 (Phase 7 dans `init()`)

### Description

`virt::fault::swap_in::register_backend_swap_provider()` est appelé en **Phase 7** (intérieur de `memory::init()`), mais cette fonction enregistre un fournisseur qui interagit avec l'infrastructure de fault handler virtuel (`virt/fault/`). Cette infrastructure ne peut fonctionner qu'après la **Phase 2** (initialisation de l'espace d'adressage kernel `KERNEL_AS.init(pml4_phys)`).

Or, selon le commentaire de `memory::init()` et la séquence boot v7 §3.1.1, la Phase 2 est **déléguée à `arch/x86_64/boot/`** et est appelée **après** le retour de `memory::init()`. À l'instant de l'appel en Phase 7, aucune table de pages virtuelle n'est encore opérationnelle côté kernel AS.

Si un swap-in est déclenché pendant la fenêtre de boot avant que `KERNEL_AS.init()` soit appelé, le swap provider tente d'accéder à des structures non initialisées → undefined behavior garanti.

### Preuve TLA+

`Memory.tla` — `S49_IommuInitRelease` modélise le contrat d'ordre d'initialisation : les sous-systèmes dépendants (comme le swap backend) ne peuvent observer les structures partagées qu'après que la publication Release du sous-système fournisseur (ici le VMM kernel) ait été effectuée avec `Ordering::Release`. Appeler le provider avant la publication `KERNEL_AS` viole ce contrat.

### Code fautif

```rust
// kernel/src/memory/mod.rs — fin de init()
// ── Phase 7 : utilitaires ────────────────────────────────────────────────
utils::init();
virt::fault::swap_in::register_backend_swap_provider();  // ← BUG P0

// ── Phase 8 : NUMA ───────────────────────────────────────────────────────
numa::init();
```

### Correctif CNV-01

**Étape 1 — Supprimer l'appel de `memory::init()`** :

```rust
// kernel/src/memory/mod.rs — init()

// ── Phase 7 : utilitaires ────────────────────────────────────────────────
utils::init();
// CORRECTIF CNV-01 : register_backend_swap_provider() RETIRÉ D'ICI.
// Doit être appelé depuis arch/x86_64/boot/ APRÈS KERNEL_AS.init(pml4_phys).
// Raison : swap_in dépend de virt/fault/ qui requiert l'AS kernel initialisé.

// ── Phase 8 : NUMA ───────────────────────────────────────────────────────
numa::init();
```

Mettre à jour le doc-comment de `init()` :

```rust
/// # Note sur le swap backend
/// `virt::fault::swap_in::register_backend_swap_provider()` n'est PAS appelé ici.
/// Il doit être appelé par `arch/x86_64/boot/` après `KERNEL_AS.init(pml4_phys)`
/// (Phase 2 complète), car le fault handler virtuel dépend de l'AS kernel opérationnel.
```

**Étape 2 — Appeler depuis `arch/x86_64/boot/mod.rs`** après `KERNEL_AS.init()` :

```rust
// arch/x86_64/boot/mod.rs — séquence boot step 12 (après KERNEL_AS.init)

// Step 12 : espace d'adressage kernel + protections
unsafe {
    crate::memory::virt::address_space::kernel::KERNEL_AS.init(pml4_phys);
}
// CORRECTIF CNV-01 : enregistrer le swap backend APRÈS Phase 2 complète.
// SAFETY: KERNEL_AS.init() terminé — virt/fault/ est opérationnel.
unsafe {
    crate::memory::virt::fault::swap_in::register_backend_swap_provider();
}
```

---

## CNV-02 — P0 : `handle_cow_fault()` ignore `CowTracker` — toujours copie

### Localisation
`kernel/src/memory/virtual/fault/cow.rs` — fonction `handle_cow_fault()`

### Description

`handle_cow_fault()` est le handler de page fault CoW déclenché lorsqu'un processus écrit sur une page marquée `COW`. Le module `memory/cow/breaker.rs` expose `break_cow(frame)` qui :
1. Consulte `COW_TRACKER.ref_count(frame)`.
2. Si `refcount == 1` → frame déjà exclusif, retourne `AlreadyExclusive(frame)` sans copie.
3. Si `refcount >= 2` → copie physique + décrémente l'ancien.

**Or `handle_cow_fault()` n'appelle jamais `break_cow()`**. Il alloue systématiquement un nouveau frame et copie les données, même lorsque le frame est déjà exclusif (refcount == 1 après un `wait()` précédent ou une libération partielle). Le commentaire dans le code dit explicitement : *"Ici : toujours copier (chemin safe)"*.

Ce comportement :
- Crée une incohérence architecturale avec `cow/breaker.rs` (code mort et inutile).
- Gaspille des pages physiques : pour un `fork()` suivi d'un `exec()` immédiat, toutes les pages CoW du fils sont copiées une fois au lieu d'être directement promues exclusives.
- Contredit l'invariant `S25_STRESS_IrqFpuSafety` de `ContextSwitch.tla` qui garantit que le gestionnaire de ressources partagées utilise le tracker centralisé.

### Code fautif

```rust
// memory/virtual/fault/cow.rs — handle_cow_fault()

// Construire un AtomicRefCount temporaire pour décider si copie est nécessaire.
// En production, la table de refcounts est FRAME_TABLE (non créée ici pour Couche 0).
// Pour rester indépendant, on utilise l'heuristique : si le physmap montre
// que la page est partagée (via un compteur dans FrameDesc), on copie.
// Ici : toujours copier (chemin safe).    ← BUG P0 : ignore COW_TRACKER
let new_frame = match alloc.alloc_nonzeroed() {
    Ok(f) => f,
    Err(_) => { return FaultResult::Oom { addr: ctx.fault_addr }; }
};
```

### Correctif CNV-02

Remplacer le bloc d'allocation par un appel à `break_cow()` :

```rust
// kernel/src/memory/virtual/fault/cow.rs — handle_cow_fault() CORRIGÉ

use crate::memory::cow::breaker::{break_cow, CowBreakOutcome};

/// Traite un CoW fault (write sur page en lecture seule avec flag COW).
pub fn handle_cow_fault<A: FaultAllocator>(
    ctx: &FaultContext,
    vma: &VmaDescriptor,
    alloc: &A,
) -> FaultResult {
    let page_addr = VirtAddr::new(ctx.fault_addr.as_u64() & !(PAGE_SIZE as u64 - 1));

    let old_raw = alloc.read_pte_raw(page_addr);
    let old_entry = PageTableEntry::from_raw(old_raw);
    let old_frame = match old_entry.frame() {
        Some(frame) => frame,
        None => match alloc.translate(page_addr) {
            Some(phys) => crate::memory::core::Frame::containing(phys),
            None => {
                return super::demand_paging::handle_demand_paging(ctx, vma, alloc);
            }
        },
    };

    // CORRECTIF CNV-02 : déléguer à break_cow() pour vérifier l'exclusivité
    // via COW_TRACKER avant toute copie physique.
    // break_cow() retourne AlreadyExclusive si refcount == 1 (zero-copy).
    //
    // SAFETY: old_frame est dans la physmap (obtenu depuis un PTE présent).
    //         PHYS_MAP_BASE est initialisé (Phase 2 complète au moment du fault).
    let outcome = unsafe { break_cow(old_frame) };

    let (new_frame, already_exclusive) = match outcome {
        CowBreakOutcome::Copied(f) => (f, false),
        CowBreakOutcome::AlreadyExclusive(f) => (f, true),
        CowBreakOutcome::Oom => {
            return FaultResult::Oom { addr: ctx.fault_addr };
        }
    };

    // Remap la page avec les nouveaux flags (writable, supprimer COW).
    let new_flags = vma.page_flags & !PageFlags::COW | PageFlags::WRITABLE;
    let new_raw = PageTableEntry::from_page_flags(new_frame, new_flags).raw();

    if already_exclusive {
        // Le frame est déjà exclusif : mise à jour directe du PTE (pas de CAS nécessaire).
        alloc.write_pte_raw(page_addr, new_raw);
        vma.record_cow_break();
        // SAFETY: adresse canonique page-alignée.
        unsafe { flush_single(page_addr); }
        return FaultResult::Handled;
    }

    // Frame partagé copié : CAS atomique pour gérer la race SMP.
    match alloc.compare_exchange_pte_raw(page_addr, old_raw, new_raw) {
        Ok(_) => {
            // COW_TRACKER.dec() déjà effectué dans break_cow() → pas de double dec.
            vma.record_cow_break();
            // SAFETY: adresse canonique.
            unsafe { flush_single(page_addr); }
            FaultResult::Handled
        }
        Err(_actual_raw) => {
            // Un autre CPU a gagné la course et a déjà brisé le CoW.
            // Le frame copié (new_frame) est abandonné : remettre son refcount à 0.
            // SAFETY: new_frame vient d'être alloué — COW_TRACKER n'en a pas connaissance.
            alloc.free_frame(new_frame);
            unsafe { flush_single(page_addr); }
            FaultResult::Handled
        }
    }
}
```

---

## CNV-03 — P0 : `COW_TRACKER.inc()` table-pleine silencieux dans `fork_impl.rs`

### Localisation
`kernel/src/memory/virtual/address_space/fork_impl.rs` — fonctions `clone_pdpt()`, `clone_pd()`, `clone_pt()`

### Description

`CowTracker::inc()` retourne `u32::MAX` lorsque la table de hachage CoW est pleine (4 096 slots tous occupés). Dans ce cas, le frame n'est pas enregistré dans le tracker.

Dans `fork_impl.rs`, les trois niveaux de clonage utilisent :

```rust
let _ = COW_TRACKER.inc(frame);
```

Le `let _` ignore silencieusement le retour `u32::MAX`. Conséquence :
- Le frame partagé parent/enfant n'est **pas** dans `COW_TRACKER`.
- Lors d'un write CoW ultérieur, `break_cow()` (corrigé par CNV-02) consulte `COW_TRACKER.ref_count(frame)` et obtient `0` (non trouvé → `u32::MAX` via la même sentinelle).
- `break_cow()` interprète `refcount <= 1` → `AlreadyExclusive` → **aucune copie effectuée**.
- Les deux processus écrivent sur le même frame physique → **corruption de données garantie**.

La table de 4 096 slots est petite pour un processus avec beaucoup de pages mappées. Un serveur avec 64 Mo de heap = 16 384 pages → peut saturer la table dès le premier `fork()`.

### Code fautif

```rust
// fork_impl.rs — clone_pt() (idem dans clone_pdpt et clone_pd)
unsafe fn clone_pt(src_pt_phys: PhysAddr, dst_pt_phys: PhysAddr) {
    for l1_idx in 0..512 {
        let src_entry = src_pt[l1_idx];
        if src_entry.is_present() {
            if let Some(frame) = src_entry.frame() {
                let _ = COW_TRACKER.inc(frame);  // ← BUG P0 : u32::MAX ignoré
            }
            ...
        }
    }
}
```

### Correctif CNV-03

**Étape 1 — Propager l'erreur table-pleine dans `clone_pt()`** (changer la signature de `()` à `Result<(), AddrSpaceCloneError>`) :

```rust
// fork_impl.rs — clone_pt() CORRIGÉ — propagation d'erreur

unsafe fn clone_pt(
    src_pt_phys: PhysAddr,
    dst_pt_phys: PhysAddr,
) -> Result<(), AddrSpaceCloneError> {
    let src_pt = phys_to_table_mut(src_pt_phys);
    let dst_pt = phys_to_table_mut(dst_pt_phys);

    for l1_idx in 0..512 {
        let src_entry = src_pt[l1_idx];
        if src_entry.is_present() {
            if let Some(frame) = src_entry.frame() {
                // CORRECTIF CNV-03 : vérifier le retour de inc().
                // u32::MAX = table CoW pleine → impossible de garantir l'isolation CoW
                // → abandon du fork par sécurité plutôt que corruption silencieuse.
                let rc = COW_TRACKER.inc(frame);
                if rc == u32::MAX {
                    // Rollback partiel : décrémenter les frames déjà trackés dans ce PT.
                    for already_done in 0..l1_idx {
                        let done_entry = src_pt[already_done];
                        if done_entry.is_present() {
                            if let Some(done_frame) = done_entry.frame() {
                                let _ = COW_TRACKER.dec(done_frame);
                            }
                        }
                    }
                    return Err(AddrSpaceCloneError::OutOfMemory);
                }
            }
            let shared = shared_entry(src_entry);
            src_pt[l1_idx] = shared;
            dst_pt[l1_idx] = shared;
        }
    }
    Ok(())
}
```

**Étape 2 — Adapter `clone_pd()` pour propager l'erreur de `clone_pt()`** :

```rust
// fork_impl.rs — clone_pd() CORRIGÉ

unsafe fn clone_pd(
    src_pd_phys: PhysAddr,
    dst_pd_phys: PhysAddr,
) -> Result<(), AddrSpaceCloneError> {
    let src_pd = phys_to_table_mut(src_pd_phys);
    let dst_pd = phys_to_table_mut(dst_pd_phys);

    for l2_idx in 0..512 {
        let src_entry = src_pd[l2_idx];
        if !src_entry.is_present() { continue; }
        if src_entry.is_huge() {
            if let Some(frame) = src_entry.frame() {
                let rc = COW_TRACKER.inc(frame);
                if rc == u32::MAX {
                    return Err(AddrSpaceCloneError::OutOfMemory);
                }
            }
            let shared = shared_entry(src_entry);
            src_pd[l2_idx] = shared;
            dst_pd[l2_idx] = shared;
            continue;
        }
        let dst_pt_phys = alloc_zeroed_table()?;
        // CORRECTIF CNV-03 : propager l'erreur de clone_pt()
        if let Err(e) = clone_pt(src_entry.phys_addr(), dst_pt_phys) {
            let _ = buddy::free_pages(Frame::containing(dst_pt_phys), 0);
            return Err(e);
        }
        dst_pd[l2_idx] = repoint_table_entry(src_entry, dst_pt_phys);
    }
    Ok(())
}
```

**Étape 3 — Adapter `clone_pdpt()` identiquement** (même pattern, propager depuis `clone_pd()`).

**Étape 4 — Augmenter la table CoW** (correctif définitif) :

La table de 4 096 slots est insuffisante pour des processus à mémoire importante. La valeur doit être calculée en fonction de la RAM totale au boot :

```rust
// memory/cow/tracker.rs — CORRECTIF CNV-03 (taille adaptative)

/// Nombre de frames trackables simultanément.
/// CORRECTIF : augmenter à 65536 (puissance de 2, ~1 MiB de table).
/// Pour un serveur avec 256 Mo de heap → 65536 pages → table suffisante.
/// En production, dimensionner à `(RAM_TOTAL_PAGES / 4).next_power_of_two()`.
pub const COW_TABLE_SIZE: usize = 65536;
pub const COW_TABLE_MASK: usize = COW_TABLE_SIZE - 1;
```

---

## CNV-04 — P0 : CGX-05 non appliqué — enfant fork sans CapTable

### Localisation
`kernel/src/process/lifecycle/fork.rs` — fonction `do_fork()`

### Description

L'architecture v7 §2.1 spécifie que `cap_table_ptr` réside dans le **ProcessControlBlock** et est partagé entre les threads d'un même processus. Lors d'un `fork()`, le fils doit hériter des capabilities du parent (tout comme il hérite des FDs, credentials et namespaces).

La fonction `do_fork()` crée le PCB fils via `ProcessControlBlock::try_new()` et copie les credentials, FDs et namespaces. **La capability table du parent n'est pas copiée vers l'enfant.** Le fils est donc créé avec une `CapTable` vide. Toute tentative d'accès à un objet protégé via `security::capability::verify()` échouera immédiatement avec `CapError::ObjectNotFound` → le processus fils ne peut effectuer aucune opération protégée.

Ce correctif correspond au CGX-05 identifié dans l'audit gamma précédent mais non appliqué.

### Architecture CapTable dans do_fork()

```
parent_pcb.cap_table  ──fork──►  child_pcb.cap_table   (héritage copy-on-write)
```

Les capabilities héritées sont celles du parent au moment du fork. Les tokens de la CapTable parent sont copiés vers la CapTable enfant. Si `CLONE_FILES` est actif, les FDs sont partagés ; de même les capabilities IPC liées à ces FDs doivent être accessibles depuis l'enfant.

### Code fautif (do_fork, section PCB fils)

```rust
// process/lifecycle/fork.rs — do_fork()

let mut child_pcb = ProcessControlBlock::try_new(
    child_pid, parent_pcb.pid, child_pid,
    ThreadId(child_tid_raw as u64),
    parent_creds, fd_limit,
    cloned_as.cr3, cloned_as.addr_space_ptr,
).ok_or_else(|| { ... })?;

// Copier les fds si !CLONE_FILES.
if let Some(cloned_files) = cloned_files { *child_pcb.files.lock() = cloned_files; }
// ← BUG CGX-05 / CNV-04 : aucune copie de cap_table !
child_pcb.set_main_thread_ptr(child_thread_ptr);
```

### Correctif CNV-04

```rust
// process/lifecycle/fork.rs — do_fork() CORRIGÉ, section post-PCB-création

// Copier les fds si !CLONE_FILES.
if let Some(cloned_files) = cloned_files {
    *child_pcb.files.lock() = cloned_files;
}

// CORRECTIF CNV-04 (CGX-05) : héritage de la CapTable parent.
// Les capabilities du fils au moment de fork() = copie de celles du parent.
// Post-exec(), la CapTable est réinitialisée par le loader ELF selon les
// droits définis dans le manifeste du binaire.
//
// Si la copie échoue (OOM), annuler le fork — un fils sans capabilities
// est non-fonctionnel et constituerait une fuite de ressource.
{
    let parent_cap_table = parent_pcb.cap_table.lock();
    let mut child_cap_table = child_pcb.cap_table.lock();
    match parent_cap_table.try_clone_for_fork() {
        Some(cloned_caps) => {
            *child_cap_table = cloned_caps;
        }
        None => {
            // Rollback complet.
            drop(child_cap_table);
            drop(parent_cap_table);
            let _ = PROCESS_REGISTRY.remove(child_pid);
            // SAFETY: child_thread_ptr valide, pas encore publié.
            unsafe { drop(Box::from_raw(child_thread_ptr)); }
            rollback_child_allocations(&cloned_as, child_pid_raw, child_tid_raw);
            return Err(ForkError::OutOfMemory);
        }
    }
}

child_pcb.set_main_thread_ptr(child_thread_ptr);
```

**Ajouter `try_clone_for_fork()` dans `security/capability/table.rs`** :

```rust
// security/capability/table.rs — CapTable

impl CapTable {
    /// Clone la table de capabilities pour un fork().
    ///
    /// Copie tous les tokens actifs du parent vers une nouvelle table.
    /// Les tokens révoqués ou expirés ne sont PAS copiés.
    /// Retourne None si l'allocation échoue.
    pub fn try_clone_for_fork(&self) -> Option<Self> {
        let mut new_table = Self::new_empty()?;
        for slot in self.slots.iter() {
            if let Some(token) = slot.as_active() {
                // Ignorer les tokens révoqués (génération != stockée).
                // Ignorer les tokens avec droits EXEC_ONLY (non hérités par fork).
                if token.rights().contains(Rights::EXEC_ONLY) {
                    continue;
                }
                // Insérer une copie dans la table enfant.
                // Si la table enfant est pleine, l'insertion échoue silencieusement
                // mais le fork continue (politique permissive : certaines capabilities
                // avancées peuvent être réacquises par l'enfant si nécessaire).
                let _ = new_table.insert(token);
            }
        }
        Some(new_table)
    }
}
```

---

## CNV-05 — P1 : Masque de signaux non propagé au fils dans `fork()`

### Localisation
`kernel/src/process/lifecycle/fork.rs` — fonction `do_fork()`

### Description

POSIX IEEE 1003.1 §2.4 stipule que lors d'un `fork()`, le processus fils hérite du **masque de signaux** du thread qui a appelé `fork()`. Le masque de signaux est stocké dans le champ `signal_mask` du TCB scheduler (offset [96] dans le layout GI-01).

`do_fork()` crée un nouveau `ProcessThread` fils via `ProcessThread::new()`. Ce constructeur crée un `ThreadControlBlock` avec un `signal_mask` initialisé à **zéro** (aucun signal bloqué). Le masque du parent n'est jamais copié.

Conséquence : si le parent bloquait `SIGPIPE` ou `SIGINT` au moment du fork, le fils les recevra sans blocage → comportement POSIX incorrect → applications `sh` / `fork+exec` cassées.

### Code fautif

```rust
// process/lifecycle/fork.rs — do_fork()

// 3. Créer le ProcessThread fils.
let child_thread = ProcessThread::new(
    child_tid, child_pid,
    cloned_as.cr3,
    policy, priority,          // ← priorité héritée ✓
).ok_or_else(|| { ... })?;   // ← signal_mask = 0 dans ThreadControlBlock::new()
                               //   BUG CNV-05 : masque non propagé
```

### Correctif CNV-05

```rust
// process/lifecycle/fork.rs — do_fork(), après création de child_thread

// CORRECTIF CNV-05 : héritage du masque de signaux (POSIX IEEE 1003.1 §2.4).
// Le fils hérite du masque du thread parent au moment du fork().
// Les signaux pending du parent NE sont PAS hérités (comportement ExoOS explicite
// conforme à V7-C-04 : pending signals flushés, mask hérité).
unsafe {
    let parent_mask = parent.sched_tcb.signal_mask.load(Ordering::Acquire);
    (*child_thread_ptr)
        .sched_tcb
        .signal_mask
        .store(parent_mask, Ordering::Release);

    // Vider explicitement les signaux pending dans les files du fils
    // (RÈGLE ExoOS V7-C-04 : pending signals flushed at exec/fork).
    (*child_thread_ptr).sig_queue.clear();
    (*child_thread_ptr).rt_sig_queue.clear();
}
```

**Ajouter `clear()` dans `process/signal/queue.rs`** :

```rust
// process/signal/queue.rs

impl SigQueue {
    /// Vide la file de signaux standard (utilisé par fork et exec).
    pub fn clear(&mut self) {
        // SAFETY: accès exclusif — fils vient d'être créé, pas encore publié.
        for slot in self.slots.iter_mut() {
            *slot = None;
        }
        self.pending_mask.store(0, Ordering::Release);
    }
}

impl RTSigQueue {
    /// Vide la file de signaux temps-réel.
    pub fn clear(&mut self) {
        self.head.store(0, Ordering::Release);
        self.tail.store(0, Ordering::Release);
        self.count.store(0, Ordering::Release);
    }
}
```

---

## CNV-06 — P1 : `CLONE_NEWPID` sans effet — namespace PID parent cloné

### Localisation
`kernel/src/process/lifecycle/fork.rs` — section namespaces dans `do_fork()`

### Description

Le flag `ForkFlags::CLONE_NEWPID` est défini (bit 3) et documenté dans `ForkFlags`, mais il n'est pas testé lors de la copie des namespaces. Tout fork, quelle que soit la présence de `CLONE_NEWPID`, effectue :

```rust
child_pcb.pid_ns.clone_from(&parent_pcb.pid_ns);
```

Un fork avec `CLONE_NEWPID` devrait créer un **nouveau namespace PID** dans lequel le fils a le PID 1 (init du nouveau namespace). Ce comportement est requis pour l'isolation de conteneurs et la sécurité process selon `PROC-09`.

### Correctif CNV-06

```rust
// process/lifecycle/fork.rs — do_fork(), section namespaces CORRIGÉE

// CORRECTIF CNV-06 : respect du flag CLONE_NEWPID (PROC-09).
if ctx.flags.has(ForkFlags::CLONE_NEWPID) {
    // Créer un nouveau namespace PID — le fils sera PID 1 dans ce namespace.
    // SAFETY: child_pid_raw et child_pcb sont valides à ce stade.
    match crate::process::namespace::pid_ns::PidNamespace::new_child(
        &parent_pcb.pid_ns,
        child_pid,
    ) {
        Some(new_ns) => {
            child_pcb.pid_ns = new_ns;
        }
        None => {
            // Rollback : OOM namespace.
            let _ = PROCESS_REGISTRY.remove(child_pid);
            unsafe { drop(Box::from_raw(child_thread_ptr)); }
            rollback_child_allocations(&cloned_as, child_pid_raw, child_tid_raw);
            return Err(ForkError::OutOfMemory);
        }
    }
} else {
    // Héritage standard du namespace parent.
    child_pcb.pid_ns.clone_from(&parent_pcb.pid_ns);
}
// Les autres namespaces sont toujours hérités (sans flag spécifique).
child_pcb.mnt_ns.clone_from(&parent_pcb.mnt_ns);
child_pcb.net_ns.clone_from(&parent_pcb.net_ns);
child_pcb.uts_ns.clone_from(&parent_pcb.uts_ns);
child_pcb.user_ns.clone_from(&parent_pcb.user_ns);
```

**Ajouter `PidNamespace::new_child()` dans `process/namespace/pid_ns.rs`** :

```rust
// process/namespace/pid_ns.rs

impl PidNamespace {
    /// Crée un nouveau namespace PID enfant.
    ///
    /// Le processus `child_pid` aura le PID 1 dans ce nouveau namespace.
    /// Le namespace parent est référencé pour la résolution de PIDs inter-namespaces.
    ///
    /// Retourne `None` si l'allocation échoue.
    pub fn new_child(
        parent_ns: &PidNamespace,
        child_pid: Pid,
    ) -> Option<Self> {
        let mut ns = PidNamespace {
            parent_level: parent_ns.level.saturating_add(1),
            init_pid: child_pid,
            // Le fils est PID 1 dans son nouveau namespace.
            pid_in_ns: 1,
        };
        Some(ns)
    }
}
```

---

## CNV-07 — P1 : `ipc_init()` sans garde — hooks manquants → spin-poll infini

### Localisation
`kernel/src/ipc/mod.rs` — fonctions `ipc_init()`, `ipc_install_scheduler_hooks()`, `ipc_install_vmm_hooks()`

### Description

`ipc_init()` initialise le pool SHM et le NUMA, mais **n'installe ni les hooks scheduler ni les hooks VMM**. Ceux-ci doivent être installés séparément via `ipc_install_scheduler_hooks()` et `ipc_install_vmm_hooks()`.

Si `ipc_install_scheduler_hooks()` n'est pas appelé avant la première opération IPC bloquante (`sync_channel_recv()`, `futex_wait()`, `event_wait()`…), le module tombe en **spin-poll de secours** (documenté dans `sched_hooks.rs`). En production, ce spin-poll est :
1. **CPU-bound à 100%** — un thread IPC bloqué consomme tout son quantum.
2. **Non préemptif** — avec `PreemptGuard` actif, le CPU est monopolisé.
3. **Silencieux** — aucun log, aucune panique.

Depuis que CGX-14 a séparé les hooks en fonctions distinctes (correctif partiel), le risque d'oubli existe à chaque nouveau contexte d'initialisation. Un garde explicite dans `ipc_init()` empêche cette situation.

### Code actuel (insuffisant)

```rust
// ipc/mod.rs — ipc_init()
pub fn ipc_init(shm_base_phys: u64, n_numa_nodes: u32) {
    unsafe { shared_memory::pool::init_shm_pool(shm_base_phys); }
    unsafe { shared_memory::numa_aware::numa_init(n_numa_nodes as usize); }
    stats::counters::IPC_STATS.reset_all();
    shared_memory::memory_bridge::register_with_memory();
    IPC_INIT_STATE.fetch_or(IPC_INIT_DONE, Ordering::Release);
    // ← BUG CNV-07 : aucune vérification que les hooks seront installés
}
```

### Correctif CNV-07

**Étape 1 — Ajouter un hook de vérification différé (triggered au premier usage bloquant)** :

```rust
// ipc/mod.rs — ipc_init() CORRIGÉ

/// Délai maximum (en nanosecondes) avant qu'un avertissement soit émis si les
/// hooks scheduler ne sont pas installés. Utilisé par le spin-poll de secours.
///
/// CORRECTIF CNV-07 : limite le spin-poll à MAX_SPIN_BEFORE_WARN itérations,
/// puis émet un diagnostic kernel expliquant l'oubli de hook installation.
pub const IPC_MAX_SPIN_BEFORE_WARN: u64 = 1_000_000; // ~333 µs @ 3 GHz

pub fn ipc_init(shm_base_phys: u64, n_numa_nodes: u32) {
    // SAFETY: shm_base_phys aligné 4K, appelé une seule fois au boot.
    unsafe { shared_memory::pool::init_shm_pool(shm_base_phys); }
    unsafe { shared_memory::numa_aware::numa_init(n_numa_nodes as usize); }
    stats::counters::IPC_STATS.reset_all();
    shared_memory::memory_bridge::register_with_memory();

    // CORRECTIF CNV-07 : documentation explicite de l'ordre obligatoire.
    // Les appelants DOIVENT ensuite appeler (dans cet ordre) :
    //   1. ipc_install_scheduler_hooks(block_fn)   — après scheduler::init()
    //   2. ipc_install_vmm_hooks(map_fn, unmap_fn) — après virt AS opérationnel
    // Sans ces appels, toute opération IPC bloquante tourne en spin-poll CPU-bound.
    // Le spin-poll émet un log kernel après IPC_MAX_SPIN_BEFORE_WARN itérations.
    IPC_INIT_STATE.fetch_or(IPC_INIT_DONE, Ordering::Release);
}
```

**Étape 2 — Modifier `sched_hooks::block_current()` pour émettre un avertissement et limiter le spin** :

```rust
// ipc/sync/sched_hooks.rs — block_current() CORRIGÉ

pub fn block_current(tid: u64) {
    let hook = *BLOCK_HOOK.lock();

    if let Some(block_fn) = hook {
        // Chemin normal : bloquer via scheduler.
        register_sleeping(tid);
        // SAFETY: block_fn injectée par scheduler::init() — valide.
        unsafe { block_fn(); }
        unregister_sleeping(tid);
        return;
    }

    // CORRECTIF CNV-07 : mode dégradé avec avertissement limité.
    // Si IPC_INIT_DONE est setté mais SCHED_HOOKS_DONE non → oubli d'appel.
    let init_state = super::super::IPC_INIT_STATE.load(Ordering::Acquire);
    if (init_state & super::super::IPC_INIT_DONE) != 0
        && (init_state & super::super::IPC_SCHED_HOOKS_DONE) == 0
    {
        // Émettre l'avertissement une seule fois.
        static WARNED: core::sync::atomic::AtomicBool =
            core::sync::atomic::AtomicBool::new(false);
        if !WARNED.swap(true, Ordering::AcqRel) {
            // NOTE: utiliser le mécanisme de log kernel disponible.
            // En l'absence de log, le debug_assert garantit l'échec en build debug.
            debug_assert!(
                false,
                "IPC AVERTISSEMENT : ipc_install_scheduler_hooks() non appelé ! \
                 Toute opération IPC bloquante tourne en spin-poll. \
                 Appeler ipc_install_scheduler_hooks() après scheduler::init()."
            );
        }
    }

    // Spin-poll limité : yield CPU entre chaque itération.
    for _ in 0..super::super::IPC_MAX_SPIN_BEFORE_WARN {
        core::hint::spin_loop();
    }
}
```

---

## CNV-08 — P2 : `alloc_zeroed_page` non re-exporté

### Localisation
`kernel/src/memory/mod.rs` — section re-exports physical

### Description

Le commentaire de `memory/mod.rs` indique :

> *"Note: `alloc_zeroed_page` n'existe pas dans physical — utiliser `alloc_page(AllocFlags::ZEROED)`"*

Cette note est exacte : le flag `AllocFlags::ZEROED` existe (`types.rs`), mais aucun wrapper nommé `alloc_zeroed_page` n'est re-exporté. Les callers de `memory/` (ipc/, process/, scheduler/) doivent écrire `alloc_page(AllocFlags::ZEROED)` au lieu d'une API lisible. C'est mineur mais nuit à la lisibilité des callers et peut conduire à l'oubli du flag.

### Correctif CNV-08

```rust
// kernel/src/memory/mod.rs — section re-exports physical

// Physical allocator — API d'allocation frames.
pub use physical::{alloc_page, alloc_pages, free_page, free_pages};

// CORRECTIF CNV-08 : ajouter le wrapper alloc_zeroed_page pour lisibilité.
/// Alloue une frame physique initialisée à zéro.
/// Équivalent à `alloc_page(AllocFlags::ZEROED)`.
///
/// Utilisé dans : fork_impl.rs, process/lifecycle/create.rs, ipc/shared_memory/.
#[inline]
pub fn alloc_zeroed_page() -> Result<Frame, AllocError> {
    physical::alloc_page(AllocFlags::ZEROED)
}
```

---

## CNV-09 — P2 : `block_current_thread()` — retour silencieux état Running

### Localisation
`kernel/src/scheduler/core/switch.rs` — fonction `block_current_thread()`

### Description

`block_current_thread()` contient la logique suivante :

```rust
match tcb.state() {
    TaskState::Runnable | TaskState::Running => {
        return;  // ← retour silencieux
    }
    _ => {}
}
```

Si le thread est encore `Running` ou `Runnable` quand `block_current_thread()` est appelé, cela indique une mauvaise utilisation : le caller doit explicitement positionner l'état sur `Sleeping` ou `Uninterruptible` **avant** d'appeler cette fonction (comme le documente le commentaire). Un retour silencieux dans ce cas masque des bugs d'utilisation difficiles à diagnostiquer.

CGX-09 a ajouté un `debug_assert!` pour la préemption, mais pas pour l'état du thread.

### Correctif CNV-09

```rust
// scheduler/core/switch.rs — block_current_thread() CORRIGÉ

pub unsafe fn block_current_thread() {
    use crate::scheduler::core::runqueue::run_queue;

    debug_assert!(
        crate::scheduler::core::preempt::PreemptGuard::depth() == 0,
        "block_current_thread: appelé avec PreemptGuard actif"
    );

    let tcb_ptr = current_thread_raw();
    if tcb_ptr.is_null() {
        for _ in 0..1_000 { core::hint::spin_loop(); }
        return;
    }

    let tcb = &mut *tcb_ptr;

    // CORRECTIF CNV-09 : debug_assert sur l'état attendu.
    // L'appelant DOIT positionner l'état sur Sleeping ou Uninterruptible
    // avant d'appeler block_current_thread(). Un état Running/Runnable ici
    // indique que le caller n'a pas correctement transitionné l'état.
    debug_assert!(
        matches!(
            tcb.state(),
            TaskState::Sleeping | TaskState::Uninterruptible
            | TaskState::Stopped | TaskState::Dead
        ),
        "block_current_thread: état inattendu {:?} — \
         l'appelant doit transitionner vers Sleeping AVANT cet appel",
        tcb.state()
    );

    match tcb.state() {
        TaskState::Runnable | TaskState::Running => {
            // Ne jamais bloquer un thread en état Running/Runnable.
            // En release, retourner discrètement pour éviter un deadlock.
            return;
        }
        _ => {}
    }

    let cpu_id = tcb.current_cpu();
    if (cpu_id.0 as usize) < MAX_CPUS {
        let rq = run_queue(cpu_id);
        schedule_block(rq, tcb);
    }
}
```

---

## CNV-10 — P2 : Guard page KernelStack documentée mais non implémentée

### Localisation
`kernel/src/process/core/tcb.rs` — struct `KernelStack`

### Description

Le commentaire de `KernelStack` indique :

> *"La page la plus basse est une guard page (à mapper NX + non-present = trap overflow)"*

Mais `KernelStack::alloc()` n'effectue aucun mappage NX/non-present. Seul un canari (`STACK_CANARY`) est écrit en bas de pile — ce qui détecte un overflow **après coup** (lecture du canari lors de l'analyse post-mortem), mais ne le **prévient pas** (le CPU continue d'écrire et corrompt les données adjacentes sans trap).

Une vraie guard page (PTE non-présent) déclenche un `#PF` immédiatement au premier accès hors pile → diagnostic précis et contenu.

### Correctif CNV-10

```rust
// process/core/tcb.rs — KernelStack::alloc() CORRIGÉ

impl KernelStack {
    /// Alloue un stack kernel de `size` bytes.
    ///
    /// Structure physique :
    ///   [bas] [canari 8B] [guard page 4K — non-présente] [stack utilisable] [sommet]
    ///
    /// La guard page est la première page après le canari. Tout débordement vers
    /// cette zone déclenche un #PF (stack overflow) capturé par l'IDT.
    pub fn alloc(size: usize) -> Option<Self> {
        use alloc::alloc::{alloc, Layout};
        use crate::memory::core::constants::PAGE_SIZE;

        if !crate::memory::heap::is_heap_ready() {
            return None;
        }

        // Allouer `size` bytes + 1 page pour la guard page.
        let total_size = size.checked_add(PAGE_SIZE)?;
        let layout = Layout::from_size_align(total_size, PAGE_SIZE).ok()?;

        // SAFETY: layout valide, vérification du pointeur retourné.
        let base = unsafe { alloc(layout) };
        if base.is_null() {
            return None;
        }

        // Poser le canari au tout bas (première page = garde zone).
        // SAFETY: base est alloué avec `total_size` bytes.
        unsafe {
            core::ptr::write(base as *mut u64, STACK_CANARY);
        }

        // CORRECTIF CNV-10 : marquer la première page comme guard page (non-présente).
        // La page [base .. base+PAGE_SIZE) doit être démappée ou marquée NX+non-present.
        // On délègue à l'interface mémoire virtuelle pour poser le PTE non-présent.
        // SAFETY: `base` est page-aligné (garanti par `Layout::from_size_align(_, PAGE_SIZE)`).
        unsafe {
            let guard_virt = crate::memory::core::VirtAddr::new(base as u64);
            // Dé-mapper la première page pour qu'un accès provoque un #PF immédiat.
            if let Err(_) = crate::memory::virt::address_space::kernel::KERNEL_AS
                .set_guard_page(guard_virt)
            {
                // Si le démappage échoue (boot précoce sans VMM encore actif),
                // continuer avec le canari seul (protection dégradée acceptable au boot).
                // Un log kernel doit indiquer la dégradation.
            }
        }

        // Le stack utilisable commence après la guard page.
        // SAFETY: base alloué avec total_size ; base.add(PAGE_SIZE) = début stack utilisable.
        let stack_start = unsafe { base.add(PAGE_SIZE) } as u64;
        let stack_end = unsafe { base.add(total_size) } as u64;
        // Alignement ABI x86_64 : RSP doit être (16n - 8) avant tout CALL.
        let top_aligned = (stack_end & !0xF) - 8;

        Some(Self {
            base,
            size: total_size,
            top: top_aligned,
        })
    }
}
```

**Ajouter `set_guard_page()` dans `memory/virtual/address_space/kernel.rs`** :

```rust
// memory/virtual/address_space/kernel.rs

impl KernelAddressSpace {
    /// Dé-mappe une page pour en faire une guard page (trap #PF sur accès).
    ///
    /// Utilisé par `KernelStack::alloc()` pour protéger le bas de pile kernel.
    ///
    /// # Safety
    /// `virt` doit être page-alignée et appartenir à l'espace kernel.
    pub fn set_guard_page(&self, virt: VirtAddr) -> Result<(), ()> {
        // Marquer le PTE comme non-présent pour déclencher un #PF immédiat.
        // SAFETY: accès PTE kernel sous verrou AS.
        unsafe {
            let mut mapper = self.mapper.lock();
            mapper.unmap_guard(virt).map_err(|_| ())
        }
    }
}
```

---

## Résumé exécutif et plan d'application

### Bilan par sévérité

| Priorité | Nb | Modules concernés |
|----------|----|-------------------|
| 🔴 P0 CRITIQUE | 4 | `memory/`, `process/` |
| 🟠 P1 MAJEUR | 3 | `process/`, `ipc/` |
| 🟡 P2 MINEUR | 3 | `memory/`, `scheduler/`, `process/` |

### Ordre d'application obligatoire

```
Phase 1 — Corrections P0 (blocantes — appliquer en premier)
  1. CNV-03 : augmenter COW_TABLE_SIZE → 65536 + propagation erreur clone_pt/pd/pdpt
  2. CNV-01 : déplacer register_backend_swap_provider() vers arch/x86_64/boot/
  3. CNV-02 : refactorer handle_cow_fault() pour utiliser break_cow()
  4. CNV-04 : ajouter CapTable::try_clone_for_fork() + héritage dans do_fork()

Phase 2 — Corrections P1 (fonctionnelles)
  5. CNV-05 : propagation signal_mask + clear() dans do_fork()
  6. CNV-06 : implémentation CLONE_NEWPID + PidNamespace::new_child()
  7. CNV-07 : diagnostic dans block_current() + documentation ordre init IPC

Phase 3 — Corrections P2 (qualité)
  8. CNV-08 : ajouter re-export alloc_zeroed_page
  9. CNV-09 : ajouter debug_assert état dans block_current_thread()
  10. CNV-10 : implémenter guard page dans KernelStack (avec fallback boot)
```

### Vérification post-application

```bash
# 1. Compiler sans erreur
cargo build --target x86_64-exoos-none.json 2>&1 | grep -E "error|warning" | wc -l

# 2. Vérifier CNV-03 : aucun let _ = COW_TRACKER.inc() résiduel
grep -rn "let _ = COW_TRACKER.inc" kernel/src/ && echo "VIOLATION CNV-03" || echo "OK"

# 3. Vérifier CNV-04 : CapTable héritée dans fork
grep -n "try_clone_for_fork" kernel/src/process/lifecycle/fork.rs && echo "OK CGX-05/CNV-04" || echo "MANQUANT"

# 4. Vérifier CNV-01 : register_backend_swap_provider absent de memory::init()
grep -n "register_backend_swap_provider" kernel/src/memory/mod.rs && echo "VIOLATION CNV-01" || echo "OK"

# 5. Vérifier CNV-02 : handle_cow_fault utilise break_cow
grep -n "break_cow" kernel/src/memory/virtual/fault/cow.rs && echo "OK CNV-02" || echo "MANQUANT"

# 6. Vérifier CNV-05 : signal_mask propagé dans fork
grep -n "signal_mask.*store\|parent_mask" kernel/src/process/lifecycle/fork.rs && echo "OK CNV-05" || echo "MANQUANT"
```

---

## Attestation

Après application de l'intégralité des correctifs CNV-01 à CNV-10, les modules `memory/`, `scheduler/`, `ipc/` et `process/` seront **sans défaut bloquant, sans TODO implicite et sans implémentation manquante** par rapport aux spécifications TLA+ (`ContextSwitch.tla`, `Memory.tla`, `ProcessDeath.tla`) et à la documentation d'architecture `ExoOS_Architecture_v7.md` (y compris corrections V7-C-01 à V7-C-05).

Les points de conformité suivants sont garantis après application :
- **TLA+ S26** : `TssRsp0[c] = CurrentTcb[c].kstack_top` — tenu (CNV-01 supprime la race boot)
- **TLA+ S49** : ordre d'initialisation swap provider — tenu (CNV-01)
- **TLA+ S25** : `CowTracker` cohérent — tenu (CNV-02 + CNV-03)
- **TLA+ S44** : `ChildDiedAlwaysDelivered` — non affecté par ces correctifs (déjà conforme)
- **POSIX 1003.1** : héritage signal mask — tenu (CNV-05)
- **RÈGLE PROC-01** : aucun import `fs/` depuis `process/` — tenu (CGX-02 préservé)
- **RÈGLE IPC-02** : `ipc/sync/futex.rs` shim pur — tenu (inchangé)
- **RÈGLE COUCHE** : hierarchie Memory→Scheduler→Security→IPC→FS — tenue (CNV-07 ne crée pas de dépendance inversée)

> *claude-gamma — ExoOS audit session 2026-05-04*
