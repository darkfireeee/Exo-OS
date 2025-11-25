# Guide Complet : Adapter musl pour Exo-OS

## Étape 0 : Préparation

### Télécharger musl

```bash
cd exo-os/
mkdir -p third_party
cd third_party

# Cloner musl (version stable)
git clone https://git.musl-libc.org/git/musl
cd musl
git checkout v1.2.5  # Version stable récente

# Créer une branche Exo-OS
git checkout -b exo-os-port
```

### Structure du projet musl

```
musl/
├── arch/
│   └── x86_64/          # Architecture-specific
│       ├── syscall_arch.h   ← À MODIFIER
│       └── bits/
├── src/
│   ├── internal/
│   │   └── syscall.h        ← À MODIFIER
│   ├── stdio/               ← À GARDER
│   ├── string/              ← À GARDER
│   ├── stdlib/              ← malloc à ADAPTER
│   ├── unistd/              ← À WRAPPER
│   ├── signal/              ← À TRADUIRE
│   └── process/             ← fork à ÉMULER
├── include/
└── Makefile
```

---

## Étape 1 : Modifier le Syscall Layer

### Fichier 1 : `arch/x86_64/syscall_arch.h`

**Original** (Linux) :
```c
static __inline long __syscall0(long n)
{
    unsigned long ret;
    __asm__ __volatile__ ("syscall" 
        : "=a"(ret) 
        : "a"(n) 
        : "rcx", "r11", "memory");
    return ret;
}
```

**Modifié** (Exo-OS) :
```c
// arch/x86_64/syscall_arch.h - EXO-OS PORT
#ifndef _SYSCALL_ARCH_H
#define _SYSCALL_ARCH_H

// IMPORTANT: Inclure les headers Exo-OS
#include "exo_syscall_bridge.h"

// Rediriger tous les syscalls vers notre bridge
static __inline long __syscall0(long n)
{
    return exo_syscall_0(n);
}

static __inline long __syscall1(long n, long a1)
{
    return exo_syscall_1(n, a1);
}

static __inline long __syscall2(long n, long a1, long a2)
{
    return exo_syscall_2(n, a1, a2);
}

static __inline long __syscall3(long n, long a1, long a2, long a3)
{
    return exo_syscall_3(n, a1, a2, a3);
}

static __inline long __syscall4(long n, long a1, long a2, long a3, long a4)
{
    return exo_syscall_4(n, a1, a2, a3, a4);
}

static __inline long __syscall5(long n, long a1, long a2, long a3, long a4, long a5)
{
    return exo_syscall_5(n, a1, a2, a3, a4, a5);
}

static __inline long __syscall6(long n, long a1, long a2, long a3, long a4, long a5, long a6)
{
    return exo_syscall_6(n, a1, a2, a3, a4, a5, a6);
}

#define VDSO_USEFUL  // Pas de VDSO sur Exo-OS
#define VDSO_CGT_SYM "__vdso_clock_gettime"
#define VDSO_CGT_VER "LINUX_2.6"
#define VDSO_GETCPU_SYM "__vdso_getcpu"
#define VDSO_GETCPU_VER "LINUX_2.6"

#endif
```

---

### Fichier 2 : Créer `exo_syscall_bridge.c`

```c
// third_party/musl/exo_syscall_bridge.c
//
// PONT ENTRE MUSL ET EXO-OS KERNEL

#include <stdint.h>
#include <errno.h>
#include "exo_syscall_numbers.h"

// Déclarations externes (implémentées en Rust)
extern long exo_kernel_syscall(long num, long a1, long a2, long a3, 
                                long a4, long a5, long a6);

// ============================================================================
// WRAPPERS SYSCALL (appelés par musl)
// ============================================================================

long exo_syscall_0(long n) {
    return exo_kernel_syscall(n, 0, 0, 0, 0, 0, 0);
}

long exo_syscall_1(long n, long a1) {
    return exo_kernel_syscall(n, a1, 0, 0, 0, 0, 0);
}

long exo_syscall_2(long n, long a1, long a2) {
    return exo_kernel_syscall(n, a1, a2, 0, 0, 0, 0);
}

long exo_syscall_3(long n, long a1, long a2, long a3) {
    return exo_kernel_syscall(n, a1, a2, a3, 0, 0, 0);
}

long exo_syscall_4(long n, long a1, long a2, long a3, long a4) {
    return exo_kernel_syscall(n, a1, a2, a3, a4, 0, 0);
}

long exo_syscall_5(long n, long a1, long a2, long a3, long a4, long a5) {
    return exo_kernel_syscall(n, a1, a2, a3, a4, a5, 0);
}

long exo_syscall_6(long n, long a1, long a2, long a3, long a4, long a5, long a6) {
    return exo_kernel_syscall(n, a1, a2, a3, a4, a5, a6);
}

// ============================================================================
// TRADUCTION ERRNO (Linux → Exo-OS)
// ============================================================================

int exo_translate_errno(long exo_error) {
    // TODO: Mapper les codes d'erreur Exo-OS vers POSIX errno
    // Pour l'instant, retourne tel quel
    return (int)exo_error;
}
```

---

### Fichier 3 : Headers `exo_syscall_numbers.h`

```c
// third_party/musl/exo_syscall_numbers.h
//
// NUMÉROS DE SYSCALLS EXO-OS

#ifndef _EXO_SYSCALL_NUMBERS_H
#define _EXO_SYSCALL_NUMBERS_H

// ============================================================================
// SYSCALLS EXO-OS (à synchroniser avec kernel/src/syscall/numbers.rs)
// ============================================================================

// Process management
#define SYS_exo_exit       1
#define SYS_exo_spawn      2
#define SYS_exo_getpid     3
#define SYS_exo_gettid     4

// I/O operations (via Fusion Rings)
#define SYS_exo_open       10
#define SYS_exo_close      11
#define SYS_exo_read       12
#define SYS_exo_write      13

// Memory management
#define SYS_exo_mmap       20
#define SYS_exo_munmap     21

// IPC
#define SYS_exo_send_msg   30
#define SYS_exo_recv_msg   31

// Time
#define SYS_exo_clock_gettime  40

// ============================================================================
// MAPPING LINUX → EXO-OS
// ============================================================================

// Certains syscalls Linux peuvent être mappés directement
#define SYS_exit        SYS_exo_exit
#define SYS_getpid      SYS_exo_getpid
#define SYS_close       SYS_exo_close

// D'autres nécessitent une émulation (voir ci-dessous)
#define SYS_fork        -1  // Pas de support natif, émulé
#define SYS_read        SYS_exo_read
#define SYS_write       SYS_exo_write

#endif
```

---

## Étape 2 : Implémenter le Bridge en Rust

### Fichier : `kernel/src/posix_x/bridge.rs`

```rust
// kernel/src/posix_x/bridge.rs
//
// PONT C → RUST POUR SYSCALLS

use crate::syscall;

/// Fonction appelée par le code C de musl
#[no_mangle]
pub extern "C" fn exo_kernel_syscall(
    num: i64,
    a1: u64, a2: u64, a3: u64,
    a4: u64, a5: u64, a6: u64,
) -> i64 {
    // Dispatcher selon le numéro de syscall
    match num {
        1 => syscall::exit(a1 as i32),
        3 => syscall::getpid() as i64,
        4 => syscall::gettid() as i64,
        
        10 => syscall::open(
            a1 as *const u8,  // path
            a2 as i32,        // flags
            a3 as u32,        // mode
        ),
        
        11 => syscall::close(a1 as i32),
        
        12 => syscall::read(
            a1 as i32,           // fd
            a2 as *mut u8,       // buf
            a3 as usize,         // count
        ),
        
        13 => syscall::write(
            a1 as i32,
            a2 as *const u8,
            a3 as usize,
        ),
        
        40 => syscall::clock_gettime(
            a1 as i32,           // clockid
            a2 as *mut TimeSpec,
        ),
        
        // Fork est émulé (pas de syscall natif)
        -1 => emulate_fork(),
        
        _ => {
            serial_println!("[POSIX-X] Unknown syscall: {}", num);
            -1  // ENOSYS
        }
    }
}

/// Émulation de fork() (complexe!)
fn emulate_fork() -> i64 {
    // TODO: Implémenter
    // 1. Cloner l'espace mémoire (COW)
    // 2. Cloner la table FD
    // 3. Cloner l'environnement
    // 4. Retourner 0 dans le child, PID dans le parent
    
    -1  // Pas encore implémenté
}
```

---

## Étape 3 : Adapter les Fonctions Critiques

### `src/unistd/read.c` - Exemple de wrapper

**Original musl** :
```c
ssize_t read(int fd, void *buf, size_t count)
{
    return syscall_cp(SYS_read, fd, buf, count);
}
```

**Avec POSIX-X** (ajouter traduction FD → Capability) :
```c
ssize_t read(int fd, void *buf, size_t count)
{
    // POSIX-X: Traduire FD → Capability
    int cap_fd = posix_x_translate_fd(fd);
    if (cap_fd < 0) {
        errno = EBADF;
        return -1;
    }
    
    // Appeler le syscall Exo-OS via Fusion Ring
    return syscall_cp(SYS_exo_read, cap_fd, buf, count);
}
```

### Créer `posix_x_helpers.c`

```c
// third_party/musl/src/posix_x/helpers.c
//
// HELPERS POSIX-X

#include <errno.h>

// Table FD globale (thread-local futur)
static int fd_to_capability[1024];

int posix_x_translate_fd(int fd) {
    if (fd < 0 || fd >= 1024) return -1;
    
    int cap = fd_to_capability[fd];
    if (cap == 0) return -1;  // Pas de capability
    
    return cap;
}

void posix_x_register_fd(int fd, int capability) {
    if (fd >= 0 && fd < 1024) {
        fd_to_capability[fd] = capability;
    }
}

void posix_x_close_fd(int fd) {
    if (fd >= 0 && fd < 1024) {
        fd_to_capability[fd] = 0;
    }
}
```

---

## Étape 4 : Configuration du Build

### Créer `configure.exo-os`

```bash
#!/bin/bash
# Configuration pour compiler musl pour Exo-OS

./configure \
    --target=x86_64-exo-os \
    --prefix=/opt/exo-os/usr \
    --disable-shared \
    --enable-static \
    CC=clang \
    CFLAGS="-O2 -fno-stack-protector -I../../../kernel/include"
```

### Modifier `Makefile` (ajouter nos fichiers)

```makefile
# Ajouter après les sources existantes
SRCS += exo_syscall_bridge.c
SRCS += src/posix_x/helpers.c
```

---

## Étape 5 : Tester Progressivement

### Test 1 : Hello World (minimal)

```c
// test_hello.c
#include <stdio.h>

int main() {
    printf("Hello from Exo-OS!\n");
    return 0;
}
```

**Compilation** :
```bash
cd third_party/musl
./configure.exo-os
make

# Compiler le test
clang -nostdlib -static \
    -I include \
    -L lib \
    test_hello.c \
    lib/crt1.o lib/libc.a \
    -o test_hello.elf
```

### Test 2 : Syscall Simple (getpid)

```c
// test_syscall.c
#include <unistd.h>
#include <stdio.h>

int main() {
    int pid = getpid();
    printf("My PID: %d\n", pid);
    return 0;
}
```

### Test 3 : I/O (read/write)

```c
// test_io.c
#include <unistd.h>
#include <fcntl.h>

int main() {
    char buf[100];
    int fd = open("/test.txt", O_RDONLY);
    if (fd < 0) return 1;
    
    ssize_t n = read(fd, buf, 100);
    write(1, buf, n);  // stdout
    
    close(fd);
    return 0;
}
```

---

## Étape 6 : Intégration dans Exo-OS

### Structure finale

```
exo-os/
├── kernel/
│   └── src/
│       └── posix_x/
│           ├── mod.rs          # Module principal
│           ├── bridge.rs       # C ↔ Rust bridge
│           ├── translator.rs   # FD → Cap, etc.
│           └── emulation.rs    # fork, exec, etc.
│
├── third_party/
│   └── musl/                   # musl adapté
│       ├── exo_syscall_bridge.c
│       ├── exo_syscall_numbers.h
│       └── src/posix_x/helpers.c
│
└── userland/
    ├── libc/                   # Lien symbolique vers musl
    └── apps/
        └── test_hello/         # Apps de test
```

---

## Checklist de Migration

### Phase 1 : Boot Minimal
- [ ] Télécharger musl 1.2.5
- [ ] Créer branche `exo-os-port`
- [ ] Modifier `syscall_arch.h`
- [ ] Créer `exo_syscall_bridge.c`
- [ ] Implémenter `exo_kernel_syscall()` en Rust
- [ ] Compiler musl (static)
- [ ] Test : `printf("Hello")`

### Phase 2 : Syscalls Basiques
- [ ] Implémenter `getpid()` / `gettid()`
- [ ] Test : Afficher PID
- [ ] Implémenter `exit()`
- [ ] Test : Exit propre

### Phase 3 : I/O
- [ ] Créer table FD → Capabilities
- [ ] Implémenter `open()` / `close()`
- [ ] Implémenter `read()` / `write()`
- [ ] Test : Lire/écrire fichier

### Phase 4 : Émulations Complexes
- [ ] Émuler `fork()` (COW + clone)
- [ ] Émuler `execve()` (loader ELF)
- [ ] Implémenter signaux → IPC
- [ ] Test : App multi-processus

---

## Ressources

### Liens Utiles
- **musl source** : https://git.musl-libc.org/cgit/musl/
- **musl doc** : https://wiki.musl-libc.org/
- **syscall numbers Linux** : https://filippo.io/linux-syscall-table/

### Exemples de Ports
- **Redox OS** : https://gitlab.redox-os.org/redox-os/relibc (Rust libc)
- **SerenityOS** : LibC custom, bon exemple d'architecture

### Support
Si bloqué, partagez :
1. Erreur de compilation
2. Code modifié
3. Tests échoués

