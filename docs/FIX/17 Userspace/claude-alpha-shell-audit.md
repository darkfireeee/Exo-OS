# ExoOS — Audit Shell/Userspace : Rapport Claude Alpha
**Date :** 2026-05-07  
**Commit analysé :** kernel.zip (branche post-ExoPhoenix)  
**Scope :** Blocages empêchant le démarrage du terminal/shell  
**Contexte Codex :** Dernier état avant impasse — fork atteint SYSRET mais PID1 silencieux après

---

## Résumé exécutif

Deux bugs P0 distincts et orthogonaux font que **tout processus userspace meurt silencieusement dès la première écriture mémoire après `fork()`**, que ce soit le parent ou l'enfant. L'impasse de Codex est correctement diagnostiquée (SYSRET atteint, puis silence), mais la cause racine est plus profonde que le seul clonage CoW : c'est l'architecture entière du fault handler userspace qui est non fonctionnelle.

---

## BUG-SHELL-01 — P0 CRITIQUE : `KERNEL_FAULT_ALLOC` est aveugle au processus courant

**Fichier :** `kernel/src/arch/x86_64/memory_iface.rs` (lignes ~261–314)  
**Fichier :** `kernel/src/arch/x86_64/exceptions.rs` (`do_page_fault`)

### Description

`KERNEL_FAULT_ALLOC` est l'unique `FaultAllocator` utilisé pour **tous** les page faults, y compris ceux venant de Ring 3. Toutes ses méthodes opèrent exclusivement sur `KERNEL_AS` :

```rust
// memory_iface.rs — impl FaultAllocator for KernelFaultAllocator

fn map_page(...) { KERNEL_AS.map(virt, frame, flags, self) }          // ← KERNEL_AS
fn remap_flags(...) { walker(KERNEL_AS.pml4_phys()).remap_flags(...) } // ← KERNEL_AS
fn translate(...) { KERNEL_AS.translate(virt) }                        // ← KERNEL_AS
fn read_pte_raw(...) { PageTableWalker::new(KERNEL_AS.pml4_phys())... } // ← KERNEL_AS
fn compare_exchange_pte_raw(...) { walker(KERNEL_AS.pml4_phys())... }   // ← KERNEL_AS
```

Le commentaire du code l'admet explicitement :
```
/// Mappe uniquement dans l'espace d'adressage kernel global (KERNEL_AS).
/// Quand process/ sera intégré, les faults utilisateur utiliseront
/// un allocateur lié à l'espace d'adressage du processus courant.
```

### Séquence d'échec concrète

Après `fork()`, la stack userspace du parent (et toutes les pages CoW du fils) sont marquées read-only avec le flag `FLAG_COW`. Dès la première écriture :

1. `#PF` déclenché depuis Ring 3, CR2 = adresse user stack
2. `do_page_fault()` appelle `handle_page_fault(&ctx, &KERNEL_FAULT_ALLOC)`
3. Dans `handle_cow_fault` :
   ```rust
   let old_raw = alloc.read_pte_raw(page_addr);
   // → walk KERNEL_AS.pml4_phys() → pas d'entrée user → retourne 0
   ```
4. `old_entry = PageTableEntry::from_raw(0)` → not present
5. `old_entry.frame()` → None
6. `alloc.translate(page_addr)` → walk KERNEL_AS → None
7. Fallback : `demand_paging::handle_demand_paging(ctx, vma, alloc)`
8. Dans `handle_demand_paging`, `alloc.map_page(user_addr, new_frame, flags)` :
   ```rust
   KERNEL_AS.map(user_addr, new_frame, flags, self)
   // → mappe la page dans KERNEL_AS, PAS dans le CR3 du processus !
   ```
9. Retour : `FaultResult::Handled` (faux positif)
10. IRETQ vers Ring 3 → le processus réécrit → `#PF` à nouveau → boucle infinie

**Résultat observé :** après quelques centaines de faults identiques (compteur `FAULT_STATS.demand_paging` monte), le scheduler finit par préempter le processus. Jamais d'erreur explicite. Le log Codex "SYSRET atteint, puis silence" correspond exactement à ce comportement : le parent revient de fork() via SYSRET, essaie d'écrire sur sa propre stack userspace, boucle indéfiniment sur le fault sans jamais progresser.

### Solution requise

Créer un `UserFaultAllocator` qui reçoit un `*const UserAddressSpace` au moment du fault, et délègue toutes les opérations PTE à cet espace d'adressage (son `pml4_phys`), pas à `KERNEL_AS`.

```rust
pub struct UserFaultAllocator<'a> {
    user_as: &'a UserAddressSpace,
}

impl FaultAllocator for UserFaultAllocator<'_> {
    fn read_pte_raw(&self, virt: VirtAddr) -> u64 {
        PageTableWalker::new(self.user_as.pml4_phys()).read_pte_raw(virt)
    }
    fn compare_exchange_pte_raw(&self, virt: VirtAddr, cur: u64, new: u64) -> Result<(), u64> {
        unsafe { PageTableWalker::new(self.user_as.pml4_phys())
                     .compare_exchange_leaf_raw(virt, cur, new) }
    }
    fn map_page(&self, virt: VirtAddr, frame: Frame, flags: PageFlags) -> Result<(), AllocError> {
        unsafe { self.user_as.map_page(virt, frame, flags, &KernelFrameAlloc) }
    }
    fn remap_flags(&self, virt: VirtAddr, flags: PageFlags) -> Result<(), AllocError> {
        PageTableWalker::new(self.user_as.pml4_phys()).remap_flags(virt, flags)
    }
    fn translate(&self, virt: VirtAddr) -> Option<PhysAddr> {
        self.user_as.translate(virt)
    }
    // alloc_zeroed / alloc_nonzeroed / free_frame → buddy (inchangé)
}
```

Dans `do_page_fault()`, utiliser `UserFaultAllocator` si `from_userspace` et `user_as` résolu, `KERNEL_FAULT_ALLOC` sinon :

```rust
if !from_kernel {
    if let Some(user_as_ptr) = resolve_current_user_as() {
        let user_alloc = UserFaultAllocator { user_as: &*user_as_ptr };
        return handle_page_fault(&ctx, &user_alloc);
    }
}
handle_page_fault(&ctx, &KERNEL_FAULT_ALLOC)
```

---

## BUG-SHELL-02 — P0 CRITIQUE : Arbre VMA non cloné lors du `fork()`

**Fichier :** `kernel/src/memory/virtual/address_space/fork_impl.rs` (fonction `clone_cow`)

### Description

`clone_cow()` crée le `UserAddressSpace` du fils avec `UserAddressSpace::new(child_pml4_phys, 0)`, qui initialise un **arbre VMA vide** :

```rust
// fork_impl.rs — clone_cow()
let child_as = match try_box_new(UserAddressSpace::new(child_pml4_phys, 0)) {
    Some(addr_space) => addr_space,
    // ...
};
// heap_end est copié, mais rien d'autre.
// L'arbre VMA (stack, code, data, heap, TLS...) est vide.
```

Pendant ce temps, `VmaTree` n'implémente pas `Clone` et il n'existe aucune fonction `clone_vmas_for_fork` dans le codebase (vérifié par grep).

### Séquence d'échec concrète

Dans `do_page_fault()`, la résolution VMA utilise l'arbre du fils :
```rust
if let Some(vma) = user_as.find_vma(fault_addr) {
    ctx = ctx.with_vma(vma);
}
```

Le fils a un arbre vide → `find_vma` retourne `None` → `ctx.vma_ptr = null`.

Dans `handle_page_fault` :
```rust
let vma = match ctx.find_vma(ctx.fault_addr) {
    Some(v) => v,
    None => {
        FAULT_STATS.not_mapped.fetch_add(1, Ordering::Relaxed);
        return FaultResult::Segfault { addr: ctx.fault_addr };  // ← toujours ici
    }
};
```

**Résultat :** le fils SIGSEGV immédiatement sur n'importe quel fault. Puisque fork_child_trampoline fait `iretq` vers `child_rsp` (stack userspace marquée CoW), le fils fault au premier accès stack → Segfault → mort.

Ce bug est **indépendant** de BUG-SHELL-01 : même si l'allocateur était correct, sans VMA le handler retournerait Segfault avant même d'atteindre le CoW break.

### Solution requise

Implémenter le clonage de l'arbre VMA dans `clone_cow()` :

```rust
// Dans VmaTree (vma/tree.rs)
pub fn clone_for_fork(&self) -> Option<VmaTree> {
    let mut new_tree = VmaTree::new();
    for vma in self.iter() {
        let mut cloned = vma.clone();
        // Les VMAs CoW partagées sont read-only jusqu'au break.
        // Pas besoin de modifier les flags VMA ici — les PTE sont déjà CoW.
        new_tree.insert(cloned)?;
    }
    Some(new_tree)
}
```

Dans `UserAddressSpace::new_for_fork()` (ou en modifiant `clone_cow`) :
```rust
// Après la construction du child_as
{
    let parent_inner = parent_as.inner.lock();
    let mut child_inner = child_as.inner.lock();
    child_inner.vma_tree = parent_inner.vma_tree.clone_for_fork()
        .ok_or(AddrSpaceCloneError::OutOfMemory)?;
    child_inner.mmap_hint = parent_inner.mmap_hint;
    child_inner.stack_bottom = parent_inner.stack_bottom;
}
```

---

## BUG-SHELL-03 — P1 : `flush_tlb_after_fork` trop agressif (performance + correctness)

**Fichier :** `kernel/src/memory/virtual/address_space/fork_impl.rs`

```rust
fn flush_tlb_after_fork(&self, _parent_cr3: u64) {
    unsafe {
        shootdown_sync(TlbFlushType::All, smp_cpu_count());
    }
}
```

`TlbFlushType::All` = flush complet de tous les TLBs sur tous les CPUs. C'est un sérialisation globale. Le correct serait un flush des seules pages marquées CoW (flush par plage) sur le CPU local + shootdown ciblé. En pratique avec un seul CPU (QEMU), pas de bug de correctness mais c'est un invariant à corriger avant SMP.

### Solution requise

```rust
fn flush_tlb_after_fork(&self, parent_cr3: u64) {
    // Flush local du CR3 parent suffit pour invalider les PTEs passées CoW.
    unsafe { crate::arch::x86_64::switch_cr3(parent_cr3); }
}
```
(Recharger le même CR3 invalide toutes les TLB entries non-global — suffisant après un fork single-AS.)

---

## BUG-SHELL-04 — P1 : Masque RFLAGS `FORCE_CLR` incorrect dans `do_fork()`

**Fichier :** `kernel/src/process/lifecycle/fork.rs` (lignes ~231–244)

```rust
const RFLAGS_FORCE_CLR: u64 = 0x0000_0000_0004_0100; // "TF=0, NT=0, RF=0, VM=0" ← commentaire faux
```

`0x40100` en binaire = bits 8 (TF) et **18 (AC — Alignment Check)**. Le commentaire prétend effacer TF, NT, RF, VM — mais :
- NT (bit 14), RF (bit 16), VM (bit 17) sont déjà absents de `RFLAGS_SAFE_MASK` → déjà 0
- **AC (bit 18) est dans `RFLAGS_SAFE_MASK`** (0x200CD5 inclut bit 18) → préservé par le mask, puis **effacé silencieusement** par FORCE_CLR

Résultat : le fils ne peut jamais activer AC (SIGBUS alignment). Comportement incorrect pour les programmes utilisant `stac`/`clac` ou testant EFLAGS.AC. Probablement pas bloquant pour exosh, mais source de bugs futurs.

### Solution requise

```rust
const RFLAGS_FORCE_CLR: u64 = (1 << 8)   // TF
                             | (1 << 14)  // NT
                             | (1 << 16)  // RF
                             | (1 << 17); // VM
// = 0x0003_4100
```

---

## BUG-SHELL-05 — P2 : `heap_end` partiellement hérité, `mmap_hint` et `stack_bottom` pas transmis

**Fichier :** `kernel/src/memory/virtual/address_space/fork_impl.rs`

Seul `heap_end` est copié du parent vers le fils. `mmap_hint` et `stack_bottom` sont réinitialisés aux valeurs par défaut dans `UserAddressSpace::new()`. Si le parent a fait des mmaps avant fork, le fils peut remappe aux mêmes adresses virtuelles → collision VMA silencieuse dès le premier `mmap()` du fils.

Ce bug est masqué tant que BUG-SHELL-02 n'est pas corrigé (le fils meurt avant le premier mmap).

---

## BUG-SHELL-06 — P2 : Signaux non envoyés lors d'un Segfault (SIGSEGV manquant)

**Fichier :** `kernel/src/arch/x86_64/exceptions.rs` (`do_page_fault`, branche `FaultResult::Segfault`)

```rust
FaultResult::Segfault { addr } => {
    let _ = addr;
    if frame.from_userspace() {
        // SIGSEGV sera livré par exception_return_to_user (RÈGLE SIGNAL-01).
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#PF kernel : accès invalide", frame);
    }
}
```

`exception_return_to_user()` appelle `proc_signal_on_exception_return()` — mais seulement si `signal_pending` est positionné dans le TCB. Or, aucun code n'envoie SIGSEGV au processus avant cet appel. Si le chemin signal n'est pas complet, le processus continue à tourner sans jamais recevoir SIGSEGV, provoquant une boucle infinie de faults à la même adresse.

### Solution requise

Avant `exception_return_to_user`, envoyer explicitement SIGSEGV :
```rust
FaultResult::Segfault { addr } => {
    if frame.from_userspace() {
        // Envoyer SIGSEGV au processus courant
        crate::process::signal::send_signal_to_current(Signal::SIGSEGV);
        exception_return_to_user(frame);
    } ...
}
```

---

## Analyse du path Codex : bon diagnostic, mauvaise cible

Codex a correctement identifié que :
1. Le triple fault initial venait d'un accès kernel sous CR3 userspace → corrigé par remapping image kernel
2. Le panic dans le clonage FD → corrigé (try_clone_for_fork)
3. SYSRET atteint mais silence → encore actif

Codex est en train de chercher la cause du silence post-SYSRET au niveau du **retour syscall / scheduler** (dernier message : "pointer vers CoW stack parent"). C'est la bonne intuition mais la mauvaise couche : le problème n'est pas dans switch.rs ou dispatch.rs, il est dans le fault handler.

**Recommandation :** ne pas continuer sur le chemin "instrumentation fork pour voir où ça se perd". Corriger BUG-SHELL-01 et BUG-SHELL-02 en parallèle, puis rebooter. Le silence disparaîtra.

---

## Plan d'action prioritaire

| Priorité | Bug | Fichier(s) à modifier | Complexité |
|----------|-----|----------------------|------------|
| P0 | BUG-SHELL-01 | `memory_iface.rs`, `exceptions.rs` | ~100 lignes |
| P0 | BUG-SHELL-02 | `fork_impl.rs`, `vma/tree.rs`, `address_space/user.rs` | ~60 lignes |
| P1 | BUG-SHELL-03 | `fork_impl.rs` | 5 lignes |
| P1 | BUG-SHELL-04 | `fork.rs` | 1 ligne |
| P2 | BUG-SHELL-05 | `fork_impl.rs` | 10 lignes |
| P2 | BUG-SHELL-06 | `exceptions.rs` | 5 lignes |

BUG-SHELL-01 et BUG-SHELL-02 doivent être corrigés **ensemble** : corriger l'un sans l'autre ne suffira pas à démarrer le shell.
