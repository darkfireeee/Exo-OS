# Exo-OS — Rapport d'analyse Claude-Delta
## Blocages userspace / shell / terminal

> **Périmètre de l'analyse** : `kernel.zip` + historique d'actions Codex.  
> **Contexte** : le boot atteint PID1 → premier `fork()` de `ipc_router` → le parent
> ne revient plus après SYSRETQ. Le shell n'est jamais atteint.

---

## Résumé exécutif

Trois bogues indépendants se superposent et empêchent tout démarrage de service :

| # | Sévérité | Fichier | Symptôme |
|---|----------|---------|----------|
| B-01 | **CRITIQUE** | `arch/x86_64/memory_iface.rs` | `KERNEL_FAULT_ALLOC` marche les mauvaises page tables pour les fautes utilisateur → CoW parent jamais résolue |
| B-02 | **CRITIQUE** | `memory/virtual/address_space/fork_impl.rs` | L'arbre VMA n'est **pas** copié vers l'enfant → tout `#PF` de l'enfant retourne Segfault |
| B-03 | **CRITIQUE** | `arch/x86_64/exceptions.rs` | `SIGSEGV` jamais mis en file avant `exception_return_to_user` → boucle infinie `#PF` |
| B-04 | Majeur | `memory/virtual/address_space/fork_impl.rs` | `flush_tlb_after_fork` fait un `shootdown_sync(All, N_CPU)` synchrone qui peut deadlocker |
| B-05 | Mineur | `process/lifecycle/fork.rs` | `RFLAGS_SAFE_MASK` oublie le bit AC (bit 18) malgré le commentaire |
| B-06 | Mineur | `process/lifecycle/exec.rs` | `stack_base` calculé avec un nombre de pages fixe (8) indépendant de l'ELF chargé |

---

## B-01 — `KERNEL_FAULT_ALLOC` marche la mauvaise PML4 pour les fautes user (CRITIQUE)

### Fichier
`kernel/src/arch/x86_64/memory_iface.rs` — `KernelFaultAllocator`

### Description

`do_page_fault` (exceptions.rs, ligne 595) appelle systématiquement :

```rust
let result = crate::memory::virt::fault::handler::handle_page_fault(&ctx, &KERNEL_FAULT_ALLOC);
```

`KERNEL_FAULT_ALLOC` est une instance de `KernelFaultAllocator`. Ses méthodes :

```rust
fn translate(&self, virt: VirtAddr) -> Option<PhysAddr> {
    crate::memory::virt::address_space::KERNEL_AS.translate(virt)  // ← KERNEL_AS !
}

fn read_pte_raw(&self, virt: VirtAddr) -> u64 {
    let walker = PageTableWalker::new(
        crate::memory::virt::address_space::KERNEL_AS.pml4_phys(), // ← KERNEL_AS !
    );
    walker.read_pte_raw(virt)
}

fn compare_exchange_pte_raw(&self, virt: VirtAddr, current: u64, new: u64) -> Result<(), u64> {
    let walker = PageTableWalker::new(
        crate::memory::virt::address_space::KERNEL_AS.pml4_phys(), // ← KERNEL_AS !
    );
    unsafe { walker.compare_exchange_leaf_raw(virt, current, new) }
}
```

Le commentaire du fichier l'admet explicitement :
> *"Quand `process/` sera intégré, les faults utilisateur utiliseront un allocateur lié à l'espace d'adressage du processus courant."*

Ce point d'intégration **n'a pas été réalisé**.

### Conséquence exacte sur le CoW parent

Après `do_fork()`, le parent revient en userspace via SYSRETQ. La première écriture sur sa propre pile (RSP userspace) est désormais CoW-protégée (read-only). Cela déclenche un `#PF` (W=1, P=1) :

1. `do_page_fault` construit un `FaultContext` avec la VMA de la pile (trouvée dans le `UserAddressSpace` du processus).
2. `handle_page_fault(&ctx, &KERNEL_FAULT_ALLOC)` est appelé.
3. `handle_cow_fault` lit la PTE via `KERNEL_FAULT_ALLOC.read_pte_raw(page_addr)`.
4. Cette lecture marche la **PML4 du noyau** (`KERNEL_AS`), pas le CR3 du processus parent.
5. La PTE retournée est 0 (absent dans l'AS kernel) → `old_entry.frame()` = None.
6. Fallback vers `alloc.translate(page_addr)` → `KERNEL_AS.translate(virt)` → None.
7. Fallback vers `demand_paging::handle_demand_paging` → alloue une **nouvelle page vierge** dans KERNEL_AS.
8. Le parent récupère une pile vide au lieu de briser son CoW → corruption immédiate → crash.

**Le patron correct existe déjà dans `kernel/src/drivers/dma.rs`** (struct `UserFaultAllocator<'a>` qui marche `self.user_as.pml4_phys()`), mais il n'est jamais utilisé dans `do_page_fault`.

---

## B-02 — L'arbre VMA n'est pas cloné lors d'un fork (CRITIQUE)

### Fichier
`kernel/src/memory/virtual/address_space/fork_impl.rs` — `clone_cow()`

### Description

```rust
let child_as = match try_box_new(UserAddressSpace::new(child_pml4_phys, 0)) {
    Some(addr_space) => addr_space,
    ...
};
if inherited_heap_end != 0 {
    child_as.heap_end.store(inherited_heap_end, ...);
}
```

`UserAddressSpace::new()` crée un `VmaTree::new()` **vide**. Seul `heap_end` est transmis.

Le processus enfant hérite des pages physiques (les entrées PTE sont correctement dupliquées en CoW par `clone_userspace_tables`), mais **aucune VMA** n'est copiée dans son `UserAddressSpace`.

### Conséquence exacte

Chaque `#PF` dans l'enfant passe par :

```rust
if let Some(vma) = user_as.find_vma(fault_addr) {
    ctx = ctx.with_vma(vma);
}
```

`find_vma` retourne toujours `None` → le `FaultContext` n'a pas de VMA.

Dans `handle_page_fault` (handler.rs) :

```rust
let vma = match ctx.vma() {
    Some(v) => v,
    None => return FaultResult::Segfault { addr: ctx.fault_addr },
};
```

Résultat : **toute faute mémoire dans l'enfant produit immédiatement un Segfault**, y compris la première exécution de la pile d'entrée après le retour dans `fork_child_trampoline`.

Cela explique pourquoi `init: spawned` n'est jamais affiché — l'enfant (ipc_router) meurt au premier accès mémoire.

---

## B-03 — SIGSEGV jamais mis en file avant `exception_return_to_user` (CRITIQUE)

### Fichier
`kernel/src/arch/x86_64/exceptions.rs` — `do_page_fault()`

### Description

```rust
FaultResult::Segfault { addr } => {
    let _ = addr;
    if frame.from_userspace() {
        // SIGSEGV sera livré par exception_return_to_user (RÈGLE SIGNAL-01).
        // Quand process/ est intégré : process::signal::send(SIGSEGV).
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("...", frame);
    }
}
```

Le commentaire dit « Quand process/ sera intégré ». **Le `send_signal` n'est pas là.**

`exception_return_to_user` → `proc_signal_on_exception_return` vérifie `sched_tcb.has_signal_pending()`. Si aucun signal n'est en file (cas normal après B-01 ou B-02), la fonction retourne sans rien faire.

Le processus repart vers userspace à la même adresse fautive → nouveau `#PF` → même chemin → **boucle infinie** consommatrice de CPU, jamais terminée.

La fonction `send_signal_to_pid` existe bien dans `process/signal/delivery.rs`, mais elle n'est pas appelée depuis le fault handler.

---

## B-04 — TLB shootdown trop agressif dans fork (Majeur)

### Fichier
`kernel/src/memory/virtual/address_space/fork_impl.rs` — `flush_tlb_after_fork()`

### Description

```rust
fn flush_tlb_after_fork(&self, _parent_cr3: u64) {
    unsafe {
        shootdown_sync(
            TlbFlushType::All,
            crate::arch::x86_64::smp::init::smp_cpu_count(),
        );
    }
}
```

`TlbFlushType::All` invalide **toutes** les entrées TLB (kernel + user), sur **tous les CPUs** actifs, en mode synchrone (ACK attendu).

Problèmes :
1. **Performance** : seules les entrées user doivent être invalidées (`TlbFlushType::User` suffit — les pages kernel marquées CoW n'existent pas).
2. **Risque de deadlock** : `shootdown_sync` attend l'ACK de `smp_cpu_count()` CPUs. Si un CPU est en boucle spin, IRQ désactivées, ou en NMI, l'ACK ne vient jamais → le fork tient un lock et ne revient pas.

---

## B-05 — Masque RFLAGS incorrect dans fork (Mineur)

### Fichier
`kernel/src/process/lifecycle/fork.rs`

### Description

```rust
const RFLAGS_SAFE_MASK: u64 = 0x0000_0000_0020_0CD5; // CF,PF,AF,ZF,SF,DF,OF,AC,ID
```

Le commentaire liste `AC` (bit 18 = 0x40000) mais le masque est `0x200CD5` :

```
0x200CD5 = 0010 0000 0000 1100 1101 0101
```

Bit 18 (AC, Alignment Check) = 0x40000 → **absent**. AC sera toujours effacé chez l'enfant, même si le parent l'avait activé. Ce n'est pas bloquant mais viole le contrat documenté.

Valeur correcte : `0x0000_0000_0026_0CD5`.

---

## B-06 — `stack_base`/`stack_size` calculés de manière fixe dans exec (Mineur)

### Fichier
`kernel/src/process/lifecycle/exec.rs`

### Description

```rust
const DEFAULT_STACK_PAGES: u64 = 8;
const DEFAULT_STACK_SIZE: u64 = DEFAULT_STACK_PAGES * PAGE_SIZE_U64; // 32 KiB

let stack_top = elf_result.initial_stack_top;
let stack_base = (stack_top.saturating_sub(DEFAULT_STACK_SIZE)) & !(PAGE_SIZE_U64 - 1);
```

Le chargeur ELF alloue la pile (typiquement 2 MiB par défaut ou selon la `PT_GNU_STACK`). `DEFAULT_STACK_PAGES = 8` représente 32 KiB, ce qui est bien inférieur à la pile réelle. `thread.addresses.stack_base` pointe donc **au milieu de la pile réelle**.

Conséquences :
- Les vérifications de débordement de pile (`stack_base` < RSP) sont faussées.
- Si un signal utilise `sigaltstack`, les calculs d'espace de pile disponible sont incorrects.

---

## Analyse de la piste Codex (GPT-4.5)

### Ce que Codex a bien identifié

- Le `#PF` survient dans le **buddy allocator** pendant le `fork` → c'était B-01 en train de se manifester (walk de KERNEL_AS → aucune PTE → demand paging → accès au buddy allocator).
- La correction du mapping noyau bas (`early_init.rs`) a **éliminé le triple fault** initial, preuve que la carte PML4 des CR3 user était incomplète.
- La correction du clonage de la table des FDs (`try_clone_for_fork` explicite) était correcte et nécessaire.
- L'identification « le retour SYSRET est atteint mais PID1 ne logue plus » pointe précisément vers B-01.

### Ce que Codex n'a pas encore trouvé

- Codex supposait que la CoW stack fault allait « se résoudre » une fois le mapping kernel bas corrigé. En réalité, la CoW stack fault ne peut pas se résoudre avec `KERNEL_FAULT_ALLOC` (B-01) car c'est un problème structurel de l'allocateur, pas du mapping.
- B-02 (VMA non cloné) n'a pas été diagnostiqué — c'est le deuxième verrou indépendant.
- B-03 (SIGSEGV non envoyé) n'a pas été vu car B-01 et B-02 bloquent avant.

**Codex était sur le bon chemin** (la CoW stack du parent) mais s'est arrêté une couche trop haut.

---

## Ordre de priorité des corrections

```
B-01 → B-02 → B-03   (débloquer fork/exec)
B-04                  (stabilité SMP)
B-05, B-06            (exactitude, non bloquants)
```

Le fichier `claude-delta-corrections.md` détaille les correctifs concrets pour chaque bogue.
