# ExoOS — Corrections Claude-Beta : Patches Concrets
**Complément au rapport `claude-beta-audit-userspace.md`**  
Chaque section est un patch applicable directement au code existant.

---

## PATCH-01 : VmaTree — clone CoW pour fork (BUG-01)

### Nouveau fichier : `memory/virtual/vma/fork_clone.rs`

```rust
// kernel/src/memory/virt/vma/fork_clone.rs
//
// Clonage de VmaTree pour fork() : duplique les descripteurs en marquant
// les VMAs writables comme CoW (pour BUG-01 + BUG-02 simultanément).

use super::{VmaDescriptor, VmaFlags, VmaTree};
use alloc::vec::Vec;

/// Clone le VmaTree du parent pour un processus fils (fork CoW).
///
/// Toutes les VMAs avec WRITE reçoivent le flag COW en plus.
/// Le parent doit aussi recevoir COW (voir `mark_parent_vmas_cow`).
///
/// Retourne None si une allocation échoue.
pub fn clone_vma_tree_for_fork(src: &VmaTree) -> Option<VmaTree> {
    let mut dst = VmaTree::new();
    // collect() les VMAs du parent pour éviter de tenir le lock pendant insert
    let snapshots: Vec<VmaDescriptor> = src.snapshot_all();
    for mut vma in snapshots {
        // Toute VMA writable devient CoW dans le fils
        if vma.flags.contains(VmaFlags::WRITE) {
            vma.flags |= VmaFlags::COW;
        }
        // Allouer un descripteur slab pour le fils
        let vma_ptr = alloc_vma_descriptor(vma)?;
        unsafe {
            if !dst.insert(vma_ptr) {
                free_vma_descriptor(vma_ptr);
                // Libérer tout ce qui a déjà été inséré
                dst.drain_and_free();
                return None;
            }
        }
    }
    Some(dst)
}

/// Marque les VMAs writables du PARENT comme CoW après fork.
/// Doit être appelé sur l'AddressSpace parent avant le retour de do_fork().
pub fn mark_parent_vmas_cow(tree: &mut VmaTree) {
    tree.for_each_mut(|vma| {
        if vma.flags.contains(VmaFlags::WRITE) {
            vma.flags |= VmaFlags::COW;
        }
    });
}

// helpers dépendants du slab allocator VMA
fn alloc_vma_descriptor(desc: VmaDescriptor) -> Option<*mut VmaDescriptor> {
    use alloc::boxed::Box;
    let b = Box::try_new(desc).ok()?;
    Some(Box::into_raw(b))
}

fn free_vma_descriptor(ptr: *mut VmaDescriptor) {
    unsafe { drop(alloc::boxed::Box::from_raw(ptr)); }
}
```

**Note :** `VmaTree::snapshot_all()`, `for_each_mut()` et `drain_and_free()` sont de nouveaux helpers à ajouter à `VmaTree`. Leur implémentation dépend du type de tree (BST, interval tree, etc.) — la logique est triviale (itération + clone par valeur si `VmaDescriptor: Clone`).

---

## PATCH-02 : fork_impl.rs — Utiliser le clone de VMAs (BUG-01 + BUG-02)

### Diff conceptuel sur `clone_cow()`

```rust
// AVANT (actuel) :
let child_as = match try_box_new(UserAddressSpace::new(child_pml4_phys, 0)) {
    Some(addr_space) => addr_space,
    None => { ... return Err(OutOfMemory); }
};
if inherited_heap_end != 0 {
    child_as.heap_end.store(inherited_heap_end, ...);
}

// APRÈS (correction BUG-01) :
let (child_as, child_as_inner) = if src_space_ptr != 0 {
    let parent_as = unsafe { &*(src_space_ptr as *const UserAddressSpace) };
    
    // [BUG-02] Marquer le PARENT (VMAs write → CoW) AVANT de cloner
    {
        let mut inner = parent_as.inner.lock();
        mark_parent_vmas_cow(&mut inner.vma_tree);
    }
    
    // [BUG-01] Cloner l'espace du parent (avec VMAs, heap_end, hints)
    let child = parent_as.clone_for_fork(child_pml4_phys)
        .ok_or(AddrSpaceCloneError::OutOfMemory)?;
    child
} else {
    // Bootstrap : pas de parent (PID1 initial)
    try_box_new(UserAddressSpace::new(child_pml4_phys, 0))
        .ok_or(AddrSpaceCloneError::OutOfMemory)?
};
```

### Nouveau helper dans `user.rs`

```rust
impl UserAddressSpace {
    /// Crée un espace d'adressage fils pour fork().
    ///
    /// Clone le VmaTree en marquant les VMAs WRITE comme COW.
    /// Le pml4_phys du fils est déjà construit par fork_impl avant cet appel.
    pub fn clone_for_fork(&self, child_pml4_phys: PhysAddr) -> Option<Box<UserAddressSpace>> {
        use crate::memory::virt::vma::fork_clone::clone_vma_tree_for_fork;
        
        let inner = self.inner.lock();
        let child_tree = clone_vma_tree_for_fork(&inner.vma_tree)?;
        let heap_end = self.heap_end.load(Ordering::Acquire);
        
        let child_as = Box::try_new(UserAddressSpace {
            inner: Mutex::new(UserAsInner {
                vma_tree: child_tree,
                mmap_hint: inner.mmap_hint,
                stack_bottom: inner.stack_bottom,
            }),
            stats: UserAsStats::new(),
            pml4_phys: child_pml4_phys,
            pid: AtomicU64::new(0),       // mis à jour par update_child_addr_space_pid
            heap_end: AtomicU64::new(heap_end),
        }).ok()?;
        
        Some(child_as)
    }
}
```

---

## PATCH-03 : flush_tlb_after_fork — Flush CPU local (BUG-03)

### Diff dans `fork_impl.rs`

```rust
// AVANT :
fn flush_tlb_after_fork(&self, _parent_cr3: u64) {
    unsafe {
        shootdown_sync(TlbFlushType::All, crate::arch::x86_64::smp::init::smp_cpu_count());
    }
}

// APRÈS :
fn flush_tlb_after_fork(&self, parent_cr3: u64) {
    unsafe {
        // [BUG-03] Flush du CPU local en premier.
        // SYSRETQ ramène en Ring3 avec les PTEs désormais read-only (CoW).
        // Si le TLB local n'est pas flushé, le parent écrit via une entrée
        // TLB stale (writable) sans déclencher de #PF → corruption du frame partagé.
        //
        // write_cr3 invalide toutes les non-global TLB entries du CPU courant.
        // Si PCID actif : forcer le flush en clearant le bit 63 (NOFLUSH=0).
        let flush_cr3 = parent_cr3 & !crate::arch::x86_64::paging::CR3_NOFLUSH_BIT;
        crate::arch::x86_64::write_cr3(flush_cr3);
        
        // Puis flush les CPUs distants via IPI
        shootdown_sync(
            TlbFlushType::All,
            crate::arch::x86_64::smp::init::smp_cpu_count(),
        );
    }
}
```

**Attention :** si `CR3_NOFLUSH_BIT` n'est pas défini dans `paging.rs`, l'ajouter :
```rust
/// Bit 63 de CR3 : NOFLUSH (PCID active — ne pas invalider le TLB au chargement).
pub const CR3_NOFLUSH_BIT: u64 = 1 << 63;
```

---

## PATCH-04 : execve — Zériser callee-saved (BUG-04)

### Diff dans `syscall/dispatch.rs`, `handle_execve_inplace`

```rust
// AVANT :
Ok(()) => {
    let new_rip = thread.addresses.entry_point;
    let new_rsp = thread.addresses.initial_rsp;
    frame.rcx = new_rip;
    frame.rsp = new_rsp;
    frame.r11 = 0x0202;
    frame.rax = 0;
    unsafe {
        core::arch::asm!("mov qword ptr gs:[0x08], {rsp}", rsp = in(reg) new_rsp, ...);
    }
}

// APRÈS :
Ok(()) => {
    let new_rip = thread.addresses.entry_point;
    let new_rsp = thread.addresses.initial_rsp;
    
    // [BUG-04] Zériser les registres callee-saved pour éviter la fuite
    // de données de l'ancienne image vers le nouvel espace d'adressage.
    // SYSRETQ restaure rbx, rbp, r12-r15 depuis la frame.
    frame.rbx = 0;
    frame.rbp = 0;
    frame.r12 = 0;
    frame.r13 = 0;
    frame.r14 = 0;
    frame.r15 = 0;
    // rdi, rsi, rdx, r10, r8, r9 sont caller-saved : déjà écrasés par le syscall.
    
    frame.rcx = new_rip;
    frame.rsp = new_rsp;
    frame.r11 = 0x0202;  // RFLAGS : IF=1, bit réservé=1
    frame.rax = 0;
    
    unsafe {
        core::arch::asm!(
            "mov qword ptr gs:[0x08], {rsp}",
            rsp = in(reg) new_rsp,
            options(nostack, nomem)
        );
    }
}
```

---

## PATCH-05 : RFLAGS_FORCE_CLR (BUG-05)

### Diff dans `process/lifecycle/fork.rs`

```rust
// AVANT :
const RFLAGS_FORCE_CLR: u64 = 0x0000_0000_0004_0100; // TF=0, NT=0, RF=0, VM=0

// APRÈS :
/// Flags à forcer à 0 dans RFLAGS du fils.
/// TF(8), NT(14), RF(16), VM(17)
const RFLAGS_FORCE_CLR: u64 = (1 << 8)   // TF — Trap Flag
                             | (1 << 14)  // NT — Nested Task
                             | (1 << 16)  // RF — Resume Flag
                             | (1 << 17); // VM — Virtual-8086 mode
// = 0x0003_4100
```

---

## Ordre d'application recommandé

```
1. PATCH-01 : créer memory/virtual/vma/fork_clone.rs
2. PATCH-02 : modifier fork_impl.rs (clone_cow + UserAddressSpace::clone_for_fork)
              → dépend de PATCH-01
3. PATCH-03 : corriger flush_tlb_after_fork
              → peut s'appliquer indépendamment
4. PATCH-04 : zériser callee-saved dans execve
              → peut s'appliquer indépendamment
5. PATCH-05 : corriger RFLAGS_FORCE_CLR
              → trivial, indépendant
```

## Séquence de test recommandée après patches

```bash
# 1. Unit tests (host) — doivent tous passer
cargo test --workspace --message-format short

# 2. Test ExoFS
make test-exofs

# 3. Boot QEMU avec traces fork/exec
qemu-system-x86_64 ... -debugcon file:/tmp/e9.log ...
grep -E "fork:|fork_child|spawn|ipc_router|exosh" /tmp/e9.log

# Attendu après correction :
# fork: enter
# fork: clone-as begin
# fork: clone-as done
# fork: files cloned
# fork: enqueue
# → child runs → pas de SIGSEGV → ipc_router démarre
# init: spawned ipc_router
# init: start vfs_server
# ...
# exosh: ready
```

## Points de vigilance post-correction

1. **VmaTree::snapshot_all()** — si l'implémentation utilise un BST intrusive avec nœuds `unsafe`, le snapshot doit se faire sous verrou et copier les descripteurs **par valeur**, pas par pointeur.

2. **Double-lock** — `clone_for_fork` prend le lock interne de `UserAddressSpace`. Ne jamais l'appeler depuis un contexte qui tient déjà ce lock.

3. **exec après fork** — après `do_execve()`, les VMAs COW du fork sont remplacées par les nouvelles VMAs ELF. Le flag `process_flags::EXEC_DONE` et la libération de l'ancien AS via `KERNEL_AS_CLONER.free_addr_space(old_as_ptr)` nettoient tout. Pas d'interaction avec les patches ci-dessus.

4. **mmap et sbrk** — les nouvelles VMAs créées après fork (dans l'enfant) doivent aussi passer par le chemin normal `insert_vma`. Les patches ne cassent pas ce chemin.
