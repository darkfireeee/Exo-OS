# ExoOS — Patches Critiques — claude-gamma
## Fichier : claude-gamma-PATCHES.md

---

## PATCH-C1 — KernelFaultAllocator : utiliser le CR3 courant

**Fichier** : `kernel/src/arch/x86_64/memory_iface.rs`

### Ajout : helper `current_pml4_phys()`

Ajouter cette fonction dans `memory_iface.rs` (avant `KernelFaultAllocator`) :

```rust
/// Lit le CR3 du CPU courant (espace d'adressage actif).
/// Utilisé par KernelFaultAllocator pour opérer sur le bon espace utilisateur.
///
/// # Safety
/// CPL 0 uniquement. Le CR3 retourné est valide tant que le thread
/// courant n'est pas préempté et switché.
#[inline(always)]
fn current_pml4_phys() -> PhysAddr {
    let cr3: u64;
    // SAFETY: CPL 0, CR3 toujours lisible.
    unsafe {
        core::arch::asm!(
            "mov {v}, cr3",
            v = out(reg) cr3,
            options(nostack, nomem, preserves_flags),
        );
    }
    // Les 12 bits bas de CR3 sont des flags (PCID, etc.) — masquer.
    PhysAddr::new(cr3 & !0xFFFu64)
}
```

### Modification : `impl FaultAllocator for KernelFaultAllocator`

Remplacer les quatre méthodes qui utilisent `KERNEL_AS.pml4_phys()` :

```rust
impl FaultAllocator for KernelFaultAllocator {
    #[inline]
    fn alloc_zeroed(&self) -> Result<Frame, AllocError> {
        alloc_page(AllocFlags::ZEROED)
    }

    #[inline]
    fn alloc_nonzeroed(&self) -> Result<Frame, AllocError> {
        alloc_page(AllocFlags::NONE)
    }

    #[inline]
    fn free_frame(&self, f: Frame) {
        let _ = free_page(f);
    }

    fn map_page(&self, virt: VirtAddr, frame: Frame, flags: PageFlags) -> Result<(), AllocError> {
        // PATCH-C1 : opérer sur le CR3 courant (espace utilisateur actif),
        // pas sur KERNEL_AS dont la PML4 ne contient pas les adresses user.
        let pml4 = current_pml4_phys();
        let mut walker = crate::memory::virt::page_table::PageTableWalker::new(pml4);
        // SAFETY: virt doit être une adresse canonique dans l'espace courant.
        unsafe { walker.map_page(virt, frame, flags, self) }
    }

    fn remap_flags(&self, virt: VirtAddr, flags: PageFlags) -> Result<(), AllocError> {
        // PATCH-C1 : même raison — CR3 courant.
        let pml4 = current_pml4_phys();
        let mut walker = crate::memory::virt::page_table::PageTableWalker::new(pml4);
        walker.remap_flags(virt, flags)
    }

    #[inline]
    fn translate(&self, virt: VirtAddr) -> Option<PhysAddr> {
        // PATCH-C1 : traduire dans l'espace courant.
        let pml4 = current_pml4_phys();
        let walker = crate::memory::virt::page_table::PageTableWalker::new(pml4);
        walker.translate(virt)
    }

    fn read_pte_raw(&self, virt: VirtAddr) -> u64 {
        // PATCH-C1 : lire le PTE dans l'espace courant.
        let walker = crate::memory::virt::page_table::PageTableWalker::new(
            current_pml4_phys(),
        );
        walker.read_pte_raw(virt)
    }

    fn compare_exchange_pte_raw(&self, virt: VirtAddr, current: u64, new: u64) -> Result<(), u64> {
        // PATCH-C1 : CAS sur le PTE dans l'espace courant.
        let walker = crate::memory::virt::page_table::PageTableWalker::new(
            current_pml4_phys(),
        );
        // SAFETY: `virt` désigne une PTE feuille dans l'espace courant.
        unsafe { walker.compare_exchange_leaf_raw(virt, current, new) }
    }
}
```

### Note : compatibilité kernel faults

Les kernel page faults (`from_kernel = true`) sont traités en retournant `KernelFault` immédiatement, avant tout accès à l'allocateur. `KERNEL_AS` reste utilisé pour les mappings kernel explicites (map_kernel_page, etc.) — ces chemins ne passent PAS par `KernelFaultAllocator`.

---

## PATCH-C2 — VmaTree : clonage lors du fork

### Étape 1 : Implémenter Clone pour VmaNode et VmaTree

**Fichier** : `kernel/src/memory/virtual/vma/tree.rs`

Ajouter à la fin du fichier :

```rust
// ─────────────────────────────────────────────────────────────────────────────
// Clone de VmaTree — nécessaire pour fork() CoW
// ─────────────────────────────────────────────────────────────────────────────

impl VmaTree {
    /// Clone l'arbre de VMAs pour un processus fils (fork).
    ///
    /// Chaque VMA est copié avec le flag COW ajouté si WRITE est présent.
    /// Les VMAs DONTCOPY sont exclus (ex: SignalTcb — PROC-VMA/V-17).
    pub fn clone_for_fork(&self) -> Option<VmaTree> {
        let mut new_tree = VmaTree::new();
        for vma in self.iter() {
            // PROC-VMA/V-17 : ne pas copier les VMAs marquées DONTCOPY.
            if vma.flags.contains(VmaFlags::DONTCOPY) {
                continue;
            }
            let mut cloned_flags = vma.flags;
            // Marquer WRITE → COW (read-only jusqu'au premier write).
            if cloned_flags.contains(VmaFlags::WRITE) {
                cloned_flags |= VmaFlags::COW;
            }
            let cloned_vma = VmaDescriptor {
                start: vma.start,
                end: vma.end,
                flags: cloned_flags,
                backing: vma.backing.clone(),
                // Les compteurs de stats sont remis à zéro pour le fils.
                cow_breaks: core::sync::atomic::AtomicU64::new(0),
                page_faults: core::sync::atomic::AtomicU64::new(0),
            };
            // Insérer dans le nouvel arbre — si l'insertion échoue (OOM),
            // on abandonne et retourne None.
            if new_tree.insert(cloned_vma).is_err() {
                return None;
            }
        }
        Some(new_tree)
    }
}
```

### Étape 2 : Ajouter `clone_vma_tree_from` dans UserAddressSpace

**Fichier** : `kernel/src/memory/virtual/address_space/user.rs`

```rust
impl UserAddressSpace {
    // ... méthodes existantes ...

    /// Clone l'arbre de VMAs depuis l'espace parent (fork CoW).
    /// Retourne false si OOM.
    pub fn clone_vma_tree_from(&self, parent: &UserAddressSpace) -> bool {
        let parent_inner = parent.inner.lock();
        let mut child_inner = self.inner.lock();
        match parent_inner.vma_tree.clone_for_fork() {
            Some(new_tree) => {
                child_inner.vma_tree = new_tree;
                child_inner.mmap_hint = parent_inner.mmap_hint;
                child_inner.stack_bottom = parent_inner.stack_bottom;
                true
            }
            None => false,
        }
    }
}
```

### Étape 3 : Appeler `clone_vma_tree_from` dans `clone_cow()`

**Fichier** : `kernel/src/memory/virtual/address_space/fork_impl.rs`

```rust
// Dans impl AddressSpaceCloner for KernelAddressSpaceCloner, fn clone_cow() :
// APRÈS la création de child_as et AVANT le Ok(ClonedAddressSpace{...})

// PATCH-C2 : cloner le VmaTree du parent vers le fils.
// Sans ça, le fils a un VmaTree vide → tout #PF → SEGFAULT.
if src_space_ptr != 0 {
    let parent_as = unsafe { &*(src_space_ptr as *const UserAddressSpace) };
    if !child_as.clone_vma_tree_from(parent_as) {
        unsafe { free_userspace_tables(child_pml4_phys); }
        return Err(AddrSpaceCloneError::OutOfMemory);
    }
}
```

---

## PATCH-C3 — stdin/stdout/stderr : installation au boot

**Fichier** : `kernel/src/process/lifecycle/create.rs` + `kernel/src/userspace_boot.rs`

### Contexte

Pour l'instant, il n'y a pas de "device TTY" dans le VFS. La solution à court terme est d'utiliser un handle spécial (opaque) qui route vers le TTY server via le kernel. Le kernel doit maintenir un "handle TTY de boot" créé à l'init du `tty_server`.

### Étape 1 : Créer un TTY boot handle dans le kernel

**Fichier** : `kernel/src/arch/x86_64/terminal.rs` (ou nouveau fichier `kernel/src/tty_boot.rs`)

```rust
// kernel/src/tty_boot.rs
//
// Handle TTY de boot : FD opaque représentant le terminal console avant
// que /dev/tty soit monté dans le VFS.

use core::sync::atomic::{AtomicU64, Ordering};

/// Handle opaque du TTY de boot (0 = non initialisé).
/// Initialisé par tty_server via un syscall dédié, ou par le kernel
/// au premier accès au port E9/UART.
pub static BOOT_TTY_HANDLE: AtomicU64 = AtomicU64::new(1); // 1 = handle "console boot"

pub fn boot_tty_handle() -> u64 {
    BOOT_TTY_HANDLE.load(Ordering::Acquire)
}
```

### Étape 2 : Appeler `install_std_fds` dans `create_init_process_from_elf`

**Fichier** : `kernel/src/process/lifecycle/create.rs`

```rust
// Dans create_init_process_from_elf(), après la création du PCB
// et AVANT l'enqueue du thread dans la run queue :

let tty_h = crate::tty_boot::boot_tty_handle();
{
    let mut files = pcb.files.lock();
    // PATCH-C3 : installer stdin/stdout/stderr avec le handle TTY de boot.
    // Sans ça, exosh lit/écrit sur des FDs vides (EBADF).
    files.install_std_fds(tty_h, tty_h, tty_h);
}
```

### Étape 3 : Câbler SYS_READ/SYS_WRITE pour le handle TTY

**Fichier** : `kernel/src/syscall/fs_bridge.rs`

Dans le handler `SYS_READ` / `SYS_WRITE`, ajouter un check pour le handle TTY de boot :

```rust
// Dans le handler de SYS_READ :
if handle == crate::tty_boot::boot_tty_handle() {
    // Lire depuis le port E9 (debug console) ou via IPC tty_server.
    // Pour l'instant : lire depuis le buffer d'entrée kernel (port I/O 0x60 PS/2
    // ou via un ring buffer peuplé par input_server).
    return read_from_boot_console(buf_ptr, count);
}

// Dans le handler de SYS_WRITE :
if handle == crate::tty_boot::boot_tty_handle() {
    // Écrire sur le port E9 (debug) ET sur le framebuffer.
    return write_to_boot_console(buf_ptr, count);
}
```

---

## PATCH-C4 — Pont TTY server ↔ FDs 0/1/2

### Option A — Court terme : exosh lit/écrit via IPC (sans device file)

**Fichier** : `servers/exosh/src/main.rs`

Remplacer `read_byte_blocking()` et `write_all()` par des appels IPC au tty_server :

```rust
const TTY_SERVER_PID: u32 = 11; // PID fixe selon service_table
const TTY_MSG_READ_LINE: u32 = 0x131;
const TTY_MSG_WRITE: u32 = 0x132;

fn write_all(bytes: &[u8]) {
    if bytes.is_empty() { return; }
    // PATCH-C4-A : écriture via IPC vers tty_server au lieu de SYS_WRITE(fd=1)
    let mut req = IpcMsg {
        sender_pid: unsafe { syscall::syscall0(syscall::SYS_GETPID) } as u32,
        msg_type: TTY_MSG_WRITE,
        len: bytes.len() as u32,
        // copier bytes dans le payload IPC
        ..IpcMsg::zeroed()
    };
    let max = req.data.len().min(bytes.len());
    req.data[..max].copy_from_slice(&bytes[..max]);
    unsafe {
        syscall::syscall2(
            syscall::SYS_IPC_SEND,
            TTY_SERVER_PID as u64,
            &req as *const IpcMsg as u64,
        );
    }
}

fn read_byte_blocking() -> u8 {
    // PATCH-C4-A : lire via IPC depuis tty_server
    loop {
        let mut reply = IpcMsg::zeroed();
        let req = IpcMsg { msg_type: TTY_MSG_READ_LINE, ..IpcMsg::zeroed() };
        let rc = unsafe {
            syscall::syscall3(
                syscall::SYS_IPC_CALL,
                TTY_SERVER_PID as u64,
                &req as *const IpcMsg as u64,
                &mut reply as *mut IpcMsg as u64,
            )
        };
        if rc == 0 && reply.len > 0 {
            return reply.data[0];
        }
        sleep_ms(5);
    }
}
```

### Option B — Long terme : device file `/dev/tty`

Cette option nécessite :
1. Que `vfs_server` enregistre un handler pour `/dev/tty`
2. Que ce handler route les `read()`/`write()` vers le `tty_server` via IPC
3. Que le kernel ouvre `/dev/tty` et installe le FD résultant comme stdin/stdout/stderr

La mécanique exacte dépend de l'ABI IPC définie dans `exo_syscall_abi`. L'implémentation complète est prévue pour la Phase 4 (VFS complet).

---

## PATCH-M1 — Correction de `DEPS_SCHEDULER`

**Fichier** : `servers/init_server/src/service_table.rs`

```rust
// AVANT :
const DEPS_SCHEDULER: &[&str] = &["init_server"];

// APRÈS :
const DEPS_SCHEDULER: &[&str] = &["ipc_router", "memory_server"];
```

---

## Ordre de application des patches

```
1. PATCH-C1  (memory_iface.rs — mauvais AS dans FaultAllocator)
2. PATCH-C2  (fork_impl.rs + vma/tree.rs — VmaTree non cloné)
3. PATCH-M1  (service_table.rs — deps scheduler)
4. PATCH-C3  (create.rs + tty_boot.rs — std fds)
5. PATCH-C4-A (exosh — IPC TTY en attendant le device file)
```

Après ces patches, l'ordre de démarrage attendu :
```
kernel boot → PID1 init_server → ipc_router → memory_server → vfs_server
→ ... → tty_server → exosh (avec IPC vers tty_server)
→ shell interactif accessible
```
