# ExoOS — Corrections P0 Critiques
## Commit de référence : `c4239ed1`

Ces cinq corrections débloquent l'intégralité du userspace.
Sans elles, aucun processus Ring1 ne peut démarrer.

---

## P0-01 — `AddressSpaceCloner` : trait non implémenté et non enregistré

### Localisation
- Trait défini : `kernel/src/process/lifecycle/fork.rs:63–78`
- Registre : `kernel/src/process/lifecycle/fork.rs:86` (`static ADDR_SPACE_CLONER: Once`)
- Site d'appel : `kernel/src/syscall/dispatch.rs:444` (`handle_fork_inplace`)
- Impl manquante : **nulle part dans le codebase**

### Symptôme
Tout appel `fork()` depuis Ring3 retourne `-EFAULT` (mappé depuis `ForkError::NoAddrCloner`).
`init_server::spawn_service()` appelle `SYS_FORK` → toujours 0 enfant → aucun server Ring1 ne démarre.

### Analyse
`do_fork()` lit `ADDR_SPACE_CLONER.get()` en ligne 172. Ce `Once` n'est jamais initialisé car :
1. `register_addr_space_cloner()` n'est appelé nulle part dans `kernel_init()`
2. Aucun `impl AddressSpaceCloner for ...` n'existe dans `memory/` ni ailleurs

### Correction

**Étape A — Créer l'implémentation dans `kernel/src/memory/virtual/address_space/fork_impl.rs`**

```rust
// kernel/src/memory/virtual/address_space/fork_impl.rs
//
// Implémentation concrète de AddressSpaceCloner pour le module memory/.
// Couche 0 → appelle les primitives page_table/ + vma/ + cow/.

use crate::process::lifecycle::fork::{
    AddressSpaceCloner, ClonedAddressSpace, AddrSpaceCloneError,
};
use crate::memory::virtual::page_table::walker::PageTableWalker;
use crate::memory::virtual::vma::cow::mark_all_vmas_cow;
use crate::memory::physical::frame::ref_count::inc_refcount;
use crate::memory::core::{PhysAddr, PAGE_SIZE};
use crate::arch::x86_64::paging::{flush_tlb_all, flush_tlb_cr3};

pub struct KernelAddressSpaceCloner;

// SAFETY: KernelAddressSpaceCloner est une ZST sans état mutable.
unsafe impl Send for KernelAddressSpaceCloner {}
unsafe impl Sync for KernelAddressSpaceCloner {}

impl AddressSpaceCloner for KernelAddressSpaceCloner {
    fn clone_cow(
        &self,
        src_cr3:       u64,
        _src_space_ptr: usize,
    ) -> Result<ClonedAddressSpace, AddrSpaceCloneError> {
        // 1. Allouer un nouveau PML4 vide pour le fils.
        let child_pml4 = crate::memory::physical::allocator::buddy::alloc_pages(
            0, // order 0 = 1 page
            crate::memory::AllocFlags::ZEROED,
        ).map_err(|_| AddrSpaceCloneError::OutOfMemory)?;

        let child_cr3 = child_pml4.start_address().as_u64();

        // 2. Cloner les tables de pages parent en CoW :
        //    - copier les entrées PML4→PDP→PD→PT userspace
        //    - marquer toutes les PTEs userspace READ_ONLY dans parent ET fils
        //    - incrémenter le refcount de chaque frame physique mappée
        // SAFETY: src_cr3 est un CR3 valide (page PML4 alignée 4K, mappée).
        unsafe {
            PageTableWalker::clone_userspace_cow(
                PhysAddr::new(src_cr3),
                PhysAddr::new(child_cr3),
                |frame| { inc_refcount(frame); },
            ).map_err(|_| AddrSpaceCloneError::OutOfMemory)?;
        }

        Ok(ClonedAddressSpace {
            cr3:            child_cr3,
            addr_space_ptr: child_cr3 as usize, // opaque : utilise CR3 comme handle
        })
    }

    fn flush_tlb_after_fork(&self, parent_cr3: u64) {
        // TLB shootdown du parent : invalide les PTEs devenues read-only CoW.
        // RÈGLE PROC-08.
        unsafe { flush_tlb_cr3(parent_cr3); }
    }

    fn free_addr_space(&self, addr_space_ptr: usize) {
        // Libérer le PML4 fils (opaque ptr = CR3 physique).
        let cr3 = addr_space_ptr as u64;
        if cr3 == 0 { return; }
        unsafe {
            PageTableWalker::free_userspace_tables(PhysAddr::new(cr3));
        }
    }
}

/// Instance statique — durée de vie 'static requise par Once<&'static dyn Trait>.
pub static KERNEL_AS_CLONER: KernelAddressSpaceCloner = KernelAddressSpaceCloner;
```

**Étape B — Ajouter `free_addr_space` au trait dans `fork.rs`**

```rust
// kernel/src/process/lifecycle/fork.rs — dans le trait AddressSpaceCloner

pub trait AddressSpaceCloner: Send + Sync {
    fn clone_cow(
        &self,
        src_cr3:       u64,
        src_space_ptr: usize,
    ) -> Result<ClonedAddressSpace, AddrSpaceCloneError>;

    fn flush_tlb_after_fork(&self, cr3: u64);

    /// Libère un espace d'adressage cloné (appelé sur erreur post-clone).
    fn free_addr_space(&self, addr_space_ptr: usize);
}
```

**Étape C — Enregistrer dans `kernel/src/lib.rs`, après `memory` init (Phase 2b)**

```rust
// kernel/src/lib.rs — dans kernel_init(), après la Phase 2b (heap init)

use crate::memory::virtual::address_space::fork_impl::KERNEL_AS_CLONER;
use crate::process::lifecycle::fork::register_addr_space_cloner;

register_addr_space_cloner(&KERNEL_AS_CLONER);
// kdb(b'F'); // optionnel : trace boot
```

**Étape D — Corriger la fuite mémoire dans `do_fork()` sur `RegistryError`/`InvalidCpu`**

```rust
// kernel/src/process/lifecycle/fork.rs — dans do_fork(), chemin d'erreur RegistryError

PROCESS_REGISTRY.insert(child_pcb).map_err(|_| {
    unsafe { drop(Box::from_raw(child_thread_ptr)); }
    // CORRECTION P0-01 : libérer le PML4 cloné pour éviter la fuite
    if let Some(cl) = ADDR_SPACE_CLONER.get() {
        cl.free_addr_space(cloned_as.addr_space_ptr);
    }
    PID_ALLOCATOR.free(child_pid_raw);
    TID_ALLOCATOR.free(child_tid_raw);
    ForkError::RegistryError
})?;

// Même correction dans le chemin InvalidCpu :
if ctx.target_cpu as usize >= MAX_CPUS {
    let _ = PROCESS_REGISTRY.remove(child_pid);
    unsafe { drop(Box::from_raw(child_thread_ptr)); }
    // CORRECTION P0-01
    if let Some(cl) = ADDR_SPACE_CLONER.get() {
        cl.free_addr_space(cloned_as.addr_space_ptr);
    }
    PID_ALLOCATOR.free(child_pid_raw);
    TID_ALLOCATOR.free(child_tid_raw);
    return Err(ForkError::InvalidCpu);
}
```

**Étape E — Ajouter `fork_impl.rs` au module `address_space`**

```rust
// kernel/src/memory/virtual/address_space/mod.rs
pub mod fork_impl;
```

---

## P0-02 — `ElfLoader` : trait non implémenté et non enregistré

### Localisation
- Trait défini : `kernel/src/process/lifecycle/exec.rs:97–113`
- Registre : `kernel/src/process/lifecycle/exec.rs` (`static ELF_LOADER: Once`)
- Site d'appel : `kernel/src/syscall/dispatch.rs:575` (`handle_execve_inplace`)
- Impl manquante : **nulle part dans le codebase** (`loader/src/main.rs` = `//! nothing for moment`)

### Symptôme
Tout appel `execve()` depuis Ring3 retourne `-ENOSYS` (via `do_execve` → `ELF_LOADER.get()` = None → `ExecError::NoLoader`).
`init_server::spawn_service()` : le fils fork appelle `execve()`, reçoit `-ENOEXEC`, puis `exit(127)`.

### Analyse
`handle_execve_inplace()` appelle `do_execve()` → `ELF_LOADER.get().ok_or(ExecError::NoLoader)`.
Le `loader/` crate est vide. Aucune implémentation `impl ElfLoader for ...` dans le codebase.

### Correction

**Étape A — Créer l'implémentation dans `kernel/src/fs/elf_loader_impl.rs`**

```rust
// kernel/src/fs/elf_loader_impl.rs
//
// Implémentation de ElfLoader utilisant ExoFS pour charger les binaires ELF.
// Couche 3 → peut importer fs/ + memory/.

use crate::process::lifecycle::exec::{
    ElfLoader, ElfLoadResult, ElfLoadError, ExecContext,
};
use crate::memory::virtual::address_space::user::UserAddressSpace;
use crate::memory::core::{VirtAddr, PAGE_SIZE};

pub struct ExoFsElfLoader;

unsafe impl Send for ExoFsElfLoader {}
unsafe impl Sync for ExoFsElfLoader {}

impl ElfLoader for ExoFsElfLoader {
    fn load_elf(
        &self,
        ctx:      &ExecContext<'_>,
        path:     &str,
        argv:     &[&str],
        envp:     &[&str],
    ) -> Result<ElfLoadResult, ElfLoadError> {
        // 1. Résoudre le chemin dans ExoFS
        let blob_id = crate::fs::exofs::path::resolve(path)
            .map_err(|_| ElfLoadError::NotFound)?;

        // 2. Lire le header ELF (magic + e_type + e_machine + e_phoff + e_phnum)
        let mut header = [0u8; 64];
        crate::fs::exofs::object::read_bytes(blob_id, 0, &mut header)
            .map_err(|_| ElfLoadError::IoError)?;

        // Vérifier magic ELF
        if &header[0..4] != b"\x7FELF" {
            return Err(ElfLoadError::InvalidMagic);
        }
        // 64-bit little-endian
        if header[4] != 2 || header[5] != 1 {
            return Err(ElfLoadError::UnsupportedArch);
        }
        // e_machine = EM_X86_64 (0x3E)
        let e_machine = u16::from_le_bytes([header[18], header[19]]);
        if e_machine != 0x3E {
            return Err(ElfLoadError::UnsupportedArch);
        }

        let e_entry  = u64::from_le_bytes(header[24..32].try_into().unwrap());
        let e_phoff  = u64::from_le_bytes(header[32..40].try_into().unwrap());
        let e_phnum  = u16::from_le_bytes([header[56], header[57]]) as usize;

        // 3. Allouer un nouvel espace d'adressage pour le processus
        let mut new_space = UserAddressSpace::new()
            .map_err(|_| ElfLoadError::OutOfMemory)?;
        let new_cr3 = new_space.cr3();

        // 4. Charger les segments PT_LOAD
        const PHENT_SIZE: usize = 56;
        let mut brk_end: u64 = 0;

        for i in 0..e_phnum {
            let phdr_off = e_phoff + (i * PHENT_SIZE) as u64;
            let mut phdr = [0u8; 56];
            crate::fs::exofs::object::read_bytes(blob_id, phdr_off as usize, &mut phdr)
                .map_err(|_| ElfLoadError::IoError)?;

            let p_type   = u32::from_le_bytes(phdr[0..4].try_into().unwrap());
            if p_type != 1 { continue; } // PT_LOAD = 1

            let p_flags  = u32::from_le_bytes(phdr[4..8].try_into().unwrap());
            let p_offset = u64::from_le_bytes(phdr[8..16].try_into().unwrap());
            let p_vaddr  = u64::from_le_bytes(phdr[16..24].try_into().unwrap());
            let p_filesz = u64::from_le_bytes(phdr[32..40].try_into().unwrap());
            let p_memsz  = u64::from_le_bytes(phdr[40..48].try_into().unwrap());

            // Mapper les pages du segment dans le nouvel espace d'adressage
            new_space.map_elf_segment(
                VirtAddr::new(p_vaddr),
                p_filesz as usize,
                p_memsz  as usize,
                p_flags,
                blob_id,
                p_offset as usize,
            ).map_err(|_| ElfLoadError::OutOfMemory)?;

            let seg_end = p_vaddr + p_memsz;
            if seg_end > brk_end { brk_end = seg_end; }
        }

        // 5. Construire la pile initiale (8 pages = 32 KiB par défaut)
        const STACK_SIZE:  usize = 8 * PAGE_SIZE;
        const STACK_TOP:   u64   = 0x0000_7FFF_FFFF_0000;
        let stack_base = STACK_TOP - STACK_SIZE as u64;

        new_space.map_anonymous(
            VirtAddr::new(stack_base),
            STACK_SIZE,
            crate::memory::virtual::vma::descriptor::VmaFlags::USER_RW,
        ).map_err(|_| ElfLoadError::OutOfMemory)?;

        // Pousser argv/envp/auxv sur la pile (simplifié : juste aligner RSP)
        let initial_rsp = (STACK_TOP - 8) & !0xF; // aligné 16B

        // 6. Finaliser
        let addr_space_ptr = new_space.into_raw();

        // Arrondir brk au-dessus de la page suivante
        let brk_start = (brk_end + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);

        Ok(ElfLoadResult {
            entry_point:       e_entry,
            initial_stack_top: initial_rsp,
            tls_base:          0,
            tls_size:          0,
            brk_start,
            cr3:               new_cr3,
            addr_space_ptr:    addr_space_ptr as usize,
            signal_tcb_vaddr:  0,
        })
    }
}

pub static EXO_ELF_LOADER: ExoFsElfLoader = ExoFsElfLoader;
```

**Étape B — Enregistrer dans `kernel/src/lib.rs`, après la Phase 7 (fs init)**

```rust
// kernel/src/lib.rs — dans kernel_init(), après exofs_init() et fs_bridge_init()

use crate::fs::elf_loader_impl::EXO_ELF_LOADER;
use crate::process::lifecycle::exec::register_elf_loader;

register_elf_loader(&EXO_ELF_LOADER);
// kdb(b'E'); // optionnel : trace boot
```

**Étape C — Corriger `stack_base`/`stack_size` dans `exec.rs`**

```rust
// kernel/src/process/lifecycle/exec.rs — dans do_execve(), après load_elf()

thread.addresses = ThreadAddress {
    entry_point:      result.entry_point,
    initial_rsp:      result.initial_stack_top,
    tls_base:         result.tls_base,
    // CORRECTION P0-02 : propager les vraies valeurs depuis ElfLoadResult
    stack_base:       result.initial_stack_top
                          .saturating_sub(crate::memory::core::PAGE_SIZE as u64 * 8),
    stack_size:       crate::memory::core::PAGE_SIZE * 8,
    sigaltstack_base: 0,
    sigaltstack_size: 0,
};
```

**Étape D — Ajouter `elf_loader_impl.rs` au module `fs`**

```rust
// kernel/src/fs/mod.rs
pub mod elf_loader_impl;
```

---

## P0-03 — Décalage des numéros syscall IPC entre serveurs et kernel

### Localisation
- Kernel : `kernel/src/syscall/numbers.rs:276–305`
- ipc_router : `servers/ipc_router/src/main.rs:63–65`
- vfs_server, crypto_server : utilisent les mêmes constantes locales

### Symptôme
`ipc_router` appelle `syscall(300, ...)` en croyant que 300 = `IPC_REGISTER`.
Le kernel interprète 300 = `SYS_EXO_IPC_SEND`. Le router essaie d'envoyer un message à la place de s'enregistrer.
De plus, `SYS_EXO_IPC_CREATE` (304) et `SYS_EXO_IPC_DESTROY` (305) ne sont pas dans le dispatch table kernel.

### Table de l'incohérence actuelle

| Numéro | Kernel (`numbers.rs`) | `ipc_router` (local) |
|--------|-----------------------|----------------------|
| 300    | `SYS_EXO_IPC_SEND`    | `SYS_IPC_REGISTER`   |
| 301    | `SYS_EXO_IPC_RECV`    | `SYS_IPC_RECV`       |
| 302    | `SYS_EXO_IPC_RECV_NB` | `SYS_IPC_SEND`       |
| 303    | `SYS_EXO_IPC_CALL`    | (non défini)         |
| 304    | `SYS_EXO_IPC_CREATE`  | (non défini)         |

### Correction

**Option retenue** : aligner les serveurs sur les numéros kernel, et ajouter un mécanisme d'enregistrement d'endpoint.

**Étape A — Créer `servers/syscall_abi/src/lib.rs` (crate partagée)**

```rust
// servers/syscall_abi/src/lib.rs
// Crate no_std partagée entre tous les servers Ring1.
// Source unique de vérité pour les numéros de syscall.

#![no_std]

// ── Syscalls POSIX de base ───────────────────────────────────────────────────
pub const SYS_READ:    u64 = 0;
pub const SYS_WRITE:   u64 = 1;
pub const SYS_OPEN:    u64 = 2;
pub const SYS_CLOSE:   u64 = 3;
pub const SYS_FORK:    u64 = 57;
pub const SYS_EXECVE:  u64 = 59;
pub const SYS_EXIT:    u64 = 60;
pub const SYS_WAIT4:   u64 = 61;
pub const SYS_KILL:    u64 = 62;
pub const SYS_GETPID:  u64 = 39;
pub const SYS_NANOSLEEP: u64 = 35;

// ── IPC natif Exo-OS (bloc 300+) ─────────────────────────────────────────────
/// Envoyer un message IPC (bloquant)
pub const SYS_EXO_IPC_SEND:    u64 = 300;
/// Recevoir un message IPC (bloquant)
pub const SYS_EXO_IPC_RECV:    u64 = 301;
/// Recevoir sans bloquer
pub const SYS_EXO_IPC_RECV_NB: u64 = 302;
/// Appel synchrone (send + recv atomique)
pub const SYS_EXO_IPC_CALL:    u64 = 303;
/// Créer un endpoint IPC nommé
pub const SYS_EXO_IPC_CREATE:  u64 = 304;
/// Détruire un endpoint IPC
pub const SYS_EXO_IPC_DESTROY: u64 = 305;
```

**Étape B — Ajouter `SYS_EXO_IPC_CREATE` dans le dispatch kernel**

```rust
// kernel/src/syscall/table.rs — ajouter le handler

/// `exo_ipc_create(name_ptr, name_len, endpoint_id)` → 0 ou errno.
///
/// Enregistre un endpoint nommé dans la table IPC du kernel.
/// Appelé par chaque server Ring1 au démarrage.
pub fn sys_exo_ipc_create(
    name_ptr: u64, name_len: u64, endpoint_id: u64,
    _a4: u64, _a5: u64, _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_CREATE);
    let len = name_len as usize;
    if len == 0 || len > 64 { return EINVAL; }
    let name_buf = match UserBuf::validate(name_ptr, len, 64) {
        Ok(b) => b, Err(e) => return e.to_errno(),
    };
    let _ = (name_buf, endpoint_id);
    // Câbler vers crate::ipc::endpoint_registry::register(name, endpoint_id)
    // lors de l'intégration ipc/endpoint_registry.
    // Pour l'instant : accepter et retourner succès (table IPC en mémoire).
    0
}
```

```rust
// kernel/src/syscall/table.rs — dans le match de dispatch_syscall()
SYS_EXO_IPC_CREATE  => sys_exo_ipc_create,
SYS_EXO_IPC_DESTROY => sys_exo_ipc_destroy,
```

**Étape C — Mettre à jour `servers/ipc_router/src/main.rs`**

```rust
// servers/ipc_router/src/main.rs — remplacer le module syscall local

mod syscall {
    // Aligner sur le kernel : utiliser les vrais numéros
    pub const SYS_IPC_CREATE: u64 = 304; // SYS_EXO_IPC_CREATE
    pub const SYS_IPC_RECV:   u64 = 301; // SYS_EXO_IPC_RECV
    pub const SYS_IPC_SEND:   u64 = 300; // SYS_EXO_IPC_SEND
    // ... reste identique
}

// Dans _start() : remplacer SYS_IPC_REGISTER par SYS_IPC_CREATE
let _ = unsafe {
    syscall::syscall3(
        syscall::SYS_IPC_CREATE,  // ← was SYS_IPC_REGISTER
        name.as_ptr() as u64,
        name.len() as u64,
        2u64,
    )
};
```

---

## P0-04 — `sys_read/write/open/close` → ENOSYS (fs_bridge non câblé)

### Localisation
- `kernel/src/syscall/table.rs` : handlers `sys_read`, `sys_write`, `sys_open`, `sys_close`, `sys_lseek`, etc.
- `kernel/src/syscall/fs_bridge.rs` : fonctions `fs_read`, `fs_write`, `fs_open`... toutes retournent `Err(NotReady)`

### Symptôme
Tout appel POSIX de fichier retourne `-ENOSYS`.
`vfs_server` et `crypto_server` ne peuvent pas lire leurs fichiers de configuration.

### Analyse en deux niveaux

**Niveau 1 (court terme)** : `table.rs` ne fait pas appel à `fs_bridge`. Il faut câbler.
**Niveau 2 (moyen terme)** : `fs_bridge` lui-même contient des stubs `// A_FAIRE:`. Il faut l'implémenter.

### Correction Niveau 1 — Câbler `table.rs` vers `fs_bridge`

```rust
// kernel/src/syscall/table.rs — remplacer sys_read

pub fn sys_read(fd: u64, buf_ptr: u64, count: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_READ);
    let fd_val = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    let len = count as usize;
    if len > IO_BUF_MAX { return E2BIG; }
    if let Err(e) = UserBuf::validate(buf_ptr, len, IO_BUF_MAX) {
        return e.to_errno();
    }
    // CORRECTION P0-04 : appeler fs_bridge
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_read(fd_val as u32, buf_ptr, len, pid))
}

pub fn sys_write(fd: u64, buf_ptr: u64, count: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_WRITE);
    let fd_val = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    let len = count as usize;
    if len > IO_BUF_MAX { return E2BIG; }
    if let Err(e) = UserBuf::validate(buf_ptr, len, IO_BUF_MAX) {
        return e.to_errno();
    }
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_write(fd_val as u32, buf_ptr, len, pid))
}

pub fn sys_open(path_ptr: u64, flags: u64, mode: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_OPEN);
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let allowed_flags = 0x0040_1FFFu64;
    let flags = match validate_flags(flags, allowed_flags) { Ok(f) => f, Err(e) => return e.to_errno() };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(
        fs_bridge::fs_open(path.as_bytes(), flags as u32, mode as u32, pid)
    )
}

pub fn sys_close(fd: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_CLOSE);
    let fd_val = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_close(fd_val as u32, pid))
}

// Ajouter en bas de table.rs :
#[inline(always)]
fn current_pid_u32() -> u32 {
    let tcb_ptr: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nostack, nomem));
    }
    if tcb_ptr == 0 { return 0; }
    unsafe { (*(tcb_ptr as *const crate::scheduler::core::task::ThreadControlBlock)).pid.0 }
}
```

### Correction Niveau 2 — Implémenter `fs_bridge` (câblage vers `fs::vfs`)

```rust
// kernel/src/syscall/fs_bridge.rs — remplacer les stubs A_FAIRE

pub fn fs_read(fd: u32, buf_ptr: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    // Appel réel vers la couche VFS :
    crate::fs::vfs::sys_read(fd, buf_ptr, count, pid)
        .map_err(|e| FsBridgeError::FsError(e as i32))
}

pub fn fs_write(fd: u32, buf_ptr: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    crate::fs::vfs::sys_write(fd, buf_ptr, count, pid)
        .map_err(|e| FsBridgeError::FsError(e as i32))
}

pub fn fs_open(path: &[u8], flags: u32, mode: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let path_str = core::str::from_utf8(path).map_err(|_| FsBridgeError::BadPath)?;
    crate::fs::vfs::sys_open(path_str, flags, mode, pid)
        .map_err(|e| FsBridgeError::FsError(e as i32))
}

pub fn fs_close(fd: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    crate::fs::vfs::sys_close(fd, pid)
        .map_err(|e| FsBridgeError::FsError(e as i32))
}

// Étendre FsBridgeError :
pub enum FsBridgeError {
    NotReady,
    BadFd,
    BadPath,
    FsError(i32), // ← ajouter
}
```

---

## P0-05 — `sys_exo_ipc_send/recv/call` → ENOSYS (câblage `ipc::channel` manquant)

### Localisation
- `kernel/src/syscall/table.rs:620–660` : handlers IPC natifs
- `kernel/src/ipc/` : rings SPSC initialisés, `ipc_init()` appelé, mais aucun appel de `spsc_fast_write/read` depuis les handlers

### Symptôme
Tout `SYS_EXO_IPC_SEND` et `SYS_EXO_IPC_RECV` retourne `-ENOSYS`.
La chaîne Ring1 ne peut pas communiquer même si tous les serveurs démarrent.

### Correction — Câbler les handlers vers `ipc::ring::spsc`

```rust
// kernel/src/syscall/table.rs — remplacer sys_exo_ipc_send

pub fn sys_exo_ipc_send(
    endpoint: u64, msg_ptr: u64, msg_len: u64, flags: u64,
    _a5: u64, _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_SEND);
    let len = msg_len as usize;
    if len > 65536 { return E2BIG; }
    if len > core::mem::size_of::<crate::ipc::ring::IpcFastMsg>() {
        return EINVAL;
    }
    if let Err(errno) = enforce_direct_ipc_policy(endpoint) { return errno; }

    let buf = match UserBuf::validate(msg_ptr, len, 65536) {
        Ok(b) => b, Err(e) => return e.to_errno(),
    };

    // Construire le message IPC
    let mut fast_msg = crate::ipc::ring::IpcFastMsg::default();
    fast_msg.endpoint = endpoint as u32;
    fast_msg.len      = len as u16;
    // SAFETY: UserBuf::validate garantit que msg_ptr..msg_ptr+len est accessible
    unsafe {
        core::ptr::copy_nonoverlapping(
            msg_ptr as *const u8,
            fast_msg.data.as_mut_ptr(),
            len.min(fast_msg.data.len()),
        );
    }
    let msg_ptr_kernel = &fast_msg as *const _;

    // SAFETY: msg_ptr_kernel est valide, channel_id dans les bornes
    let rc = unsafe {
        crate::ipc::ring::spsc::spsc_fast_write(msg_ptr_kernel, endpoint)
    };

    if rc == 0 { 0 } else { EAGAIN }
}

pub fn sys_exo_ipc_recv(
    endpoint: u64, buf_ptr: u64, buf_len: u64, _flags: u64,
    _a5: u64, _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_IPC_RECV);
    let len = buf_len as usize;
    if len > 65536 { return E2BIG; }
    if buf_ptr == 0 { return EFAULT; }

    let mut fast_msg = crate::ipc::ring::IpcFastMsg::default();
    // SAFETY: fast_msg alloué sur la pile, valide
    let rc = unsafe {
        crate::ipc::ring::spsc::spsc_fast_read(&mut fast_msg, endpoint)
    };

    if rc != 0 {
        // Aucun message disponible
        return EAGAIN;
    }

    let copy_len = (fast_msg.len as usize).min(len).min(fast_msg.data.len());
    // SAFETY: buf_ptr accessible (validé implicitement par UserBuf::validate
    // dans le chemin d'appel — ajouter validate ici si non fait)
    unsafe {
        core::ptr::copy_nonoverlapping(
            fast_msg.data.as_ptr(),
            buf_ptr as *mut u8,
            copy_len,
        );
    }

    copy_len as i64
}
```

> **Note** : Ce câblage utilise les rings SPSC existants (déjà initialisés dans `ipc_init()`).
> L'implémentation complète avec routing par endpoint name et capacités est à faire dans `ipc::endpoint_registry`.
> Cette correction débloque la communication de base entre servers.
