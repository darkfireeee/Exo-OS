# ExoOS — Guide de correction P0 : BUG-SHELL-01 + BUG-SHELL-02
**Date :** 2026-05-07 — Claude Alpha  
**Dépendances :** À appliquer après lecture de `claude-alpha-shell-audit.md`

---

## CORRECTION 1 : `UserFaultAllocator` — Fault handler process-aware

### Étape 1 : Créer `kernel/src/arch/x86_64/user_fault_alloc.rs`

```rust
// kernel/src/arch/x86_64/user_fault_alloc.rs
//
// FaultAllocator pour les faults Ring 3 : opère sur le CR3 du processus,
// PAS sur KERNEL_AS.

use crate::memory::core::{AllocError, AllocFlags, Frame, PageFlags, PhysAddr, VirtAddr};
use crate::memory::physical::{alloc_page, free_page};
use crate::memory::virt::fault::handler::FaultAllocator;
use crate::memory::virt::page_table::FrameAllocatorForWalk;
use crate::memory::virt::page_table::PageTableWalker;
use crate::memory::virt::UserAddressSpace;

pub struct UserFaultAllocator {
    /// Adresse physique de la PML4 du processus courant.
    pml4_phys: crate::memory::core::PhysAddr,
    /// Pointeur vers l'UserAddressSpace (pour map_page).
    user_as: *const UserAddressSpace,
}

// SAFETY: Le fault handler est mono-thread par CPU (CLI implicite dans le handler).
unsafe impl Send for UserFaultAllocator {}
unsafe impl Sync for UserFaultAllocator {}

impl UserFaultAllocator {
    /// Construit un allocateur lié à l'espace d'adressage fourni.
    ///
    /// # Safety
    /// `user_as` doit être valide pour toute la durée du fault handler.
    pub unsafe fn new(user_as: &UserAddressSpace) -> Self {
        Self {
            pml4_phys: user_as.pml4_phys(),
            user_as: user_as as *const _,
        }
    }
}

impl FrameAllocatorForWalk for UserFaultAllocator {
    fn alloc_frame(&self, flags: AllocFlags) -> Result<Frame, AllocError> {
        alloc_page(flags)
    }
    fn free_frame(&self, frame: Frame) {
        let _ = free_page(frame);
    }
}

impl FaultAllocator for UserFaultAllocator {
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
        // SAFETY: user_as valide (voir constructeur), virt dans espace user.
        unsafe { (*self.user_as).map_page(virt, frame, flags, self) }
    }

    fn remap_flags(&self, virt: VirtAddr, flags: PageFlags) -> Result<(), AllocError> {
        let mut walker = PageTableWalker::new(self.pml4_phys);
        walker.remap_flags(virt, flags)
    }

    fn translate(&self, virt: VirtAddr) -> Option<PhysAddr> {
        // SAFETY: user_as valide.
        unsafe { (*self.user_as).translate(virt) }
    }

    fn read_pte_raw(&self, virt: VirtAddr) -> u64 {
        let walker = PageTableWalker::new(self.pml4_phys);
        walker.read_pte_raw(virt)
    }

    fn compare_exchange_pte_raw(
        &self,
        virt: VirtAddr,
        current: u64,
        new: u64,
    ) -> Result<(), u64> {
        let walker = PageTableWalker::new(self.pml4_phys);
        // SAFETY: virt désigne une PTE feuille dans l'espace user.
        unsafe { walker.compare_exchange_leaf_raw(virt, current, new) }
    }
}
```

### Étape 2 : Modifier `do_page_fault()` dans `exceptions.rs`

Remplacer le dispatch final dans `do_page_fault()` :

```rust
// AVANT (à supprimer) :
let result = crate::memory::virt::fault::handler::handle_page_fault(&ctx, &KERNEL_FAULT_ALLOC);

// APRÈS :
let result = if !from_kernel && !as_ptr.is_null() {
    // SAFETY: as_ptr résolu plus haut depuis pcb.address_space_ptr().
    let user_as = unsafe {
        &*(as_ptr as *const crate::memory::virt::address_space::UserAddressSpace)
    };
    let user_alloc = unsafe {
        crate::arch::x86_64::user_fault_alloc::UserFaultAllocator::new(user_as)
    };
    crate::memory::virt::fault::handler::handle_page_fault(&ctx, &user_alloc)
} else {
    crate::memory::virt::fault::handler::handle_page_fault(&ctx, &KERNEL_FAULT_ALLOC)
};
```

**Important :** le pointeur `as_ptr` est déjà résolu dans le `do_page_fault()` existant
pour appeler `user_as.record_fault()` et `user_as.find_vma()`. Il suffit de passer le
même pointeur au nouveau `UserFaultAllocator`.

---

## CORRECTION 2 : Clonage de l'arbre VMA dans `fork()`

### Étape 1 : Ajouter `clone_for_fork()` dans `VmaTree`

Fichier : `kernel/src/memory/virtual/vma/tree.rs`

```rust
// À ajouter dans impl VmaTree

/// Clone l'arbre VMA pour un processus fils (fork POSIX).
///
/// Chaque VmaDescriptor est dupliqué tel quel — les pages CoW sont
/// déjà marquées read-only au niveau des PTEs par clone_userspace_tables().
/// Les flags VMA ne changent pas ici : c'est le CoW break qui les met à jour.
///
/// Retourne None en cas d'OOM.
pub fn clone_for_fork(&self) -> Option<VmaTree> {
    let mut new_tree = VmaTree::new();
    // iter() doit parcourir toutes les VMAs dans l'ordre croissant d'adresse.
    for vma_ref in self.iter() {
        let cloned = (*vma_ref).clone();
        if new_tree.insert(cloned).is_err() {
            return None;
        }
    }
    Some(new_tree)
}
```

*Note : si `VmaDescriptor` n'implémente pas `Clone`, ajouter `#[derive(Clone)]`
ou un constructeur de copie explicite dans `vma/descriptor.rs`.*

### Étape 2 : Ajouter `clone_inner_for_fork()` dans `UserAddressSpace`

Fichier : `kernel/src/memory/virtual/address_space/user.rs`

```rust
impl UserAddressSpace {
    // ...

    /// Clone les métadonnées internes pour un processus fils.
    ///
    /// Copie l'arbre VMA, mmap_hint et stack_bottom depuis l'espace parent.
    /// La PML4 est déjà construite par fork_impl::clone_cow().
    ///
    /// Retourne false en cas d'OOM.
    pub fn clone_inner_for_fork(&self, child: &UserAddressSpace) -> bool {
        let src = self.inner.lock();
        let mut dst = child.inner.lock();

        let Some(tree) = src.vma_tree.clone_for_fork() else {
            return false;
        };
        dst.vma_tree = tree;
        dst.mmap_hint = src.mmap_hint;
        dst.stack_bottom = src.stack_bottom;
        true
    }
}
```

### Étape 3 : Appeler `clone_inner_for_fork()` dans `fork_impl.rs`

Dans `KernelAddressSpaceCloner::clone_cow()`, après la construction de `child_as` :

```rust
// Après :
//   child_as.heap_end.store(inherited_heap_end, ...);

// Ajouter :
if src_space_ptr != 0 {
    let parent_as = unsafe { &*(src_space_ptr as *const UserAddressSpace) };
    if !parent_as.clone_inner_for_fork(&child_as) {
        unsafe { free_userspace_tables(child_pml4_phys); }
        return Err(AddrSpaceCloneError::OutOfMemory);
    }
}
```

---

## CORRECTION 3 : `flush_tlb_after_fork()` — flush ciblé

Fichier : `kernel/src/memory/virtual/address_space/fork_impl.rs`

```rust
fn flush_tlb_after_fork(&self, parent_cr3: u64) {
    // Recharger le même CR3 invalide toutes les TLB entries non-global
    // du CPU courant — suffisant après le marquage CoW local.
    // Le fils démarre sur le même CPU ou recharge son propre CR3 au switch.
    if parent_cr3 != 0 {
        unsafe { crate::arch::x86_64::switch_cr3(parent_cr3); }
    }
}
```

---

## CORRECTION 4 : Masque RFLAGS correct dans `fork.rs`

```rust
// AVANT :
const RFLAGS_FORCE_CLR: u64 = 0x0000_0000_0004_0100; // faux

// APRÈS :
const RFLAGS_FORCE_CLR: u64 = (1u64 << 8)   // TF — Trap Flag
                             | (1u64 << 14)  // NT — Nested Task
                             | (1u64 << 16)  // RF — Resume Flag
                             | (1u64 << 17); // VM — Virtual 8086
// = 0x0003_4100
```

---

## CORRECTION 5 : Envoi explicite de SIGSEGV dans `do_page_fault()`

Fichier : `kernel/src/arch/x86_64/exceptions.rs`

```rust
FaultResult::Segfault { addr } => {
    let _ = addr;
    if frame.from_userspace() {
        // Envoyer SIGSEGV AVANT exception_return_to_user().
        // Sans ça, le processus reboucle infiniment sur le même fault.
        let tcb_raw = unsafe { super::smp::percpu::read_current_tcb() };
        if tcb_raw != 0 {
            crate::process::signal::delivery::send_signal_current(
                crate::process::signal::Signal::SIGSEGV,
                tcb_raw as *mut _,
            );
        }
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#PF kernel : accès invalide", frame);
    }
}
```

*(Adapter selon l'API `process::signal::delivery` existante.)*

---

## Ordre d'application recommandé

```
1. CORRECTION 2 (VMA clone)   → compile + cargo check
2. CORRECTION 1 (UserFaultAlloc) → compile + cargo check
3. CORRECTION 3 (TLB flush)   → compile
4. CORRECTION 4 (RFLAGS mask) → compile
5. CORRECTION 5 (SIGSEGV)     → compile
6. make test-exofs             → doit rester 2833 tests OK
7. cargo test --workspace      → doit rester clean
8. QEMU boot                   → observer init: spawned au-delà de ipc_router
```
