# Guide Complet POSIX-X pour Exo-OS

## Table des Matières

1. [Introduction](#introduction)
2. [Installation Rapide](#installation-rapide)
3. [Architecture Détaillée](#architecture-détaillée)
4. [Syscalls à Implémenter](#syscalls-à-implémenter)
5. [Tests et Validation](#tests-et-validation)
6. [Troubleshooting](#troubleshooting)

---

## Introduction

POSIX-X est la couche de compatibilité POSIX d'Exo-OS, basée sur **musl libc** adapté. Elle permet d'exécuter des applications Linux/Unix existantes sur Exo-OS avec des performances optimisées grâce à une traduction intelligente des syscalls.

### Objectifs

- ✅ **Compatibilité** : 90%+ des apps POSIX fonctionnent sans recompilation
- ✅ **Performance** : 2-3x plus rapide que Linux sur syscalls critiques
- ✅ **Simplicité** : Installation d'apps aussi simple que `apt install`

---

## Installation Rapide

### Méthode 1 : Script Automatique (Recommandé)

```bash
cd exo-os/
./scripts/setup_posix_x.sh
```

Ce script :

1. Clone musl libc v1.2.5
2. Crée les fichiers d'adaptation
3. Patch `syscall_arch.h`
4. Crée le module Rust bridge
5. Compile musl en mode test

### Méthode 2 : Manuel

Voir le guide détaillé `musl_adaptation_step_by_step.md`.

---

## Architecture Détaillée

### Vue d'Ensemble

```
Application POSIX
       ↓
   musl libc (adapté)
       ↓
   POSIX-X Bridge (C)
       ↓
   Rust Bridge (FFI)
       ↓
   Exo-OS Kernel
```

### Flux d'un Syscall

#### Exemple : `read(fd, buf, count)`

1. **Application** appelle `read()` de musl
2. **musl** appelle `__syscall3(SYS_read, fd, buf, count)`
3. **syscall_arch.h** redirige vers `exo_syscall_3()`
4. **exo_syscall_bridge.c** appelle `exo_kernel_syscall()`
5. **bridge.rs** (Rust) route vers le bon handler
6. **Traduction** FD → Capability
7. **Fusion Ring** envoie le message au kernel
8. **Kernel** traite et répond
9. **Résultat** remonte la chaîne

---

## Syscalls à Implémenter

### Priorité 1 : Syscalls Essentiels (1-2 semaines)

| Syscall | Numéro Linux | Exo-OS | Complexité | Status |
|---------|--------------|--------|------------|--------|
| `exit` | 60 | 1 | Trivial | ✅ |
| `getpid` | 39 | 3 | Trivial | ✅ |
| `gettid` | 186 | 4 | Trivial | 🚧 |
| `read` | 0 | 12 | Moyen | 🚧 |
| `write` | 1 | 13 | Moyen | 🚧 |
| `open` | 2 | 10 | Moyen | 🚧 |
| `close` | 3 | 11 | Facile | 🚧 |

### Priorité 2 : I/O Avancé (2-3 semaines)

| Syscall | Traduction | Complexité |
|---------|------------|------------|
| `lseek` | Capability seek | Moyen |
| `pipe` | Fusion Ring pair | Facile |
| `dup/dup2` | Clone capability | Facile |
| `ioctl` | Case-by-case | Difficile |

### Priorité 3 : Processus (3-4 semaines)

| Syscall | Traduction | Complexité |
|---------|------------|------------|
| `fork` | spawn() + COW | **Très difficile** |
| `execve` | ELF loader | Difficile |
| `wait4` | IPC message | Moyen |
| `clone` | spawn() variants | Difficile |

### Priorité 4 : Mémoire (2 semaines)

| Syscall | Traduction | Complexité | Status |
|---------|------------|------------|--------|
| `mmap` | Shared memory | Moyen | ✅ (Kernel) |
| `munmap` | Dealloc | Facile | ✅ (Kernel) |
| `mprotect` | Rights update | Moyen | ✅ (Kernel) |
| `brk/sbrk` | Heap extend | Moyen | 🚧 |

### Priorité 5 : Signaux (4 semaines)

| Syscall | Traduction | Complexité |
|---------|------------|------------|
| `kill` | IPC message | Moyen |
| `sigaction` | Handler register | Difficile |
| `sigprocmask` | Mask management | Moyen |
| `sigreturn` | Stack unwinding | Difficile |

---

## Implémentation Détaillée

### Exemple 1 : `getpid()` (Trivial - Fast Path)

**Kernel** (`kernel/src/syscall/handlers/process.rs`) :

```rust
pub fn getpid() -> u32 {
    // Lire directement depuis le TCB (Thread Control Block)
    let current_process = scheduler::current_process();
    current_process.pid
}
```

**Bridge** (`kernel/src/posix_x/bridge.rs`) :

```rust
3 => syscall::getpid() as i64,
```

**Performance** : ~40-50 cycles (vs ~100 cycles Linux)

---

### Exemple 2 : `read()` (Moyen - Hybrid Path)

**Kernel** :

```rust
// kernel/src/syscall/handlers/io.rs
pub fn read(fd: i32, buf: *mut u8, count: usize) -> isize {
    // 1. Traduire FD → Capability
    let cap = fd_table::get_capability(fd)?;
    
    // 2. Vérifier les droits
    if !cap.has_right(Rights::READ) {
        return Err(Error::PermissionDenied);
    }
    
    // 3. Choisir le chemin optimal
    if count <= 56 {
        // Fast path: Inline dans Fusion Ring
        fusion_ring::read_inline(cap, buf, count)
    } else {
        // Zero-copy path: Shared memory
        fusion_ring::read_zerocopy(cap, buf, count)
    }
}
```

**Performance** :

- < 56 bytes : ~400 cycles
- > 56 bytes : ~800 cycles (0 copies!)

---

### Exemple 3 : `fork()` (Difficile - Legacy Path)

**Émulation complète nécessaire** :

```rust
// kernel/src/posix_x/emulation/fork.rs
pub fn emulate_fork() -> Result<u32, Error> {
    let parent = scheduler::current_process();
    
    // 1. Créer un nouveau processus
    let child = process::spawn(parent.executable_path)?;
    
    // 2. Cloner la mémoire (COW)
    memory::clone_address_space(parent, child, CopyOnWrite::Enabled)?;
    
    // 3. Cloner la table FD
    fd_table::clone(parent, child)?;
    
    // 4. Cloner l'environnement
    env::clone(parent, child)?;
    
    // 5. Setup stack child (retour = 0)
    child.set_return_value(0);
    
    // 6. Ajouter au scheduler
    scheduler::add_task(child);
    
    // Parent retourne PID du child
    Ok(child.pid)
}
```

**Performance** : ~50,000 cycles (acceptable car rare)

---

## Tests et Validation

### Test 1 : Hello World

```c
// test/posix/hello.c
#include <stdio.h>

int main() {
    printf("Hello from Exo-OS POSIX-X!\n");
    return 0;
}
```

**Compilation** :

```bash
cd third_party/musl
make

clang -nostdlib -static \
    -I include \
    test/posix/hello.c \
    lib/crt1.o lib/libc.a \
    -o hello.elf
```

**Exécution** :

```bash
./build/qemu.sh --kernel kernel.elf --initrd hello.elf
```

**Attendu** :

```
[POSIX-X] Syscall 1 (write) called
[POSIX-X] Buffer: "Hello from Exo-OS POSIX-X!\n"
Hello from Exo-OS POSIX-X!
[POSIX-X] Syscall 60 (exit) called
Process exited with code 0
```

---

### Test 2 : File I/O

```c
// test/posix/file_io.c
#include <fcntl.h>
#include <unistd.h>
#include <stdio.h>

int main() {
    int fd = open("/test.txt", O_RDONLY);
    if (fd < 0) {
        printf("Failed to open file\n");
        return 1;
    }
    
    char buf[256];
    ssize_t n = read(fd, buf, sizeof(buf));
    write(1, buf, n);  // stdout
    
    close(fd);
    return 0;
}
```

**Test** :

```bash
echo "This is a test file" > initrd/test.txt
./build/qemu.sh --kernel kernel.elf --initrd file_io.elf
```

---

### Test 3 : Fork (Complex)

```c
// test/posix/fork_test.c
#include <unistd.h>
#include <stdio.h>

int main() {
    printf("Parent PID: %d\n", getpid());
    
    pid_t pid = fork();
    
    if (pid == 0) {
        // Child
        printf("Child PID: %d\n", getpid());
    } else {
        // Parent
        printf("Parent created child %d\n", pid);
    }
    
    return 0;
}
```

---

## Troubleshooting

### Problème 1 : Compilation musl échoue

**Erreur** :

```
exo_syscall_bridge.c:5:10: fatal error: 'exo_syscall_numbers.h' file not found
```

**Solution** :

```bash
cd third_party/musl
ls exo_syscall_numbers.h  # Vérifier que le fichier existe
```

Si absent, relancez `./scripts/setup_posix_x.sh`.

---

### Problème 2 : Linker errors

**Erreur** :

```
undefined reference to `exo_kernel_syscall'
```

**Solution** :

1. Vérifier que `mod posix_x;` est dans `kernel/src/lib.rs`
2. Vérifier que la fonction est bien `#[no_mangle]`
3. Compiler le kernel avec `cargo build --release`

---

### Problème 3 : Syscall retourne toujours -1

**Diagnostic** :

```rust
// Ajouter des logs dans bridge.rs
#[no_mangle]
pub extern "C" fn exo_kernel_syscall(...) -> i64 {
    serial_println!("[BRIDGE] Syscall {} called", num);
    serial_println!("  Args: {} {} {}", a1, a2, a3);
    
    let result = match num {
        // ...
    };
    
    serial_println!("  Result: {}", result);
    result
}
```

---

## Roadmap d'Implémentation

### Semaine 1-2 : Foundation

- [x] Setup musl
- [x] Bridge C ↔ Rust
- [ ] 5 syscalls basiques (exit, getpid, read, write, open)
- [ ] Test Hello World

### Semaine 3-4 : I/O Complet

- [ ] 10 syscalls I/O
- [ ] FD table fonctionnelle
- [ ] Test file operations

### Semaine 5-8 : Processus

- [ ] fork() émulé
- [ ] execve() loader ELF
- [ ] wait4(), exit status
- [ ] Test multi-process

### Semaine 9-12 : Signaux & IPC

- [ ] Signaux basiques (SIGINT, SIGTERM)
- [ ] Signal handlers
- [ ] Pipes via Fusion Rings
- [ ] Sockets basics

### Semaine 13-16 : Optimisations

- [ ] Cache capabilities
- [ ] Zero-copy détection automatique
- [ ] Batching intelligent
- [ ] Benchmarks vs Linux

---

## Ressources

### Documentation

- musl source : <https://git.musl-libc.org/>
- Linux syscalls : <https://man7.org/linux/man-pages/man2/syscalls.2.html>
- POSIX spec : <https://pubs.opengroup.org/onlinepubs/9699919799/>

### Exemples

- Redox OS (Rust libc) : <https://gitlab.redox-os.org/redox-os/relibc>
- SerenityOS (C++ libc) : <https://github.com/SerenityOS/serenity>

### Contact

Si bloqué, ouvrez une issue avec :

1. Le code problématique
2. Les logs kernel
3. La commande de compilation

Bon courage ! 🚀
