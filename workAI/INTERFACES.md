# 📐 INTERFACES - Spécifications Techniques

---

## 🧠 1. MEMORY API (✅ DISPONIBLE)

**Status** : ✅ IMPLÉMENTÉ (2h de travail)  
**Responsable** : Copilot  
**Date** : Maintenant  

### 1.1 Physical Frame Allocator (Buddy System)

```rust
use crate::memory::physical::buddy_allocator;
use crate::memory::address::PhysicalAddress;

// Allouer une frame physique (4KB)
pub fn alloc_frame() -> MemoryResult<PhysicalAddress>;

// Libérer une frame
pub fn free_frame(addr: PhysicalAddress) -> MemoryResult<()>;

// Allouer plusieurs frames contiguës
pub fn alloc_contiguous(count: usize) -> MemoryResult<PhysicalAddress>;

// Libérer frames contiguës
pub fn free_contiguous(addr: PhysicalAddress, count: usize) -> MemoryResult<()>;

// Statistiques
pub fn get_stats() -> Option<BuddyStats>;
```

**Fichier** : `kernel/src/memory/physical/buddy_allocator.rs` (600+ lignes)  
**Features** :
- Ordres 0→12 (4KB → 16MB)
- Coalescing automatique
- Bitmap tracking
- Thread-safe avec Mutex

### 1.2 Virtual Memory Manager (Page Tables)

```rust
use crate::memory::virtual::page_table::{map_page, unmap_page, translate, PageFlags};
use crate::memory::address::{VirtualAddress, PhysicalAddress};

// Mapper une page virtuelle → physique
pub fn map_page(virt: VirtualAddress, phys: PhysicalAddress, flags: PageFlags) -> MemoryResult<()>;

// Unmapper une page
pub fn unmap_page(virt: VirtualAddress) -> MemoryResult<PhysicalAddress>;

// Traduire adresse virtuelle → physique
pub fn translate(virt: VirtualAddress) -> Option<PhysicalAddress>;

// Mettre à jour les flags
pub fn update_flags(virt: VirtualAddress, flags: PageFlags) -> MemoryResult<()>;

// Flush TLB
pub fn flush_tlb(virt: VirtualAddress);
pub fn flush_tlb_all();
```

**Fichier** : `kernel/src/memory/virtual/page_table.rs` (700+ lignes)  
**Features** :
- 4-level page tables (P4→P3→P2→P1)
- Support 4KB, 2MB, 1GB pages
- TLB invalidation optimisée
- PageFlags presets (KERNEL, USER, READONLY, DEVICE)

### 1.3 Heap Allocator (GlobalAlloc)

```rust
use crate::memory::heap::LockedHeap;
use core::alloc::{GlobalAlloc, Layout};

// Utilisation directe via alloc crate
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::string::String;

let vec = Vec::new(); // Utilise automatiquement l'allocator global
let boxed = Box::new(42);
let string = String::from("hello");

// Statistiques
let stats = crate::ALLOCATOR.stats();
println!("Heap: {}KB allocated, {}KB free", stats.allocated / 1024, stats.free / 1024);
```

**Fichier** : `kernel/src/memory/heap/mod.rs` (déjà implémenté)  
**Features** :
- Linked-list allocator
- First-fit allocation
- Automatic coalescing

### 1.4 Exemple d'utilisation pour POSIX-X (mmap)

```rust
// kernel/src/posix_x/syscalls/fast_path/memory.rs
use crate::memory::{map_page, alloc_frame, PageFlags};
use crate::memory::address::{VirtualAddress, PhysicalAddress};

pub fn sys_mmap(addr: usize, len: usize, prot: i32, flags: i32) -> Result<usize, SyscallError> {
    let page_count = (len + 4095) / 4096;
    let virt_base = VirtualAddress::new(addr);
    
    // Créer les flags à partir de POSIX prot
    let page_flags = PageFlags::from_prot(prot);
    
    // Allouer et mapper les pages
    for i in 0..page_count {
        let phys = alloc_frame().map_err(|_| SyscallError::OutOfMemory)?;
        let virt = VirtualAddress::new(addr + i * 4096);
        
        map_page(virt, phys, page_flags)
            .map_err(|_| SyscallError::MapFailed)?;
    }
    
    Ok(addr)
}

pub fn sys_munmap(addr: usize, len: usize) -> Result<(), SyscallError> {
    let page_count = (len + 4095) / 4096;
    
    for i in 0..page_count {
        let virt = VirtualAddress::new(addr + i * 4096);
        
        // Unmapper et libérer la frame physique
        if let Ok(phys) = unmap_page(virt) {
            let _ = free_frame(phys);
        }
    }
    
    Ok(())
}

pub fn sys_brk(addr: usize) -> Result<usize, SyscallError> {
    // TODO: Gérer le heap de l'espace utilisateur
    // Pour l'instant, simplement retourner l'adresse actuelle
    Ok(addr)
}
```

### 1.5 Exemple pour Drivers (DMA allocation)

```rust
use crate::memory::physical::buddy_allocator;

// Allouer 1MB contiguous pour DMA buffer
let dma_buffer = buddy_allocator::alloc_contiguous(256)?; // 256 frames × 4KB = 1MB
let dma_addr = dma_buffer.value();

// Utiliser le buffer...

// Libérer
buddy_allocator::free_contiguous(dma_buffer, 256)?;
```

### 1.6 Types de base

```rust
// kernel/src/memory/address.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalAddress(usize);

impl PhysicalAddress {
    pub const fn new(addr: usize) -> Self;
    pub const fn value(&self) -> usize;
    pub const fn is_page_aligned(&self) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualAddress(usize);

impl VirtualAddress {
    pub const fn new(addr: usize) -> Self;
    pub const fn value(&self) -> usize;
    pub fn is_kernel(&self) -> bool;
    pub const fn is_page_aligned(&self) -> bool;
}

// Error type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    OutOfMemory,
    InvalidAddress,
    AlreadyMapped,
    NotMapped,
    PermissionDenied,
    AlignmentError,
    InvalidSize,
    InternalError(&'static str),
}

pub type MemoryResult<T> = Result<T, MemoryError>;
```

---

## 📡 2. SYSCALL API (✅ DISPONIBLE)

**Status** : ✅ IMPLÉMENTÉ  
**Responsable** : Copilot  
**Fichier** : `kernel/src/syscall/dispatch.rs` (300+ lignes)

### 2.1 Enregistrement de Syscalls

```rust
use crate::syscall::{register_syscall, SyscallHandler, SyscallError, syscall_numbers};

// Type de handler
pub type SyscallHandler = fn(args: &[u64; 6]) -> Result<u64, SyscallError>;

// Enregistrer un syscall
register_syscall(syscall_numbers::SYS_READ, my_read_handler)?;

// Exemple de handler
fn my_read_handler(args: &[u64; 6]) -> Result<u64, SyscallError> {
    let fd = args[0] as i32;
    let buf = args[1] as *mut u8;
    let count = args[2] as usize;
    
    // Implementation...
    Ok(bytes_read)
}
```

### 2.2 Syscall Numbers (Linux-compatible)

```rust
use crate::syscall::syscall_numbers::*;

// Fichiers
pub const SYS_READ: usize = 0;
pub const SYS_WRITE: usize = 1;
pub const SYS_OPEN: usize = 2;
pub const SYS_CLOSE: usize = 3;

// Mémoire
pub const SYS_MMAP: usize = 9;
pub const SYS_MUNMAP: usize = 11;
pub const SYS_BRK: usize = 12;

// Processus
pub const SYS_FORK: usize = 57;
pub const SYS_EXECVE: usize = 59;
pub const SYS_EXIT: usize = 60;
pub const SYS_GETPID: usize = 39;

// + 40 autres syscalls standards
```

### 2.3 Erreurs Syscall

```rust
#[derive(Debug, Clone, Copy)]
pub enum SyscallError {
    InvalidSyscall = -1,
    InvalidArgument = -2,
    PermissionDenied = -3,
    NotFound = -4,
    AlreadyExists = -5,
    OutOfMemory = -6,
    IoError = -7,
    // ... etc
}

impl SyscallError {
    pub fn to_errno(self) -> i64;
}
```

### 2.4 Initialisation

```rust
// Dans kernel init
unsafe {
    crate::syscall::init();  // Configure MSRs + handlers par défaut
}
```

### 2.5 Exemple POSIX-X Usage

```rust
// kernel/src/posix_x/syscalls/fast_path/vfs.rs
use crate::syscall::{register_syscall, syscall_numbers::SYS_OPEN, SyscallError};

fn sys_open_handler(args: &[u64; 6]) -> Result<u64, SyscallError> {
    let path_ptr = args[0] as *const u8;
    let flags = args[1] as i32;
    let mode = args[2] as u32;
    
    // Convertir path
    let path = unsafe { c_str_to_rust(path_ptr) };
    
    // Appeler VFS
    match crate::fs::vfs::open(path, flags, mode) {
        Ok(fd) => Ok(fd as u64),
        Err(_) => Err(SyscallError::NotFound),
    }
}

pub fn init_vfs_syscalls() {
    register_syscall(SYS_OPEN, sys_open_handler).unwrap();
    // ... autres
}
```

---

## 🔄 3. SCHEDULER API (✅ DISPONIBLE)

**Status** : ✅ IMPLÉMENTÉ  
**Responsable** : Copilot  
**Fichiers** : `kernel/src/scheduler/` (600+ lignes)

### 3.1 Thread Management

```rust
use crate::scheduler::{SCHEDULER, ThreadId, ThreadState, ThreadPriority};

// Créer un thread
let tid = SCHEDULER.spawn("worker", worker_entry, 8192);

// Entry point
fn worker_entry() -> ! {
    loop {
        // Do work...
        SCHEDULER.yield_now();  // Voluntarily yield
    }
}

// Bloquer le thread courant
SCHEDULER.block_current();

// Débloquer un thread
SCHEDULER.unblock_thread(tid);
```

### 3.2 Thread Structure

```rust
pub struct Thread {
    id: ThreadId,
    name: Box<str>,
    state: ThreadState,
    priority: ThreadPriority,
    context: ThreadContext,  // RSP, RIP, CR3, RFLAGS
    // ... stats
}

pub enum ThreadState {
    Ready,
    Running,
    Blocked,
    Terminated,
}

pub enum ThreadPriority {
    Idle = 0,
    Low = 1,
    Normal = 2,
    High = 3,
    Realtime = 4,
}
```

### 3.3 Context Switch (Windowed)

```rust
#[repr(C)]
pub struct ThreadContext {
    pub rsp: u64,     // Stack pointer
    pub rip: u64,     // Instruction pointer
    pub cr3: u64,     // Page table
    pub rflags: u64,  // Flags
}

// Uniquement 4 registres sauvés = 304 cycles (vs 2134 Linux)
```

### 3.4 3-Queue EMA Scheduler

```rust
pub enum QueueType {
    Hot,     // <1ms EMA runtime
    Normal,  // 1-10ms EMA
    Cold,    // >10ms EMA
}

// Threads migrés automatiquement entre queues selon EMA
// Hot > Normal > Cold (priorité scheduling)
```

### 3.5 Statistics

```rust
let stats = SCHEDULER.stats();
println!("Total switches: {}", stats.total_switches);
println!("Hot queue: {} threads", stats.hot_queue_len);
println!("Normal queue: {} threads", stats.normal_queue_len);
println!("Cold queue: {} threads", stats.cold_queue_len);

// Thread-level stats
let thread_stats = thread.stats();
println!("Runtime: {}ns", thread_stats.total_runtime_ns);
println!("Context switches: {}", thread_stats.context_switches);
println!("EMA runtime: {}ns", thread_stats.ema_runtime_ns);
```

### 3.6 Initialisation

```rust
// Dans kernel init
crate::scheduler::init();

// Spawn threads initiaux
SCHEDULER.spawn("idle", idle_thread, 4096);
SCHEDULER.spawn("init", init_thread, 16384);

// Démarrer le scheduling
crate::scheduler::start();  // Never returns
```

---

## 🧠 4. IPC API (⏳ EN COURS)

**Status** : ⏳ EN DÉVELOPPEMENT  
**ETA** : Après Scheduler (8-10h)

### Structure Prévue

```rust
// Fusion Rings - Zero-copy IPC
pub fn create_channel<T>() -> Result<(Sender<T>, Receiver<T>), IpcError>;

// Inline path (≤56B, <400 cycles)
// Zero-copy path (>56B, <900 cycles)
```

---

## 🔐 5. Security API (⏳ EN ATTENTE)

**Status** : ⏳ EN ATTENTE  
**ETA** : Après IPC (12-14h)

### Structure Prévue

```rust
// Capabilities system
pub struct Capability {
    object: ObjectId,
    rights: Rights,
}
```

---

## 📝 Convention de Nommage

### Fichiers

- Modules : `snake_case.rs`
- Tests : `tests/test_module_name.rs`

### Code Rust

- Structs : `PascalCase`
- Enums : `PascalCase`
- Traits : `PascalCase`
- Functions : `snake_case`
- Constants : `SCREAMING_SNAKE_CASE`
- Modules : `snake_case`

### Code C

- Fonctions : `snake_case`
- Macros : `SCREAMING_SNAKE_CASE`
- Types : `snake_case_t`

---

## 🧪 Tests Requis

Chaque module doit avoir :

1. **Tests unitaires** : Dans `#[cfg(test)] mod tests`
2. **Tests d'intégration** : Dans `tests/`
3. **Benchmarks** : Dans `benches/` si applicable
4. **Documentation** : Exemples dans doc comments

### Exemple

```rust
/// Alloue une frame physique de mémoire.
///
/// # Examples
///
/// ```
/// use exo_os::memory::alloc_frame;
///
/// let frame = alloc_frame()?;
/// assert!(frame.is_valid());
/// ```
///
/// # Errors
///
/// Retourne `AllocError::OutOfMemory` si plus de frames disponibles.
pub fn alloc_frame() -> Result<PhysFrame, AllocError> {
    // Implementation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alloc_frame() {
        let frame = alloc_frame().unwrap();
        assert!(frame.start_address() % 4096 == 0);
    }
}
```

---

## 📊 Performance Requirements

### Objectifs par opération

| Opération | Cycles Max | Notes |
|-----------|------------|-------|
| Driver call | < 200 | Via trait dispatch |
| Memory alloc (cache hit) | < 10 | Thread-local |
| Memory alloc (slab) | < 60 | CPU-local |
| IPC send (inline) | < 400 | ≤56B inline |
| IPC send (zero-copy) | < 900 | >56B shared mem |
| Syscall (fast path) | < 60 | SYSCALL/SYSRET |

### Mesurer avec rdtsc

```rust
use core::arch::x86_64::_rdtsc;

let start = unsafe { _rdtsc() };
// ... operation ...
let end = unsafe { _rdtsc() };
let cycles = end - start;
```

---

## 🎯 Priorités d'Implémentation (Pour Gemini)

### Phase 1 (Immédiate)

1. **Utils** : Commencer dès maintenant
2. **Tests framework** : Préparer infrastructure

### Phase 2 (Après interfaces boot)

3. **Drivers de base** : Serial, VGA, Keyboard
4. **Filesystem minimal** : tmpfs

### Phase 3 (Après IPC)

5. **Network stack** : Ethernet + IP
6. **POSIX-X** : Mapping syscalls

### Phase 4 (Final)

7. **AI agents** : Si temps disponible

---

## 📞 Contact

Si questions sur interfaces :

1. Poster dans `PROBLEMS.md` avec tag [QUESTION]
2. Je répondrai dans les 30 minutes
3. Mise à jour de ce document si clarification nécessaire

---

**Note** : Ce document sera mis à jour progressivement au fur et à mesure que les zones critiques avancent.
