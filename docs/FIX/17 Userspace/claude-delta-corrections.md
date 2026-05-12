# Exo-OS — Guide de corrections Claude-Delta
## Plan d'action concret pour débloquer le terminal/shell

---

## Correction C-01 — `UserFaultAllocator` dans `do_page_fault`  
**Priorité : CRITIQUE · Débloque : fork parent CoW, exec stack**

### Problème
`KERNEL_FAULT_ALLOC` marche `KERNEL_AS.pml4_phys()` pour les fautes userspace.  
Le patron correct existe dans `drivers/dma.rs` mais n'est pas branché au fault handler.

### Correctif

#### Étape 1 — Créer un `UserFaultAllocator` partageable dans `memory_iface.rs`

```rust
// kernel/src/arch/x86_64/memory_iface.rs

use crate::memory::virt::address_space::UserAddressSpace;

/// Allocateur de fautes lié à un UserAddressSpace.
/// Utilisé par do_page_fault() pour les fautes Ring 3.
pub struct UserFaultAllocator<'a> {
    pub user_as: &'a UserAddressSpace,
}

impl FrameAllocatorForWalk for UserFaultAllocator<'_> {
    fn alloc_frame(&self, flags: AllocFlags) -> Result<Frame, AllocError> {
        alloc_page(flags)
    }
    fn free_frame(&self, frame: Frame) {
        let _ = free_page(frame);
    }
}

impl FaultAllocator for UserFaultAllocator<'_> {
    fn alloc_zeroed(&self) -> Result<Frame, AllocError> {
        alloc_page(AllocFlags::ZEROED)
    }
    fn alloc_nonzeroed(&self) -> Result<Frame, AllocError> {
        alloc_page(AllocFlags::NONE)
    }
    fn free_frame(&self, f: Frame) {
        let _ = free_page(f);
    }
    fn map_page(&self, virt: VirtAddr, frame: Frame, flags: PageFlags) -> Result<(), AllocError> {
        unsafe { self.user_as.map_page(virt, frame, flags, self) }
    }
    fn remap_flags(&self, virt: VirtAddr, flags: PageFlags) -> Result<(), AllocError> {
        let mut walker = crate::memory::virt::page_table::PageTableWalker::new(
            self.user_as.pml4_phys(),
        );
        walker.remap_flags(virt, flags)
    }
    fn translate(&self, virt: VirtAddr) -> Option<PhysAddr> {
        self.user_as.translate(virt)
    }
    fn read_pte_raw(&self, virt: VirtAddr) -> u64 {
        let walker = crate::memory::virt::page_table::PageTableWalker::new(
            self.user_as.pml4_phys(),
        );
        walker.read_pte_raw(virt)
    }
    fn compare_exchange_pte_raw(
        &self, virt: VirtAddr, current: u64, new: u64,
    ) -> Result<(), u64> {
        let walker = crate::memory::virt::page_table::PageTableWalker::new(
            self.user_as.pml4_phys(),
        );
        unsafe { walker.compare_exchange_leaf_raw(virt, current, new) }
    }
}
```

#### Étape 2 — Modifier `do_page_fault` pour utiliser `UserFaultAllocator`

Dans `kernel/src/arch/x86_64/exceptions.rs`, remplacer la partie dispatch :

```rust
// --- AVANT ---
let result = crate::memory::virt::fault::handler::handle_page_fault(
    &ctx,
    &KERNEL_FAULT_ALLOC,
);

// --- APRÈS ---
use super::memory_iface::{UserFaultAllocator, KERNEL_FAULT_ALLOC};

let result = if !from_kernel {
    // Faute Ring 3 : utiliser l'espace d'adressage du processus courant.
    let tcb_raw = unsafe { super::smp::percpu::read_current_tcb() };
    let user_as_opt = if tcb_raw != 0 {
        let tcb = unsafe {
            &*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock)
        };
        let pcb = crate::process::core::registry::PROCESS_REGISTRY
            .find_by_pid(crate::process::core::pid::Pid(tcb.pid.0));
        pcb.and_then(|p| {
            let ptr = p.address_space_ptr();
            if ptr.is_null() { None }
            else { Some(unsafe { &*(ptr as *const crate::memory::virt::address_space::UserAddressSpace) }) }
        })
    } else {
        None
    };

    match user_as_opt {
        Some(user_as) => {
            let alloc = UserFaultAllocator { user_as };
            crate::memory::virt::fault::handler::handle_page_fault(&ctx, &alloc)
        }
        None => {
            // Pas de processus courant — tomber sur le kernel alloc (ex: idle).
            crate::memory::virt::fault::handler::handle_page_fault(&ctx, &KERNEL_FAULT_ALLOC)
        }
    }
} else {
    // Faute Ring 0 : kernel fault alloc.
    crate::memory::virt::fault::handler::handle_page_fault(&ctx, &KERNEL_FAULT_ALLOC)
};
```

> **Note** : La construction de `ctx` (lookup VMA) peut rester identique car elle utilise déjà le bon `user_as`. Seule la partie `handle_page_fault` change d'allocateur.

---

## Correction C-02 — Cloner l'arbre VMA dans `fork_impl.rs`  
**Priorité : CRITIQUE · Débloque : tout accès mémoire de l'enfant**

### Problème
`UserAddressSpace::new()` crée un `VmaTree` vide. L'enfant n'a pas de VMAs → tout `#PF` → Segfault.

### Correctif

#### Étape 1 — Ajouter `clone_vma_tree()` à `VmaTree`

```rust
// kernel/src/memory/virtual/vma/tree.rs

impl VmaTree {
    /// Clone toutes les VMAs de cet arbre vers `dst`.
    /// Les VMAs sont copiées avec leurs flags (le bit COW est conservé pour les
    /// VMAs privées writable — la couche CoW les traitera à la première écriture).
    pub fn clone_into(&self, dst: &mut VmaTree) {
        // Itérer sur l'arbre source en ordre croissant.
        let mut iter = self.iter();
        while let Some(vma_ref) = iter.next() {
            // Allouer un nouveau VmaDescriptor (clone du src).
            if let Some(new_vma) = alloc::boxed::Box::try_new(vma_ref.clone()).ok() {
                let raw = alloc::boxed::Box::into_raw(new_vma);
                // SAFETY: raw provient de Box::into_raw — unique et valide.
                unsafe { dst.insert(raw); }
            }
            // En cas d'échec d'allocation, on continue — la VMA sera manquante,
            // ce qui causera un Segfault à l'accès. Acceptable en condition OOM.
        }
    }
}
```

> Cela nécessite que `VmaDescriptor` implémente `Clone`. Si ce n'est pas encore le cas,
> ajouter `#[derive(Clone)]` à la struct (les champs atomiques devront être clonés manuellement).

#### Étape 2 — Appeler `clone_vma_tree()` dans `clone_cow()`

```rust
// kernel/src/memory/virtual/address_space/fork_impl.rs
// Dans KernelAddressSpaceCloner::clone_cow(), après la création de child_as :

// Cloner les VMAs du parent vers l'enfant.
if src_space_ptr != 0 {
    let parent_as = unsafe { &*(src_space_ptr as *const UserAddressSpace) };
    let mut child_inner = child_as.inner.lock();
    let parent_inner = parent_as.inner.lock();
    parent_inner.vma_tree.clone_into(&mut child_inner.vma_tree);
    // Transmettre aussi le hint mmap pour que les futurs mmap() de l'enfant
    // commencent au bon endroit.
    child_inner.mmap_hint = parent_inner.mmap_hint;
}
```

> **Attention** : veiller à ne pas tenir les deux locks simultanément si `inner` est le même
> (cas vfork — mais vfork partage le même AS donc clone_cow n'est pas appelé).

---

## Correction C-03 — Envoyer SIGSEGV avant `exception_return_to_user`  
**Priorité : CRITIQUE · Débloque : terminaison propre des fautes, pas de boucle infinie**

### Problème
Quand `handle_page_fault` retourne `FaultResult::Segfault`, aucun signal n'est mis en file.  
Le processus recommence à la même adresse → boucle infinie `#PF`.

### Correctif

Dans `kernel/src/arch/x86_64/exceptions.rs`, patcher le bloc Segfault :

```rust
FaultResult::Segfault { addr } => {
    let _ = addr;
    if frame.from_userspace() {
        // ← AJOUT : mettre SIGSEGV en file AVANT exception_return_to_user
        let tcb_raw = unsafe { super::smp::percpu::read_current_tcb() };
        if tcb_raw != 0 {
            let tcb = unsafe {
                &*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock)
            };
            let pid = crate::process::core::pid::Pid(tcb.pid.0);
            let _ = crate::process::signal::delivery::send_signal_to_pid(
                pid,
                crate::process::signal::default::Signal::SIGSEGV,
            );
        }
        // exception_return_to_user livrera le signal via proc_signal_on_exception_return.
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#PF kernel : accès invalide", frame);
    }
}
```

Même logique pour `FaultResult::Oom` → envoyer `SIGKILL` (ou `SIGSEGV` si l'OOM killer n'est pas encore câblé).

---

## Correction C-04 — TLB flush sélectif dans fork  
**Priorité : Majeur · Débloque : stabilité SMP, performance**

### Correctif

```rust
// kernel/src/memory/virtual/address_space/fork_impl.rs

fn flush_tlb_after_fork(&self, _parent_cr3: u64) {
    unsafe {
        // AVANT : TlbFlushType::All (invalide tout, kernel compris)
        // APRÈS : TlbFlushType::User (seules les entrées user doivent être invalidées)
        shootdown_sync(
            TlbFlushType::User,
            crate::arch::x86_64::smp::init::smp_cpu_count(),
        );
    }
}
```

Si `TlbFlushType::User` n'existe pas encore, l'ajouter dans le module `tlb` :

```rust
pub enum TlbFlushType {
    All,
    User,   // invalide uniquement les entrées avec bit U/S=1
    Single(VirtAddr),
}
```

Et dans le handler IPI shootdown, implémenter le flush sélectif :
```rust
TlbFlushType::User => {
    // Reload CR3 avec le bit "no-flush" clair = flush complet des entrées user PCID.
    let cr3: u64;
    core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nostack, nomem, preserves_flags));
    // Effacer le bit 63 (no-flush) pour forcer un TLB flush user.
    core::arch::asm!("mov cr3, {}", in(reg) cr3 & !(1u64 << 63), options(nostack, nomem));
}
```

---

## Correction C-05 — Masque RFLAGS dans fork  
**Priorité : Mineur**

```rust
// kernel/src/process/lifecycle/fork.rs

// AVANT
const RFLAGS_SAFE_MASK: u64 = 0x0000_0000_0020_0CD5; // manque AC (bit 18)

// APRÈS
const RFLAGS_SAFE_MASK: u64 = 0x0000_0000_0026_0CD5; // CF,PF,AF,ZF,SF,DF,OF,AC,ID
//                                              ^^
//                                              0x40000 = AC (Alignment Check, bit 18)
```

---

## Correction C-06 — `stack_base` dans exec  
**Priorité : Mineur**

Exposer la taille réelle de la pile depuis `ElfLoadResult` :

```rust
// kernel/src/process/lifecycle/exec.rs — struct ElfLoadResult

pub struct ElfLoadResult {
    // ... champs existants ...
    /// Adresse de base de la pile (bottom de la région mappée).
    pub stack_base: u64,
    /// Taille totale de la pile mappée.
    pub stack_size: usize,
}
```

Et dans `do_execve`, utiliser directement ces champs :

```rust
// AVANT
let stack_top = elf_result.initial_stack_top;
let stack_base = (stack_top.saturating_sub(DEFAULT_STACK_SIZE)) & !(PAGE_SIZE_U64 - 1);
let stack_size = stack_top.saturating_sub(stack_base);

// APRÈS
let stack_base = elf_result.stack_base;
let stack_size = elf_result.stack_size as u64;
```

> Le chargeur ELF (`fs/`) doit peupler ces champs. Il connaît la taille réelle de la pile
> (`PT_GNU_STACK` ou la valeur par défaut de la plateforme).

---

## Séquence de validation recommandée

```
1. Appliquer C-01 (UserFaultAllocator)
   → boot QEMU : vérifier que PID1 passe fork() et que parent reprend

2. Appliquer C-02 (clone VMA)
   → boot QEMU : vérifier que l'enfant ipc_router démarre ("init: spawned")

3. Appliquer C-03 (SIGSEGV)
   → si un processus crash, il doit se terminer proprement (exit, pas boucle)

4. Appliquer C-04 (TLB flush sélectif)
   → test de stabilité multi-fork sur SMP

5. Appliquer C-05, C-06
   → cargo check --workspace ; boot QEMU final ; exosh prompt
```

---

## Points de vigilance pour Codex lors de la suite

1. **Ne pas confondre** `KERNEL_AS` et le CR3 courant — ce sont des espaces distincts.  
   Toute opération PTE sur adresse user **doit** utiliser `user_as.pml4_phys()`.

2. **Le clonage VMA** doit inclure le `mmap_hint` pour éviter que l'enfant réutilise
   des adresses déjà occupées dans son espace.

3. **`proc_signal_on_exception_return`** ne génère pas de signaux, elle livre des signaux
   déjà en file. Ne pas confondre génération et livraison.

4. **Le `flush_tlb_after_fork`** est le seul TLB flush nécessaire — ne pas en ajouter dans
   `fork_child_trampoline` (la règle PROC-08 est respectée).
