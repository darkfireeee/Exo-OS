# 🔧 IMPLÉMENTATION CoW dans sys_fork() - Guide Technique

**Date**: 2026-01-03
**Objectif**: Intégrer CoW Manager dans sys_fork() - Code production ready

---

## ✅ CE QUI EXISTE DÉJÀ

### CoW Manager (kernel/src/memory/cow_manager.rs)

```rust
// API publique disponible:
pub fn mark_cow(phys: PhysicalAddress) -> u32
pub fn is_cow(phys: PhysicalAddress) -> bool  
pub fn get_refcount(phys: PhysicalAddress) -> Option<u32>
pub fn handle_cow_fault(virt: VirtualAddress, phys: PhysicalAddress) 
    -> Result<PhysicalAddress, CowError>
pub fn free_cow_page(phys: PhysicalAddress)
pub fn clone_address_space(
    pages: &[(VirtualAddress, PhysicalAddress, PageTableFlags)]
) -> Result<Vec<(VirtualAddress, PhysicalAddress, PageTableFlags)>, CowError>
```

### Page Fault Handler (kernel/src/memory/virtual_mem/mod.rs:347-385)

```rust
fn handle_cow_page_fault(virtual_addr: VirtualAddress) -> MemoryResult<()> {
    let current_physical = mapper::get_physical_address(virtual_addr)?;
    let new_physical = crate::memory::cow_manager::handle_cow_fault(
        virtual_addr, current_physical
    )?;
    
    if new_physical == current_physical {
        // Refcount==1: just update flags
        mapper.protect_page(virtual_addr, flags.writable())?;
    } else {
        // Refcount>1: remap
        mapper.unmap_page(virtual_addr)?;
        mapper.map_page(virtual_addr, new_physical, flags.writable())?;
    }
    
    invalidate_tlb(virtual_addr);
    Ok(())
}
```

**Status**: ✅ DÉJÀ INTÉGRÉ et testé (Jour 3)

---

## ❌ CE QUI MANQUE

### 1. sys_fork() n'appelle PAS CoW

**Fichier**: `kernel/src/syscall/handlers/process.rs:223`

**Code actuel**:
```rust
pub fn sys_fork() -> MemoryResult<Pid> {
    // 1. Get parent thread
    // 2. Allocate child TID
    // 3. Create child thread with Thread::new_kernel()
    // 4. Add to scheduler
    // 5. Return child_tid
}
```

**Problèmes**:
- ❌ Pas de capture address space
- ❌ Pas d'appel à `clone_address_space()`
- ❌ Pas de marquage pages RO
- ❌ Child thread créé avec `new_kernel()` (stub)

### 2. Fonctions Helper Manquantes

```rust
// MANQUE: Capturer toutes les pages mappées du parent
fn capture_address_space() -> Result<Vec<(Virt, Phys, Flags)>, MemoryError>

// MANQUE: Marquer page en read-only
fn protect_page(virt: VirtualAddress, flags: UserPageFlags) -> MemoryResult<()>

// MANQUE: Obtenir flags d'une page
fn get_page_flags(virt: VirtualAddress) -> MemoryResult<UserPageFlags>

// MANQUE: Cloner contexte thread (registres)
impl ThreadContext {
    fn clone(&self) -> Self;
    fn set_return_value(&mut self, val: u64);
}

// MANQUE: Créer thread avec context + address space custom
impl Thread {
    fn new_with_context(
        id: u64,
        name: &str,
        context: ThreadContext,
        pages: Vec<(Virt, Phys, Flags)>,
    ) -> Self;
}
```

---

## 🎯 PLAN D'IMPLÉMENTATION

### Étape 1: Ajouter fonctions virtual_mem

**Fichier**: `kernel/src/memory/virtual_mem/mod.rs`

```rust
/// Obtenir flags d'une page
pub fn get_page_flags(virt: VirtualAddress) -> MemoryResult<UserPageFlags> {
    // Lire page table entry
    let mapper = get_current_mapper()?;
    mapper.get_flags(virt)
}

/// Marquer page en read-only (pour CoW)
pub fn protect_page(virt: VirtualAddress, new_flags: UserPageFlags) -> MemoryResult<()> {
    let mapper = get_current_mapper()?;
    mapper.update_flags(virt, new_flags)?;
    
    // Flush TLB pour cette page
    unsafe {
        asm!("invlpg [{}]", in(reg) virt.value(), options(nostack));
    }
    
    Ok(())
}

/// Obtenir adresse physique d'une page
pub fn get_physical_address(virt: VirtualAddress) -> MemoryResult<Option<PhysicalAddress>> {
    let mapper = get_current_mapper()?;
    Ok(mapper.translate(virt))
}
```

### Étape 2: Étendre ThreadContext

**Fichier**: `kernel/src/scheduler/thread.rs`

```rust
impl ThreadContext {
    /// Cloner contexte (registres)
    pub fn clone(&self) -> Self {
        Self {
            rip: self.rip,
            rsp: self.rsp,
            rbp: self.rbp,
            rax: self.rax,
            rbx: self.rbx,
            // ... tous les registres
        }
    }
    
    /// Set return value (RAX pour x86-64)
    pub fn set_return_value(&mut self, val: u64) {
        self.rax = val;
    }
}
```

### Étape 3: Ajouter Thread::new_with_context

**Fichier**: `kernel/src/scheduler/thread.rs`

```rust
impl Thread {
    /// Créer thread avec contexte custom (pour fork)
    pub fn new_with_context(
        id: u64,
        name: &str,
        context: ThreadContext,
        pages: Vec<(VirtualAddress, PhysicalAddress, UserPageFlags)>,
    ) -> Self {
        Self {
            id,
            name: name.to_string(),
            state: ThreadState::Ready,
            context,
            address_space: Some(pages), // Store cloned pages
            stack_bottom: context.rsp, // Use parent's stack
            priority: ThreadPriority::Normal,
        }
    }
}
```

### Étape 4: Implémenter capture_address_space

**Fichier**: `kernel/src/syscall/handlers/process.rs`

```rust
/// Capturer toutes les pages mappées du thread actuel
fn capture_address_space() -> MemoryResult<Vec<(VirtualAddress, PhysicalAddress, UserPageFlags)>> {
    use crate::memory::virtual_mem;
    
    let mut pages = Vec::new();
    
    // Parcourir user space (0x1000 - 0x7fff_ffff_ffff)
    let start = VirtualAddress::new(0x1000); // Skip null page
    let end = VirtualAddress::new(0x7fff_0000_0000); // 128TB user space
    
    let mut addr = start;
    while addr.value() < end.value() {
        // Vérifier si page mappée
        if let Ok(Some(phys)) = virtual_mem::get_physical_address(addr) {
            // Obtenir flags
            if let Ok(flags) = virtual_mem::get_page_flags(addr) {
                pages.push((addr, phys, flags));
            }
        }
        
        // Page suivante
        addr = VirtualAddress::new(addr.value() + PAGE_SIZE);
        
        // Stop si trop de pages (limite sécurité)
        if pages.len() > 100_000 {
            break; // ~400MB max
        }
    }
    
    Ok(pages)
}
```

### Étape 5: Remplacer sys_fork()

**Fichier**: `kernel/src/syscall/handlers/process.rs:223`

```rust
/// Fork avec Copy-on-Write (COMPLET)
pub fn sys_fork() -> MemoryResult<Pid> {
    log::info!("[FORK] Starting with CoW");
    
    // 1. Get parent thread
    let parent_tid = SCHEDULER.with_current_thread(|t| t.id())
        .ok_or(MemoryError::InvalidAddress)?;
    
    // 2. Capture parent's address space
    let parent_pages = capture_address_space()?;
    log::info!("[FORK] Captured {} pages", parent_pages.len());
    
    // 3. Clone avec CoW (marque refcount=2, pages RO)
    let child_pages = crate::memory::clone_address_space(&parent_pages)
        .map_err(|_| MemoryError::OutOfMemory)?;
    
    // 4. Marquer pages parent en RO
    for (virt, _phys, flags) in &parent_pages {
        if flags.contains(UserPageFlags::WRITABLE) {
            let ro_flags = flags.difference(UserPageFlags::WRITABLE);
            crate::memory::virtual_mem::protect_page(*virt, ro_flags)?;
        }
    }
    
    log::info!("[FORK] {} pages marked CoW", child_pages.len());
    
    // 5. Allocate child PID
    let child_pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
    
    // 6. Clone parent context (registres)
    let parent_context = SCHEDULER.with_current_thread(|t| {
        t.context().clone()
    }).ok_or(MemoryError::InvalidAddress)?;
    
    // 7. Créer contexte child (RAX=0 pour return value)
    let mut child_context = parent_context;
    child_context.set_return_value(0); // Child retourne 0
    
    // 8. Créer thread child avec pages clonées
    let child_thread = Thread::new_with_context(
        child_pid,
        "forked_child",
        child_context,
        child_pages,
    );
    
    // 9. Ajouter au scheduler
    SCHEDULER.add_thread(child_thread)
        .map_err(|_| MemoryError::OutOfMemory)?;
    
    log::info!("[FORK] Success: child PID {}", child_pid);
    
    // 10. Retourner child_pid au parent
    Ok(child_pid)
}
```

---

## 🧪 TESTS UNITAIRES

### Test 1: Compilation

```bash
cd kernel
cargo build --release
```

**Expected**: ✅ Pas d'erreurs

### Test 2: Tests Unitaires

```bash
cargo test --lib memory::cow_manager
```

**Expected**: ✅ 4/4 tests passent

### Test 3: Test QEMU Basique

Créer `userland/test_cow_simple.c`:
```c
#include <stdio.h>
#include <unistd.h>

int main() {
    printf("[TEST] Fork CoW test\n");
    
    int var = 42;
    printf("[PARENT] var=%d before fork\n", var);
    
    pid_t pid = fork();
    
    if (pid == 0) {
        // CHILD
        printf("[CHILD] var=%d before write\n", var);
        var = 99; // TRIGGER CoW
        printf("[CHILD] var=%d after write\n", var);
        return 0;
    } else {
        // PARENT  
        wait(NULL);
        printf("[PARENT] var=%d after child\n", var);
        
        if (var == 42) {
            printf("[TEST] ✅ PASS\n");
        } else {
            printf("[TEST] ❌ FAIL\n");
        }
    }
    
    return 0;
}
```

Compiler et tester:
```bash
cd userland
gcc -static test_cow_simple.c -o test_cow_simple

cd ..
make qemu
# Dans QEMU shell:
/userland/test_cow_simple
```

**Expected**:
```
[TEST] Fork CoW test
[PARENT] var=42 before fork
[CHILD] var=42 before write
[CHILD] var=99 after write
[PARENT] var=42 after child
[TEST] ✅ PASS
```

---

## 📊 CRITÈRES DE SUCCÈS

### Phase 1: Compilation ✅
- [ ] kernel compile sans erreurs
- [ ] Pas de warnings CoW
- [ ] Tests unitaires passent

### Phase 2: Tests Manuels QEMU ✅
- [ ] fork() ne crash pas
- [ ] Child process créé
- [ ] Parent continue execution
- [ ] Variables séparées (test CoW)

### Phase 3: Validation CoW ✅
- [ ] Write déclenche page fault
- [ ] Page copiée (refcount 2→1)
- [ ] Parent garde ancienne valeur
- [ ] Child a nouvelle valeur

### Phase 4: Métriques ✅
- [ ] Latence page fault <1500 cycles
- [ ] Refcount correct (2 après fork, 1 après write)
- [ ] Pas de memory leak

---

## 🚀 ORDRE D'IMPLÉMENTATION

**Aujourd'hui (Jour 4 redéfini)**:

1. ✅ Analyser ce qui manque (FAIT)
2. 🔄 Ajouter fonctions virtual_mem (2h)
3. 🔄 Étendre ThreadContext (1h)
4. 🔄 Modifier sys_fork() (2h)
5. 🔄 Tests compilation (30min)
6. 🔄 Tests QEMU basiques (2h)

**Total**: 7-8h → Jour 4 complet

---

## 📝 CHECKLIST AVANT COMMIT

- [ ] Code compile
- [ ] Tests unitaires passent
- [ ] Test QEMU fork() works
- [ ] Test QEMU CoW works (parent≠child)
- [ ] Pas de TODOs dans code
- [ ] Documentation inline
- [ ] Commit message détaillé

---

**Status**: 📋 Plan technique prêt  
**Next**: Implémenter étape par étape
