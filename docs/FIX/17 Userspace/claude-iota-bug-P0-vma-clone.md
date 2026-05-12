# claude-iota-bug-P0-vma-clone.md

**Sévérité** : P0 — BLOQUANT  
**Fichier concerné** : `kernel/src/memory/virtual/address_space/fork_impl.rs`  
**Symptôme QEMU** : enfant SIGSEGV immédiat après fork, avant d'atteindre execve

---

## Description du bug

Dans `KernelAddressSpaceCloner::clone_cow()`, le clonage CoW duplique correctement les tables de pages (PTEs marquées read-only + FLAG_COW), mais crée un `UserAddressSpace` enfant avec un arbre VMA **vide** :

```rust
// fork_impl.rs ~ligne 190
let child_as = match try_box_new(UserAddressSpace::new(child_pml4_phys, 0)) {
    Some(addr_space) => addr_space,   // ← VMA tree vide !
    None => { ... }
};
```

`UserAddressSpace::new()` initialise `inner.vma_tree = VmaTree::new()` — aucun VMA n'est copié depuis le parent.

---

## Impact

Le page fault handler (`memory/virtual/fault/handler.rs`) commence par chercher le VMA contenant l'adresse fautive :

```rust
// handler.rs ligne 56
let vma = match ctx.find_vma(ctx.fault_addr) {
    Some(v) => v,
    None => {
        return FaultResult::Segfault { addr: ctx.fault_addr };  // ← SIGSEGV
    }
};
```

Pour que `ctx.find_vma()` réussisse, `exceptions.rs` doit avoir peuplé le contexte via le VMA tree du processus fils :

```rust
// exceptions.rs ~ligne 583-586
if let Some(vma) = user_as.find_vma(fault_addr) {
    ctx = ctx.with_vma(vma);
}
```

Avec un VMA tree vide, cette lookup retourne toujours `None` → **chaque page fault dans le fils est un SIGSEGV**.

Conséquences :
- La première écriture sur la pile du fils (CoW break attendu) → SIGSEGV
- execve ne peut jamais être atteint si le fils a d'abord besoin d'une page
- init_server voit son fils mourir, note_child_exit(), attend le délai backoff, recommence → boucle

---

## Pourquoi c'est difficile à diagnostiquer

Le fils n'atteint jamais execve : il meurt dans le trampoline ou dans les premières instructions de `_start()`. Le log QEMU montre `init: spawned ipc_router` (fork ok) mais jamais `init: ready ipc_router` — le fils est mort avant que `kill(pid, 0)` puisse réussir.

---

## Correction requise

### 1. Ajouter une méthode `clone_vma_tree` sur `UserAddressSpace`

```rust
// user.rs — à ajouter
pub fn clone_vma_tree_for_fork(&self) -> Option<VmaTree> {
    let inner = self.inner.lock();
    inner.vma_tree.clone_for_fork()   // cf. point 2
}
```

### 2. Implémenter `VmaTree::clone_for_fork()`

La méthode doit :
- Itérer tous les `VmaDescriptor` du parent
- Exclure les VMAs avec `VmaFlags::DONTCOPY` (SignalTcb, etc.)
- Remplacer `VmaFlags::WRITE` par `VmaFlags::COW` sur les VMAs partageables (cf. bug P0-mark-vma-cow)
- Allouer des nouveaux `VmaDescriptor` via le slab (pas de clone shallow des raw pointers)

```rust
// vma/tree.rs — à implémenter
pub fn clone_for_fork(&self) -> Option<VmaTree> {
    let mut new_tree = VmaTree::new();
    for vma in self.iter() {
        if vma.flags.contains(VmaFlags::DONTCOPY) {
            continue;   // SignalTcb, etc. — PROC-VMA/V-17
        }
        let mut child_vma = vma.clone_descriptor()?;  // alloc slab
        if child_vma.flags.contains(VmaFlags::WRITE)
           && !child_vma.flags.contains(VmaFlags::SHARED)
        {
            child_vma.flags |= VmaFlags::COW;
        }
        new_tree.insert(Box::into_raw(Box::new(child_vma)));
    }
    Some(new_tree)
}
```

### 3. Brancher dans `clone_cow()`

```rust
// fork_impl.rs — dans clone_cow()
let parent_vma_tree = if src_space_ptr != 0 {
    let parent_as = unsafe { &*(src_space_ptr as *const UserAddressSpace) };
    parent_as.clone_vma_tree_for_fork()
} else {
    Some(VmaTree::new())
};

let child_as = match try_box_new(UserAddressSpace::with_vma_tree(child_pml4_phys, 0, parent_vma_tree?)) {
    Some(addr_space) => addr_space,
    None => { ... }
};
```

---

## Test de validation

Après correction, le test suivant doit passer sous QEMU :

```
[QEMU E9] init: start ipc_router
[QEMU E9] init: spawned ipc_router <PID>
[QEMU E9] init: ready ipc_router <PID>      ← était absent
[QEMU E9] init: start memory_server
```

Et dans un test unitaire kernel :

```rust
#[test]
fn child_vma_tree_inherits_parent() {
    // Créer un parent avec une VMA stack
    // Cloner → child_as.vma_count() > 0
    // child_as trouve la VMA à l'adresse stack
}
```

---

## Lien avec corrections Codex

Codex a vu que le parent crashait en retour de fork (SYSRET) mais a orienté l'investigation vers la cartographie kernel dans les CR3 userspace. C'était un vrai problème (triple fault résolu), mais le blocage sous-jacent reste le VMA tree vide du fils.
