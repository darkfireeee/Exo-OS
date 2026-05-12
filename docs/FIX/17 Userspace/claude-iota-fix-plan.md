# claude-iota-fix-plan.md

**Objectif** : Rendre le terminal/shell opérationnel  
**Prérequis** : Corrections Codex déjà appliquées (triple fault, FD-clone, fs_bridge)

---

## Ordre de correction

### Étape 1 — P0 : Implémenter VmaTree::clone_for_fork() [~2-3h]

**Fichiers à modifier** :
- `kernel/src/memory/virtual/vma/tree.rs` — ajouter `clone_for_fork()` et `iter()` / `iter_mut()`
- `kernel/src/memory/virtual/vma/descriptor.rs` — ajouter `clone_descriptor()` (alloc slab)
- `kernel/src/memory/virtual/address_space/user.rs` — ajouter `clone_vma_tree_for_fork()` et `mark_all_writable_vmas_cow()`

**Implémentation minimale** :

```rust
// vma/tree.rs
impl VmaTree {
    pub fn clone_for_fork(&self) -> Option<VmaTree> {
        let mut new_tree = VmaTree::new();
        // Itérer dans l'ordre croissant d'adresse
        let mut node = self.first();
        while let Some(vma) = node {
            if !vma.flags.contains(VmaFlags::DONTCOPY) {
                let mut child = vma.shallow_clone()?;  // alloc + memcpy
                if child.flags.contains(VmaFlags::WRITE)
                   && !child.flags.contains(VmaFlags::SHARED) {
                    child.flags |= VmaFlags::COW;
                    child.page_flags = child.page_flags
                        & !PageFlags::WRITABLE
                        | PageFlags::COW;
                }
                new_tree.insert_unchecked(child);
            }
            node = vma.next();
        }
        Some(new_tree)
    }
}
```

**Note** : `VmaTree` est probablement une BST ou liste chaînée par adresse. L'itérateur doit être ajouté si absent.

---

### Étape 2 — P0 : Appeler mark_vma_cow() sur le parent [~30min]

**Fichier** : `kernel/src/memory/virtual/address_space/fork_impl.rs`

Juste après `clone_userspace_tables()`, marquer les VMAs du parent :

```rust
// fork_impl.rs dans clone_cow()
unsafe {
    let src_pml4 = phys_to_table_ref(PhysAddr::new(src_cr3));
    let dst_pml4 = phys_to_table_mut(child_pml4_phys);
    // ... copie PML4[256:512] kernel ...
    if clone_userspace_tables(...).is_err() {
        free_userspace_tables(child_pml4_phys);
        return Err(AddrSpaceCloneError::OutOfMemory);
    }
}

// NOUVEAU : marquer les VMAs parent en CoW
if src_space_ptr != 0 {
    let parent_as = unsafe { &*(src_space_ptr as *const UserAddressSpace) };
    parent_as.mark_all_writable_vmas_cow();
}

// NOUVEAU : cloner le VMA tree dans l'AS enfant
let parent_vma_tree = if src_space_ptr != 0 {
    let parent_as = unsafe { &*(src_space_ptr as *const UserAddressSpace) };
    parent_as.clone_vma_tree_for_fork()
        .ok_or(AddrSpaceCloneError::OutOfMemory)?
} else {
    VmaTree::new()
};

let child_as = match try_box_new(UserAddressSpace::with_vma_tree(child_pml4_phys, 0, parent_vma_tree)) {
    Some(a) => a,
    None => {
        unsafe { free_userspace_tables(child_pml4_phys); }
        return Err(AddrSpaceCloneError::OutOfMemory);
    }
};
```

---

### Étape 3 — P1 : Corriger RFLAGS_FORCE_CLR [5min]

```rust
// fork.rs
const RFLAGS_FORCE_CLR: u64 = 0x0000_0000_0003_4100; // TF | NT | RF | VM
```

---

### Étape 4 — P1 : Corriger post_dispatch après execve raté [15min]

```rust
// dispatch.rs
if effective_nr == crate::syscall::numbers::SYS_EXECVE {
    handle_execve_inplace(frame);
    let result = frame.rax as i64;
    if result < 0 {
        post_dispatch(frame, tsc_start);
    }
    return;
}
```

---

### Étape 5 — Test QEMU [après étapes 1-4]

Lancer QEMU avec les logs E9 et vérifier la séquence attendue :

```
init_server: PID1 online
init_server: registering control endpoint
init_server: starting service graph
init: start ipc_router
init: spawned ipc_router 2
init: ready ipc_router 2         ← VALIDATION P0 résolus
init: start memory_server
init: spawned memory_server 3
init: ready memory_server 3
...
init: start exosh
init: spawned exosh 12
$ _                               ← OBJECTIF FINAL
```

---

### Étape 6 — P2 : IPC readiness robuste [facultatif pour démarrage]

Peut être différé après le premier shell fonctionnel. Implémenter le mécanisme de handshake décrit dans `P2-ipc-ready.md`.

---

## Checklist de régression

Après chaque étape :

- [ ] `cargo check --workspace` sans warning
- [ ] `cargo test --workspace` passe (hors cibles bare-metal)
- [ ] `make test-exofs` : 2833 tests OK
- [ ] Boot QEMU : kernel atteint init_server sans triple fault
- [ ] [Étape 1+2] premier service forké ne SIGSEGV plus
- [ ] [Étape 1+2] `init: ready ipc_router` apparaît dans les logs E9
- [ ] [Final] shell exosh répond à une commande (ex: `echo hello`)

---

## Architecture du correctif — vue globale

```
fork_impl.rs::clone_cow()
    │
    ├─ clone_userspace_tables()     [existant - correct]
    │   marque PTEs: read-only + FLAG_COW
    │
    ├─ parent_as.mark_all_writable_vmas_cow()  [NOUVEAU]
    │   marque VMAs parent: VmaFlags::COW
    │
    └─ child_as = UserAddressSpace::with_vma_tree(
           parent_as.clone_vma_tree_for_fork()   [NOUVEAU]
       )
       VMAs enfant: héritage + VmaFlags::COW sur writables

Résultat :
    Parent write fault → VmaFlags::COW présent → handle_cow_fault() ✓
    Enfant write fault → VMA trouvée + COW → handle_cow_fault() ✓
    Enfant execve     → ElfLoader remplace l'AS complet → VMA tree recréé ✓
```

---

## Risques et précautions

1. **Allocation slab en contexte IRQ-off** : s'assurer que `VmaDescriptor::shallow_clone()` utilise un allocateur qui ne bloque pas les IRQs. Utiliser `AllocFlags::NO_WAIT` si disponible.

2. **Ordering du mark_vma_cow** : marquer le parent AVANT de créer l'enfant, sinon une window race entre le marquage PTE (read-only) et le marquage VMA (COW flag) peut laisser le parent dans un état incohérent.

3. **VmaTree::iter() thread-safety** : `clone_vma_tree_for_fork()` doit tenir le lock du VMA tree pendant toute l'itération, ou prendre un snapshot. Ne pas déverrouiller entre deux entrées.

4. **DONTCOPY et SignalTcb** : Les VMAs avec `VmaFlags::DONTCOPY` (SignalTcb, positionné par `do_execve` via PROC-VMA/V-17) doivent être **exclues** du clone. Vérifier que `DONTCOPY` est bien positionné avant que `clone_cow` soit appelé.
