# claude-iota-bug-P0-mark-vma-cow.md

**Sévérité** : P0 — BLOQUANT  
**Fichiers concernés** :  
- `kernel/src/memory/virtual/vma/cow.rs` — `mark_vma_cow()` définie mais jamais appelée  
- `kernel/src/memory/virtual/address_space/fork_impl.rs` — CoW sans mise à jour des VMAs  
- `kernel/src/memory/virtual/fault/handler.rs` — routage conditionné sur `VmaFlags::COW`  

**Symptôme QEMU** : le parent crashe ou corrompt sa pile lors du premier accès post-fork

---

## Description du bug

### La fonction existe mais n'est jamais appelée

`memory/virtual/vma/cow.rs` définit :

```rust
pub fn mark_vma_cow(vma: &mut VmaDescriptor) {
    if vma.flags.contains(VmaFlags::WRITE) && !vma.flags.contains(VmaFlags::SHARED) {
        vma.flags |= VmaFlags::COW;          // ← marque la VMA CoW
        vma.page_flags = vma.page_flags & !PageFlags::WRITABLE | PageFlags::COW;
    }
    ...
}
```

Recherche exhaustive de tous les appels à `mark_vma_cow` dans le code source :

```
$ grep -rn "mark_vma_cow" kernel/src/
kernel/src/memory/virtual/vma/mod.rs:11:pub use cow::{..., mark_vma_cow, ...};
kernel/src/memory/virtual/vma/cow.rs:127:pub fn mark_vma_cow(vma: &mut VmaDescriptor) { ... }
```

**Résultat : zéro appel.** La fonction est déclarée, exportée, mais jamais invoquée dans le chemin fork.

---

## Conséquence sur le routage des page faults

Le page fault handler route vers `handle_cow_fault` uniquement si :

```rust
// handler.rs ligne 98
if ctx.cause == FaultCause::Write && vma.flags.contains(VmaFlags::COW) {
    return super::cow::handle_cow_fault(ctx, vma, alloc);
}
```

Puisque `mark_vma_cow()` n'est jamais appelé, `VmaFlags::COW` n'est jamais positionné sur les VMAs du parent après fork. Donc :

1. **Parent** — premier accès en écriture sur la pile (page PTE = read-only + FLAG_COW) :
   - `FaultCause::Write`, VMA a `VmaFlags::WRITE` mais pas `VmaFlags::COW`
   - Permis (WRITE présent) → ne va PAS vers `handle_cow_fault`
   - Tombe sur `demand_paging::handle_demand_paging()`
   - `demand_paging` sur une page **déjà présente** → appelle `alloc_zero_map()` → alloue un nouveau frame **ET tente de l'insérer à l'adresse déjà mappée**
   - Résultat : soit double-mapping silencieux (data de la pile écrasée par des zéros), soit erreur selon l'implémentation de `map_page()` avec page existante

2. **Fils** — même accès (après correction du VMA tree clone, cf. bug P0-vma-clone) :
   - `VmaFlags::COW` absent → même chemin demand_paging → même corruption

---

## Chaîne de corruption détaillée (parent)

```
fork() retour parent → SYSRETQ → pile userspace (read-only CoW)
  │
  ▼ 1ère push/pop sur la pile
#PF Write @ 0x7FFFFF...
  │
  ▼ exceptions.rs do_page_fault
FaultContext { cause: Write, vma: stack_vma (WRITE, no COW) }
  │
  ▼ handler.rs handle_page_fault
Permission check: WRITE flag présent → OK
CoW check: VmaFlags::COW absent → SKIP handle_cow_fault
  │
  ▼ demand_paging (MAUVAIS CHEMIN)
alloc_zero_map() → alloue frame vierge → map_page()
  │
  Selon l'impl de map_page sur page présente :
  ├─ Option A : ignore l'existant → nouvelle page zéro écrite
  │            → données de retour corrompues → segfault différé
  └─ Option B : erreur AllocError → FaultResult::Oom → kill parent
```

---

## Correction requise

### Appeler `mark_vma_cow()` sur chaque VMA writable lors du clone

La correction s'intègre dans le `clone_vma_tree_for_fork()` décrit dans `P0-vma-clone` :

```rust
// vma/tree.rs — clone_for_fork()
for vma_raw in self.iter() {
    if vma_raw.flags.contains(VmaFlags::DONTCOPY) {
        continue;
    }
    let mut child_vma = vma_raw.clone_descriptor()?;
    
    // Marquer les VMAs writables en CoW
    if child_vma.flags.contains(VmaFlags::WRITE)
       && !child_vma.flags.contains(VmaFlags::SHARED)
    {
        mark_vma_cow(&mut child_vma);   // ← appel manquant
    }
    new_tree.insert(Box::into_raw(Box::new(child_vma)));
}
```

Et sur le parent, dans `clone_userspace_tables()`, après avoir marqué les PTEs en CoW, itérer les VMAs du parent pour les marquer également :

```rust
// fork_impl.rs — dans clone_cow(), après clone_userspace_tables()
if src_space_ptr != 0 {
    let parent_as = unsafe { &*(src_space_ptr as *const UserAddressSpace) };
    parent_as.mark_all_writable_vmas_cow();   // nouvelle méthode
}
```

```rust
// user.rs — nouvelle méthode
pub fn mark_all_writable_vmas_cow(&self) {
    let mut inner = self.inner.lock();
    for vma in inner.vma_tree.iter_mut() {
        mark_vma_cow(vma);
    }
}
```

---

## Validation

Test unitaire :

```rust
#[test]
fn parent_vma_has_cow_flag_after_fork() {
    // Setup: process with WRITE stack VMA
    // After clone_cow():
    //   parent VMA flags must contain VmaFlags::COW
    //   child  VMA flags must contain VmaFlags::COW
    assert!(parent_stack_vma.flags.contains(VmaFlags::COW));
    assert!(child_stack_vma.flags.contains(VmaFlags::COW));
}

#[test]
fn parent_write_fault_routes_to_cow_handler_after_fork() {
    // Write fault on CoW page → handle_cow_fault() called, not demand_paging()
}
```

---

## Note sur le design existant

`mark_vma_cow()` est bien conçue — elle est exactement ce dont on a besoin. La bonne nouvelle est que la logique CoW dans `cow.rs` (break, copy, remap) est correcte. Le seul maillon manquant est **l'initialisation** : déclencher `mark_vma_cow()` au bon moment dans le chemin fork.
