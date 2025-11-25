# Architecture POSIX-X pour Exo-OS

## Vue d'Ensemble

```
┌─────────────────────────────────────────────────────────────┐
│                    APPLICATION POSIX                         │
│  (Code écrit pour Linux/Unix standard)                      │
└──────────────────────┬──────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────────┐
│                   POSIX-X LAYER                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐     │
│  │  Fast Path   │  │ Hybrid Path  │  │ Legacy Path  │     │
│  │  < 50 cycles │  │ 400-1000 c.  │  │ > 50k cycles │     │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘     │
│         │                  │                  │              │
│         ▼                  ▼                  ▼              │
│  ┌──────────────────────────────────────────────────┐      │
│  │        SYSCALL TRANSLATOR                         │      │
│  │  • FD → Capabilities                              │      │
│  │  • Signals → IPC Messages                         │      │
│  │  • fork() → spawn() + COW                         │      │
│  └──────────────────────────────────────────────────┘      │
└──────────────────────┬──────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────────┐
│                  MUSL LIBC ADAPTÉE                           │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐     │
│  │   stdio.c    │  │  string.c    │  │  stdlib.c    │     │
│  │   (printf)   │  │  (memcpy)    │  │  (malloc)    │     │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘     │
│         │                  │                  │              │
│         └──────────────────┴──────────────────┘              │
│                            │                                 │
│                            ▼                                 │
│              ┌────────────────────────┐                      │
│              │   SYSCALL WRAPPERS     │                      │
│              │   (syscall.c modifié)  │                      │
│              └────────────────────────┘                      │
└──────────────────────┬──────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────────┐
│                  EXO-OS KERNEL                               │
│  • Fusion Rings IPC                                          │
│  • Capabilities                                              │
│  • Native syscalls                                           │
└─────────────────────────────────────────────────────────────┘
```

## Niveaux d'Adaptation

### Niveau 1 : Syscalls Wrapper (CRITIQUE)
Modifier `musl/src/internal/syscall.h` et `syscall_arch.h`

### Niveau 2 : Libc Core (80% inchangé)
stdio, string, stdlib → Utilisés tels quels

### Niveau 3 : POSIX-X Shims (Nouveau code)
Traduction intelligente POSIX → Exo-OS primitives

---

## Composants à Modifier

| Fichier musl | Action | Raison |
|--------------|--------|--------|
| `src/internal/syscall.h` | **MODIFIER** | Rediriger vers Exo syscalls |
| `arch/x86_64/syscall_arch.h` | **REMPLACER** | Utiliser Fusion Rings |
| `src/unistd/*.c` | **WRAPPER** | FD → Capabilities |
| `src/process/fork.c` | **ÉMULER** | fork() → spawn() + COW |
| `src/signal/*.c` | **TRADUIRE** | Signals → IPC messages |
| `src/mman/mmap.c` | **ADAPTER** | mmap → Shared memory |
| `src/stdio/*.c` | **GARDER** | Fonctionne tel quel |
| `src/string/*.c` | **GARDER** | Fonctionne tel quel |
| `src/stdlib/malloc.c` | **REDIRIGER** | malloc → Exo allocator |

---

## Architecture en 3 Couches

### Couche 1 : Fast Path (< 50 cycles)
```
Application           musl                POSIX-X           Exo-OS
   ───────────────────────────────────────────────────────────
   getpid()  ────>  __syscall1()  ────>  fast_getpid() ────> Direct read TLS
   gettid()                                                    (pas de syscall!)
```

### Couche 2 : Hybrid Path (400-1000 cycles)
```
Application           musl                POSIX-X               Exo-OS
   ─────────────────────────────────────────────────────────────────
   read(fd)  ────>  __syscall3()  ────>  translate_fd() ────> Fusion Ring
                                          ├─> cache lookup       message
                                          ├─> capability         (zero-copy)
                                          └─> inline/zerocopy
```

### Couche 3 : Legacy Path (> 50k cycles)
```
Application           musl                POSIX-X               Exo-OS
   ─────────────────────────────────────────────────────────────────
   fork()    ────>  __syscall0()  ────>  emulate_fork() ────> spawn()
                                          ├─> COW memory         + clone FD
                                          ├─> clone FD table     + setup env
                                          └─> complex emulation
```

---

## Prochaines Étapes

1. **Télécharger musl** : `git clone https://git.musl-libc.org/git/musl`
2. **Créer branch Exo-OS** : `git checkout -b exo-os-port`
3. **Modifier syscall layer** : Voir guide détaillé ci-dessous
4. **Compiler** : `./configure --target=x86_64-exo-os`
5. **Tester** : Commencer avec `printf("Hello")` seulement

---

## Files d'Attente de Modification (par priorité)

### Phase 1 : Boot Minimal
1. `syscall_arch.h` - Redirection syscalls
2. `__syscall_cp.c` - Cancellation points
3. `malloc.c` - Allocator redirect

### Phase 2 : I/O Basique
4. `open.c`, `close.c`, `read.c`, `write.c`
5. `stdio/*.c` - printf family

### Phase 3 : Processus
6. `fork.c` - Émulation
7. `execve.c` - Loader
8. `pthread/*.c` - Threads

### Phase 4 : Signaux & IPC
9. `signal/*.c` - Traduction IPC
10. `socket/*.c` - Network via IPC

