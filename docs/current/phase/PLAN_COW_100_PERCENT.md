# 🎯 PLAN CoW 100% COMPLET - Tests Réels QEMU

**Date**: 2026-01-03  
**Objectif**: Finaliser CoW à 100% avec tests réels + métriques

---

## ❌ PROBLÈMES IDENTIFIÉS

### 1. CoW Manager PAS intégré dans sys_fork()

**Fichier**: `kernel/src/syscall/handlers/process.rs:223`

**Code actuel**:
```rust
pub fn sys_fork() -> MemoryResult<Pid> {
    // ...
    let child_thread = Thread::new_kernel(...);
    SCHEDULER.add_thread(child_thread);
    Ok(child_tid)
}
```

**Problème**: 
- ✅ CoW Manager existe (343 lignes)
- ❌ **JAMAIS appelé** dans sys_fork()
- ❌ Pas de `clone_address_space()`
- ❌ Pas de marquage pages CoW
- ❌ Pas de refcount tracking

### 2. Tests seulement avec Mocks

**Tests actuels**:
- ✅ 8 tests unitaires (mocks)
- ✅ 2 tests intégration (mocks)
- ❌ **0 tests QEMU réels**
- ❌ **0 métriques réelles**

### 3. Pas de validation end-to-end

**Manque**:
- ❌ Test fork() réel dans QEMU
- ❌ Test write déclenche CoW
- ❌ Test refcount décrémenté
- ❌ Test cleanup mémoire
- ❌ Métriques latence/throughput

---

## ✅ PLAN COMPLET 100%

### Phase 1: Intégrer CoW dans sys_fork() ⏱️ 2h

**Tâche 1.1**: Importer CoW Manager dans process.rs
```rust
use crate::memory::cow_manager::{COW_MANAGER, CowError};
```

**Tâche 1.2**: Modifier sys_fork() pour utiliser CoW
```rust
pub fn sys_fork() -> MemoryResult<Pid> {
    // 1. Capturer contexte parent
    let parent_context = capture_parent_context();
    
    // 2. Cloner address space avec CoW
    let (child_pages, parent_pages_updated) = 
        COW_MANAGER.lock().clone_address_space(&parent_context.pages)?;
    
    // 3. Marquer pages parent + child en read-only
    for page in &parent_pages_updated {
        mark_page_readonly(page.virt)?;
    }
    
    // 4. Créer child avec nouveau address space
    let child_thread = Thread::new_with_address_space(
        child_tid,
        child_pages,
        parent_context.stack_ptr,
        parent_context.instruction_ptr,
    );
    
    // 5. Retourner 0 au child, child_pid au parent
    Ok(child_tid)
}
```

**Tâche 1.3**: Implémenter capture_parent_context()
- Capturer RSP, RIP, RBP
- Capturer page tables actuelles
- Retourner toutes les pages mappées

**Tâche 1.4**: Implémenter mark_page_readonly()
- Enlever flag WRITABLE de page
- Flush TLB entry

**Tests Phase 1**:
- Compiler sans erreurs
- Tests unitaires existants passent encore

---

### Phase 2: Tests QEMU Réels ⏱️ 3h

**Test 1: Fork basique dans QEMU**

Créer `userland/test_cow_fork.c`:
```c
#include <stdio.h>
#include <unistd.h>

int global_var = 42;

int main() {
    printf("[TEST] CoW Fork Test Starting\n");
    printf("[PARENT] global_var = %d (before fork)\n", global_var);
    
    pid_t pid = fork();
    
    if (pid == 0) {
        // CHILD
        printf("[CHILD] global_var = %d (after fork, before write)\n", global_var);
        
        // TRIGGER CoW
        global_var = 99;
        
        printf("[CHILD] global_var = %d (after write)\n", global_var);
        return 0;
    } else {
        // PARENT
        sleep(1); // Let child run first
        printf("[PARENT] global_var = %d (after child write)\n", global_var);
        
        // Verify parent still has 42
        if (global_var == 42) {
            printf("[TEST] ✅ PASS: CoW worked, parent has 42\n");
        } else {
            printf("[TEST] ❌ FAIL: Parent corrupted, has %d\n", global_var);
        }
        
        wait(NULL);
    }
    
    return 0;
}
```

**Test 2: Mesurer Refcount**

Ajouter syscall debug pour lire refcount:
```rust
pub fn sys_debug_cow_refcount(phys_addr: usize) -> i64 {
    COW_MANAGER.lock()
        .get_refcount(PhysicalAddress::new(phys_addr))
        .unwrap_or(0) as i64
}
```

Test userland:
```c
int main() {
    int var = 100;
    uintptr_t phys = virt_to_phys(&var); // Helper syscall
    
    long refcount1 = syscall(SYS_DEBUG_COW_REFCOUNT, phys);
    printf("Refcount before fork: %ld\n", refcount1); // Should be 0
    
    fork();
    
    long refcount2 = syscall(SYS_DEBUG_COW_REFCOUNT, phys);
    printf("Refcount after fork: %ld\n", refcount2); // Should be 2
    
    var = 200; // Trigger CoW
    
    long refcount3 = syscall(SYS_DEBUG_COW_REFCOUNT, phys);
    printf("Refcount after write: %ld\n", refcount3); // Should be 1
}
```

**Test 3: Latence CoW**

```c
#include <time.h>

int main() {
    volatile int data[1024]; // 4KB page
    
    // Warm up
    for (int i = 0; i < 1024; i++) data[i] = i;
    
    struct timespec start, end;
    
    fork();
    
    if (getpid() != 0) {
        // Child
        clock_gettime(CLOCK_MONOTONIC, &start);
        
        // Trigger CoW on entire page
        for (int i = 0; i < 1024; i++) {
            data[i] = i + 1;
        }
        
        clock_gettime(CLOCK_MONOTONIC, &end);
        
        long ns = (end.tv_sec - start.tv_sec) * 1000000000 +
                  (end.tv_nsec - start.tv_nsec);
        
        printf("CoW latency: %ld ns (%ld cycles)\n", ns, ns * 3); // 3GHz CPU
    }
}
```

**Test 4: Cleanup Mémoire**

```c
int main() {
    size_t mem_before = get_used_memory(); // Helper syscall
    
    for (int i = 0; i < 100; i++) {
        pid_t pid = fork();
        if (pid == 0) {
            exit(0); // Child exits immediately
        }
        wait(NULL);
    }
    
    size_t mem_after = get_used_memory();
    
    if (mem_before == mem_after) {
        printf("✅ PASS: No memory leak\n");
    } else {
        printf("❌ FAIL: Leaked %zu bytes\n", mem_after - mem_before);
    }
}
```

---

### Phase 3: Métriques Réelles ⏱️ 2h

**Métrique 1: Latence Page Fault CoW**

Ajouter compteur dans handle_cow_page_fault():
```rust
fn handle_cow_page_fault(virt: VirtualAddress) -> MemoryResult<()> {
    let start = rdtsc();
    
    // ... code existant ...
    
    let end = rdtsc();
    let cycles = end - start;
    
    log::info!("CoW page fault: {} cycles", cycles);
    
    // Update stats
    COW_STATS.lock().record_fault(cycles);
    
    Ok(())
}
```

**Métrique 2: Taux Économie Mémoire**

```rust
pub struct CowStats {
    total_forks: AtomicU64,
    pages_shared: AtomicU64,
    pages_copied: AtomicU64,
    total_fault_cycles: AtomicU64,
    num_faults: AtomicU64,
}

impl CowStats {
    pub fn memory_saved_bytes(&self) -> usize {
        let shared = self.pages_shared.load(Ordering::Relaxed) as usize;
        shared * PAGE_SIZE
    }
    
    pub fn avg_fault_cycles(&self) -> u64 {
        let total = self.total_fault_cycles.load(Ordering::Relaxed);
        let num = self.num_faults.load(Ordering::Relaxed);
        if num == 0 { 0 } else { total / num }
    }
}
```

**Métrique 3: Refcount Distribution**

```rust
pub fn get_cow_stats() -> CowStats {
    COW_MANAGER.lock().get_stats()
}
```

Afficher dans QEMU:
```
╔════════════════════════════════════════╗
║      CoW Statistics                    ║
╠════════════════════════════════════════╣
║ Total forks:         42                ║
║ Pages shared:        1024 (4MB saved)  ║
║ Pages copied:        128               ║
║ CoW efficiency:      88.9%             ║
║ Avg fault latency:   847 cycles        ║
║ Max refcount:        5                 ║
╚════════════════════════════════════════╝
```

---

### Phase 4: Tests Stress ⏱️ 2h

**Test Stress 1: Fork Bomb (limité)**

```c
int main() {
    for (int i = 0; i < 10; i++) {
        pid_t pid = fork();
        if (pid == 0) {
            // Child forks too
            for (int j = 0; j < 5; j++) {
                fork();
            }
            exit(0);
        }
    }
    
    // Wait all children
    while (wait(NULL) > 0);
    
    // Verify no leaks
    check_memory();
}
```

**Test Stress 2: Write Intensif**

```c
int main() {
    int data[10000]; // 40KB
    for (int i = 0; i < 10000; i++) data[i] = i;
    
    fork();
    
    // Both parent + child write everything
    for (int i = 0; i < 10000; i++) {
        data[i] = data[i] + 1;
    }
    
    // Verify all pages copied
}
```

**Test Stress 3: Fork + Exec**

```c
int main() {
    for (int i = 0; i < 100; i++) {
        pid_t pid = fork();
        if (pid == 0) {
            execve("/bin/true", NULL, NULL);
            exit(1);
        }
        wait(NULL);
    }
}
```

---

## 📊 CRITÈRES SUCCÈS 100%

### Code

- ✅ CoW Manager intégré dans sys_fork()
- ✅ clone_address_space() appelé
- ✅ Pages marquées read-only
- ✅ handle_cow_page_fault() fonctionne
- ✅ Refcount tracking actif
- ✅ Cleanup mémoire complet

### Tests

- ✅ 4 tests QEMU réels passent
- ✅ Fork + write works (global var test)
- ✅ Refcount correct (2 → 1)
- ✅ Latence <1500 cycles
- ✅ Pas de memory leak

### Métriques

- ✅ Latence moyenne mesurée
- ✅ Économie mémoire calculée
- ✅ Distribution refcount
- ✅ Efficiency >80%

### Stress

- ✅ Fork bomb (limité) OK
- ✅ Write intensif OK
- ✅ Fork+exec OK

---

## 🚀 EXÉCUTION

### Jour 4 (Redéfini)

**Matin (4h)**:
1. Intégrer CoW dans sys_fork()
2. Implémenter capture_parent_context()
3. Tests compilation

**Après-midi (4h)**:
4. Créer test_cow_fork.c
5. Tester dans QEMU
6. Fixer bugs

### Jour 5

**Matin (4h)**:
1. Ajouter métriques
2. Tests stress
3. Optimisations

**Après-midi (2h)**:
4. Documentation
5. Commit final

---

## 📝 LIVRABLE FINAL

Quand CoW est 100%:

```
✅ Code intégré dans sys_fork()
✅ 4 tests QEMU réels passent
✅ Métriques <1500 cycles
✅ 0 memory leak
✅ Stress tests OK
✅ Documentation complète
✅ Commit "CoW 100% COMPLET"
```

**Ensuite seulement** → Passer à exec() VFS

---

**Priorité**: FINIR CoW à 100% AVANT exec()  
**Philosophie**: Chaque module 100% testé avec métriques réelles
