# ExoOS — Bugs Critiques — claude-gamma
## Fichier : claude-gamma-BUGS-CRITIQUES.md

---

## BUG-C1 — `KernelFaultAllocator` opère sur le mauvais espace d'adressage
**Sévérité : CRITIQUE — bloque tout CoW et demand-paging userspace**  
**Fichier** : `kernel/src/arch/x86_64/memory_iface.rs`

### Description

`KernelFaultAllocator` est l'unique implémentation de `FaultAllocator` utilisée par `do_page_fault`. Toutes ses opérations critiques sont câblées sur `KERNEL_AS` (espace d'adressage du noyau, initialisé au boot) :

```rust
// memory_iface.rs — lignes 277-313
fn map_page(&self, virt: VirtAddr, ...) -> Result<(), AllocError> {
    unsafe { crate::memory::virt::address_space::KERNEL_AS.map(virt, frame, flags, self) }
}
fn remap_flags(&self, virt: VirtAddr, ...) -> Result<(), AllocError> {
    let pml4 = crate::memory::virt::address_space::KERNEL_AS.pml4_phys(); // ← KERNEL_AS !
    let mut walker = PageTableWalker::new(pml4);
    walker.remap_flags(virt, flags)
}
fn read_pte_raw(&self, virt: VirtAddr) -> u64 {
    let walker = PageTableWalker::new(
        crate::memory::virt::address_space::KERNEL_AS.pml4_phys(), // ← KERNEL_AS !
    );
    walker.read_pte_raw(virt)
}
```

### Pourquoi c'est cassé

Sur x86_64, quand un processus utilisateur déclenche un `#PF`, le CPU reste avec le **CR3 du processus courant** chargé (pas de KPTI ici). Le handler Rust s'exécute avec ce CR3.

Mais `PageTableWalker::new(KERNEL_AS.pml4_phys())` marche les tables de pages du **PML4 du noyau** (celui créé au boot), pas du processus courant. Les adresses virtuelles utilisateur (< `0x0000_8000_0000_0000`) n'existent PAS dans le PML4 du noyau.

Conséquences :
- `read_pte_raw(user_virt)` → retourne `0` (PTE inexistante dans KERNEL_AS)
- Dans `handle_cow_fault` : `old_frame = None` → fallback vers `demand_paging`
- `handle_demand_paging` appelle `alloc.map_page(user_virt, new_frame, ...)` → essaie de mapper une adresse utilisateur dans le PML4 noyau → la PTE allouée ne sert à rien pour le processus courant

Le processus courant ne voit jamais la correction de sa page table. Son `#PF` ne peut jamais être résolu. Il reçoit un SIGSEGV, ou le noyau boucle indéfiniment.

### Lien avec le diagnostic Codex

Codex a observé : *"fault vient du buddy allocator"* — `handle_demand_paging` tente d'insérer une nouvelle page dans `KERNEL_AS` pour une adresse user, et échoue (espace mal initialisé, assertions internes, panic sur le walker).

### Correction requise

Le `FaultAllocator` doit opérer sur le **CR3 du processus courant**. La solution propre est de passer le `pml4_phys` du processus courant au moment de la construction du walker dans chaque opération :

```rust
// Lire le CR3 courant directement depuis le registre
#[inline(always)]
fn current_pml4_phys() -> PhysAddr {
    let cr3: u64;
    unsafe {
        core::arch::asm!("mov {v}, cr3", v = out(reg) cr3,
            options(nostack, nomem, preserves_flags));
    }
    PhysAddr::new(cr3 & !0xFFFu64)
}
```

Puis remplacer tous les `KERNEL_AS.pml4_phys()` dans `KernelFaultAllocator` par `current_pml4_phys()`.

**Voir le patch complet dans `claude-gamma-PATCHES.md` — PATCH-C1.**

---

## BUG-C2 — `VmaTree` non cloné lors du `fork()`
**Sévérité : CRITIQUE — tous les processus fils SEGFAULT immédiatement**  
**Fichier** : `kernel/src/memory/virtual/address_space/fork_impl.rs`

### Description

Dans `clone_cow()`, le processus fils reçoit un `UserAddressSpace` neuf avec un `VmaTree` vide :

```rust
// fork_impl.rs
let child_as = match try_box_new(UserAddressSpace::new(child_pml4_phys, 0)) {
    Some(addr_space) => addr_space,
    // ...
};
```

`UserAddressSpace::new()` initialise `vma_tree: VmaTree::new()` — arbre vide, zéro VMA.

### Pourquoi c'est cassé

La séquence dans `do_page_fault` :

```rust
// exceptions.rs (do_page_fault)
if let Some(vma) = user_as.find_vma(fault_addr) {
    ctx = ctx.with_vma(vma);
}
// Si find_vma retourne None → vma_ptr est null
```

Et dans `handle_page_fault` :

```rust
// handler.rs
let vma = match ctx.find_vma(ctx.fault_addr) {
    Some(v) => v,
    None => {
        FAULT_STATS.not_mapped.fetch_add(1, Ordering::Relaxed);
        return FaultResult::Segfault { addr: ctx.fault_addr };  // ← toujours ici pour le fils
    }
};
```

Pour tout processus fils, `user_as.find_vma()` retourne toujours `None` (VmaTree vide). Chaque `#PF` → `Segfault`. Le fils meurt dès qu'il touche sa pile, son code, ou tente un CoW break.

### VmaTree n'implémente pas Clone

```bash
grep -n "Clone\|clone" kernel/src/memory/virtual/vma/tree.rs
# → aucun résultat
```

`VmaTree` n'a pas de `#[derive(Clone)]` ni de méthode `clone()`. Il faut l'ajouter.

### Correction requise

1. Implémenter `Clone` pour `VmaTree` (copie complète de l'arbre AVL avec flags `COW`)
2. Appeler la copie dans `clone_cow()` après la création de `child_as`

```rust
// Dans fork_impl.rs, après la création de child_as :
let parent_as = unsafe { &*(src_space_ptr as *const UserAddressSpace) };
child_as.clone_vma_tree_from(parent_as);
```

**Voir le patch complet dans `claude-gamma-PATCHES.md` — PATCH-C2.**

---

## BUG-C3 — stdin/stdout/stderr jamais ouverts pour init ni ses enfants
**Sévérité : CRITIQUE — le shell ne peut ni lire ni écrire**  
**Fichier** : `kernel/src/process/core/pcb.rs` + `kernel/src/userspace_boot.rs`

### Description

`install_std_fds()` est définie dans `pcb.rs` mais n'est **jamais appelée** dans tout le codebase :

```bash
grep -rn "install_std_fds" kernel/src/
# kernel/src/process/core/pcb.rs:158:    pub fn install_std_fds(&mut self, stdin: u64, stdout: u64, stderr: u64) {
# → aucun call-site
```

Le processus `init` (PID 1) démarre avec une table de FDs **entièrement vide**. Tous ses fils (ipc_router, memory_server…, exosh) héritent via `try_clone_for_fork()` de cette table vide.

### Pourquoi c'est cassé

`servers/exosh/src/main.rs` fait directement :

```rust
fn read_byte_blocking() -> u8 {
    syscall::syscall3(SYS_READ, STDIN /*=0*/, ...)
}

fn write_all(bytes: &[u8]) {
    syscall::syscall3(SYS_WRITE, /*fd=*/1, ...)
}
```

FD 0 et FD 1 n'étant jamais installés, `SYS_READ(0, ...)` et `SYS_WRITE(1, ...)` retournent `EBADF`. La boucle REPL du shell est muette et aveugle.

### Correction requise

Lors du bootstrap de `init` dans `userspace_boot.rs`, ouvrir `/dev/console` (ou un handle TTY fourni par `tty_server`) et l'installer aux FDs 0, 1, 2 avant l'enqueue du thread :

```rust
// userspace_boot.rs — dans boot_userspace(), après create_init_process_from_elf
// (voir PATCH-C3 pour l'implémentation complète)
pcb.files.lock().install_std_fds(tty_handle, tty_handle, tty_handle);
```

**Voir le patch complet dans `claude-gamma-PATCHES.md` — PATCH-C3.**

---

## BUG-C4 — Aucun pont TTY server ↔ FDs 0/1/2
**Sévérité : CRITIQUE — même avec des FDs ouverts, rien n'est connecté au terminal**  
**Fichiers** : `servers/tty_server/src/main.rs` + absence d'un pilote de device virtuel TTY

### Description

Le `tty_server` est un serveur IPC pur. Il reçoit des messages `TTY_MSG_INPUT_BYTE`, `TTY_MSG_READ_LINE`, `TTY_MSG_WRITE`... mais il n'expose **aucun fichier de device** (pas de `/dev/tty`, pas de `/dev/console`).

`exosh` lit depuis `fd=0` via `SYS_READ`. Ce SYS_READ est traité par le kernel → `fs_bridge.rs` → VFS server → ExoFS. ExoFS n'a aucune connaissance du TTY server.

Il n'y a **aucune chaîne fonctionnelle** entre :
- `input_server` (reçoit les keystrokes depuis le pilote PS/2 ou virtio-input)
- `tty_server` (line discipline, écho)
- `exosh` (shell qui lit `fd=0`)

### Correction requise

Deux approches :

**Option A (recommandée à court terme)** : Faire lire exosh via IPC directement depuis `tty_server` au lieu de `fd=0`. Modifier `read_byte_blocking()` pour envoyer `TTY_MSG_READ_LINE` via IPC et récupérer le résultat. Cela évite de créer toute l'infrastructure de device.

**Option B (complète)** : Créer un "tty device" virtuel dans le VFS. Quand on ouvre `/dev/tty`, on obtient un FD dont les `read()`/`write()` sont routés par le kernel vers le TTY server via IPC. C'est l'approche POSIX correcte mais nécessite plus de travail.

**Voir les deux options dans `claude-gamma-PATCHES.md` — PATCH-C4.**

---

## Résumé des bugs critiques

| ID | Fichier principal | Impact | Correction |
|---|---|---|---|
| BUG-C1 | `memory_iface.rs` | CoW/demand-paging cassés | Utiliser CR3 courant |
| BUG-C2 | `fork_impl.rs` | Fils SEGFAULT immédiat | Cloner VmaTree |
| BUG-C3 | `userspace_boot.rs` | Shell aveugle/muet | Appeler `install_std_fds` |
| BUG-C4 | `tty_server` + fs_bridge | Pas de terminal physique | Pont TTY↔FD |

**Tous les quatre doivent être corrigés avant que le shell puisse fonctionner.**
