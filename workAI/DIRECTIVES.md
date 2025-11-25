# 🎯 DIRECTIVES TECHNIQUES PARTAGÉES

**Pour** : Copilot & Gemini
**Objectif** : Standards communs de développement
**Dernière mise à jour** : 23 novembre 2025 - 14:00

---

## 🔧 PROCÉDURE DE BUILD (IMPORTANT)

### Compilation du Boot
Les fichiers boot nécessitent une compilation spéciale (voir `BUILD_PROCESS.md` pour détails complets).

**Workflow obligatoire** :
```powershell
# 1. Compiler boot.asm + boot.c
.\link_boot.ps1

# 2. Compiler le kernel
cargo build
```

**Fichiers concernés** :
- `kernel/src/arch/x86_64/boot/boot.asm` (NASM → ELF64)
- `kernel/src/arch/x86_64/boot/boot.c` (GCC → ELF64)
- Output : `libboot_combined.a` (archive statique)

**Raison** : rust-lld incompatible avec objets ELF64 natifs. Le script crée une archive .a compatible.

📖 **Documentation complète** : Voir `workAI/BUILD_PROCESS.md`

---

## 📐 Architecture Générale

### Philosophie Zero-Copy
```
❌ MAUVAIS : Copier des données
let mut buffer = Vec::new();
buffer.extend_from_slice(&data);

✅ BON : Référencer directement
let buffer: &[u8] = &data;

✅ MEILLEUR : Shared memory
let shm = SharedMemory::new(size)?;
// Processus A et B partagent shm sans copie
```

### Philosophie Lock-Free
```
❌ MAUVAIS : Mutex pour compteur
let counter = Mutex::new(0);
{
    let mut c = counter.lock();
    *c += 1;
}

✅ BON : Atomic
let counter = AtomicU64::new(0);
counter.fetch_add(1, Ordering::Relaxed);

✅ MEILLEUR : Thread-local quand possible
thread_local! {
    static COUNTER: Cell<u64> = Cell::new(0);
}
COUNTER.with(|c| c.set(c.get() + 1));
```

---

## 🔒 Gestion d'Erreurs

### Result<T, E> Obligatoire

```rust
❌ MAUVAIS : Panic
pub fn alloc_frame() -> PhysFrame {
    if out_of_memory {
        panic!("Out of memory!");
    }
    frame
}

✅ BON : Result
pub fn alloc_frame() -> Result<PhysFrame, AllocError> {
    if out_of_memory {
        return Err(AllocError::OutOfMemory);
    }
    Ok(frame)
}
```

### Hiérarchie d'Erreurs

```rust
// Erreur générale du module
#[derive(Debug)]
pub enum MemoryError {
    OutOfMemory,
    InvalidAddress(VirtAddr),
    NotMapped(VirtAddr),
    PermissionDenied,
    // ...
}

impl core::fmt::Display for MemoryError { /* ... */ }
impl core::error::Error for MemoryError {}
```

---

## 🎨 Style de Code

### Rust

#### Longueur des Lignes
```rust
// Max 100 caractères par ligne
// Si plus long, découper :

// ❌ Trop long
pub fn create_channel<T: Send + Sync + 'static>(capacity: usize, flags: ChannelFlags) -> Result<(Sender<T>, Receiver<T>), ChannelError> {

// ✅ Découpé
pub fn create_channel<T: Send + Sync + 'static>(
    capacity: usize,
    flags: ChannelFlags,
) -> Result<(Sender<T>, Receiver<T>), ChannelError> {
```

#### Imports
```rust
// Groupes :
// 1. std/core/alloc
// 2. External crates
// 3. Modules locaux

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::vec::Vec;

use spin::Mutex;
use bitflags::bitflags;

use crate::memory::{PhysFrame, VirtAddr};
use crate::arch::x86_64::paging;
```

#### Documentation
```rust
/// Description courte (une ligne).
///
/// Description longue avec détails.
///
/// # Examples
///
/// ```
/// let frame = alloc_frame()?;
/// ```
///
/// # Errors
///
/// - `OutOfMemory` : Plus de frames disponibles
///
/// # Safety
///
/// (Si fonction unsafe) Conditions de sécurité
///
/// # Panics
///
/// (Si peut paniquer) Conditions de panic
pub fn alloc_frame() -> Result<PhysFrame, AllocError> {
    // ...
}
```

### C

#### Style Kernel Linux
```c
// Variables : snake_case
int frame_count = 0;

// Fonctions : snake_case
void* alloc_frame(size_t size) {
    // ...
}

// Macros : SCREAMING_SNAKE_CASE
#define MAX_FRAMES 1024

// Types : snake_case_t
typedef struct {
    uint64_t address;
    uint64_t size;
} frame_t;

// Indentation : 4 spaces (pas de tabs)
if (condition) {
    do_something();
} else {
    do_other();
}
```

### ASM (NASM)

```asm
; Commentaires : Point-virgule
; Labels : snake_case
; Instructions : Minuscules

section .text
global _start

_start:
    ; Commentaire descriptif obligatoire
    mov rax, 0          ; Clear RAX
    mov rbx, 1          ; Set RBX to 1
    
    ; Sauter une ligne entre blocs logiques
    call init_gdt
    call init_idt
    
    ; Jump vers Rust
    jmp rust_kernel_main

; Fonctions : Commentaire descriptif
; Arguments dans registres (x86_64 calling convention)
init_gdt:
    push rbp
    mov rbp, rsp
    
    ; ... code ...
    
    mov rsp, rbp
    pop rbp
    ret
```

---

## ⚡ Optimisations

### Cache Alignment

```rust
// Structures critiques : Align sur cache line (64 bytes)
#[repr(C, align(64))]
pub struct FusionRingSlot {
    sequence: AtomicU64,    // 8 bytes
    data: [u8; 56],         // 56 bytes
}
// Total : 64 bytes = 1 cache line

// Éviter false sharing :
struct PerCpuData {
    #[cache_aligned]
    cpu_local: CpuLocal,    // Chaque CPU a sa cache line
}
```

### Branch Prediction

```rust
use core::intrinsics::{likely, unlikely};

// Hint au CPU : cas probable
if likely(cache_hit) {
    // Fast path
} else {
    // Slow path (rare)
}

// Hint au CPU : erreur improbable  
if unlikely(error) {
    // Error handling (rare)
}
```

### Inline Hints

```rust
// Toujours inline (petite fonction appelée souvent)
#[inline(always)]
pub fn atomic_load_relaxed(ptr: &AtomicU64) -> u64 {
    ptr.load(Ordering::Relaxed)
}

// Jamais inline (grosse fonction appelée rarement)
#[inline(never)]
pub fn handle_panic(info: &PanicInfo) -> ! {
    // ...
}

// Compiler décide (par défaut)
#[inline]
pub fn small_function() {
    // ...
}
```

---

## 🧪 Tests

### Structure de Test

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    // Test unitaire simple
    #[test]
    fn test_alloc_frame() {
        let frame = alloc_frame().unwrap();
        assert!(frame.is_valid());
    }
    
    // Test avec should_panic
    #[test]
    #[should_panic(expected = "Out of memory")]
    fn test_alloc_frame_oom() {
        // Setup qui cause OOM
        fill_memory();
        let _ = alloc_frame();
    }
    
    // Test de performance
    #[test]
    fn bench_alloc_frame() {
        let start = rdtsc();
        let _ = alloc_frame();
        let end = rdtsc();
        
        let cycles = end - start;
        assert!(cycles < 100, "Alloc trop lent: {} cycles", cycles);
    }
}
```

### Tests d'Intégration

```rust
// tests/test_memory.rs
use exo_os::memory::*;

#[test]
fn test_memory_subsystem() {
    // Setup
    init_memory().unwrap();
    
    // Test allocation
    let frame1 = alloc_frame().unwrap();
    let frame2 = alloc_frame().unwrap();
    assert_ne!(frame1, frame2);
    
    // Test déallocation
    dealloc_frame(frame1).unwrap();
    
    // Cleanup
    shutdown_memory();
}
```

---

## 📊 Benchmarks

### Mesure de Performance

```rust
use core::arch::x86_64::_rdtsc;

pub fn benchmark_operation<F>(name: &str, mut op: F, iterations: usize)
where
    F: FnMut(),
{
    // Warmup
    for _ in 0..100 {
        op();
    }
    
    // Mesure
    let start = unsafe { _rdtsc() };
    for _ in 0..iterations {
        op();
    }
    let end = unsafe { _rdtsc() };
    
    let total_cycles = end - start;
    let avg_cycles = total_cycles / iterations as u64;
    
    println!("{}: {} cycles (avg over {} iterations)", 
             name, avg_cycles, iterations);
}

// Utilisation
benchmark_operation("alloc_frame", || {
    let _ = alloc_frame();
}, 10000);
```

---

## 🔍 Debugging

### Serial Output

```rust
// Macro pour debug précoce (avant logger)
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::arch::serial::write_fmt(format_args!($($arg)*))
    };
}

// Utilisation
serial_print!("[DEBUG] Frame allocated at {:#x}\n", addr);
```

### Assertions

```rust
// Debug assertions (désactivées en release)
debug_assert!(condition);
debug_assert_eq!(a, b);

// Release assertions (toujours actives)
assert!(critical_condition, "Reason: {}", reason);
```

---

## 📦 Dépendances

### Crates Autorisées

#### no_std Safe
- ✅ `spin` : Mutex no_std
- ✅ `bitflags` : Bit flags
- ✅ `lazy_static` : Static init
- ✅ `x86_64` : x86_64 utils

#### Cas par Cas
- ⚠️ `alloc` : Seulement si heap initialisé
- ⚠️ `hashbrown` : HashMap no_std (mais lourd)

#### Interdites
- ❌ `std` : On est no_std!
- ❌ Crates avec unsafe non audité
- ❌ Crates trop lourdes (>50KB compiled)

---

## 🎯 Checklist Avant Commit

### Code
- [ ] Compile sans warnings (`cargo build`)
- [ ] Passe rustfmt (`cargo fmt --check`)
- [ ] Passe clippy (`cargo clippy -- -D warnings`)
- [ ] Tests passent (`cargo test`)
- [ ] Documentation à jour

### Performance
- [ ] Pas de copies inutiles (zero-copy)
- [ ] Pas de locks dans fast path
- [ ] Cache alignment vérifié
- [ ] Benchmarks si critique

### Sécurité
- [ ] Pas d'unsafe non justifié
- [ ] Validation des inputs
- [ ] Gestion d'erreurs complète
- [ ] Pas de panic dans kernel path

---

## 🚀 Workflow Git

### Branches
```bash
# Feature branch
git checkout -b feature/driver-keyboard

# Fix branch  
git checkout -b fix/memory-leak

# Zone branch (pour zones séparées)
git checkout -b zone/copilot-ipc
git checkout -b zone/gemini-drivers
```

### Commits
```
Format : <type>(<scope>): <description>

Types :
- feat: Nouvelle feature
- fix: Bug fix
- perf: Amélioration perf
- refactor: Refactoring
- docs: Documentation
- test: Tests
- chore: Maintenance

Exemples :
feat(memory): Add buddy allocator
fix(ipc): Fix race condition in ring buffer
perf(scheduler): Optimize context switch to 304 cycles
docs(api): Document driver interface
```

### Pull Request
```markdown
## Description
Brève description du changement

## Type de changement
- [ ] Nouvelle feature
- [ ] Bug fix
- [ ] Breaking change

## Checklist
- [ ] Tests ajoutés
- [ ] Documentation mise à jour
- [ ] Benchmarks validés
- [ ] Pas de régression

## Performance
(Si applicable)
- Before: XXX cycles
- After: YYY cycles
- Gain: ZZ%
```

---

**Important** : Respecter ces directives garantit la cohérence et la qualité du code !
