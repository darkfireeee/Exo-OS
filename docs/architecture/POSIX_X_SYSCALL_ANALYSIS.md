# 📊 ANALYSE POSIX-X : Syscalls Implémentés vs Requis

**Date:** 3 décembre 2025  
**Objectif:** Identifier tous les syscalls nécessaires pour v1.0.0

---

## 📋 ÉTAT ACTUEL DES SYSCALLS

### ✅ IMPLÉMENTÉS (Fonctionnels)

| Syscall | Fichier | Status | Notes |
|---------|---------|--------|-------|
| `read` | `hybrid_path/io.rs` | ✅ | VFS intégré |
| `write` | `hybrid_path/io.rs` | ✅ | VFS intégré |
| `open` | `hybrid_path/io.rs` | ✅ | VFS intégré |
| `close` | `hybrid_path/io.rs` | ✅ | VFS intégré |
| `lseek` | `hybrid_path/io.rs` | ✅ | Fonctionnel |
| `getpid` | `fast_path/info.rs` | ✅ | Simple |
| `getppid` | `fast_path/info.rs` | ✅ | Simple |
| `gettid` | `fast_path/info.rs` | ✅ | Simple |
| `getuid` | `fast_path/info.rs` | ✅ | Stub (retourne 0) |
| `getgid` | `fast_path/info.rs` | ✅ | Stub (retourne 0) |
| `clock_gettime` | `fast_path/time.rs` | 🟡 | Partiel |

### 🟡 STUBS (Retournent valeur fixe ou ENOSYS)

| Syscall | Fichier | Retourne | Priorité |
|---------|---------|----------|----------|
| `fsync` | `hybrid_path/io.rs` | 0 | P2 |
| `fdatasync` | `hybrid_path/io.rs` | 0 | P2 |
| `ioctl` | `hybrid_path/io.rs` | ENOTTY | P2 |
| `getpriority` | `fast_path/process.rs` | 0 | P3 |
| `setpriority` | `fast_path/process.rs` | 0 | P3 |
| `nanosleep` | `fast_path/time.rs` | 0 | P1 |
| `fork` | `legacy_path/fork.rs` | ENOSYS | **P0** |
| `vfork` | `legacy_path/fork.rs` | ENOSYS | P1 |
| `clone` | `legacy_path/fork.rs` | ENOSYS | P1 |
| `execve` | `legacy_path/exec.rs` | ENOSYS | **P0** |
| `execveat` | `legacy_path/exec.rs` | ENOSYS | P2 |

### ❌ NON IMPLÉMENTÉS (Requis pour v1.0.0)

#### Priorité 0 - Critique (Shell basique)

| Syscall | Linux # | Description | Notes |
|---------|---------|-------------|-------|
| `fork` | 57 | Clone process | CoW requis |
| `execve` | 59 | Load program | ELF loader OK |
| `exit` | 60 | Terminate | + cleanup |
| `wait4` | 61 | Wait child | Zombie handling |
| `pipe` | 22 | Create pipe | IPC basique |
| `dup` | 32 | Duplicate FD | Simple |
| `dup2` | 33 | Dup to specific | Simple |

#### Priorité 1 - Important (Programme complet)

| Syscall | Linux # | Description | Notes |
|---------|---------|-------------|-------|
| `mmap` | 9 | Map memory | Virtual mem |
| `munmap` | 11 | Unmap | Virtual mem |
| `mprotect` | 10 | Change perms | NX bit |
| `brk` | 12 | Heap end | Allocator |
| `rt_sigaction` | 13 | Signal handler | Signals |
| `rt_sigprocmask` | 14 | Signal mask | Signals |
| `rt_sigreturn` | 15 | Return from sig | ASM |
| `kill` | 62 | Send signal | IPC |
| `stat` | 4 | File info | VFS |
| `fstat` | 5 | FD info | VFS |
| `fcntl` | 72 | FD control | Flags |
| `getdents64` | 217 | Read directory | VFS |
| `getcwd` | 79 | Current dir | Process |
| `chdir` | 80 | Change dir | Process |
| `mkdir` | 83 | Create dir | VFS |
| `rmdir` | 84 | Remove dir | VFS |
| `unlink` | 87 | Delete file | VFS |
| `rename` | 82 | Rename | VFS |

#### Priorité 2 - Network + Avancé

| Syscall | Linux # | Description | Notes |
|---------|---------|-------------|-------|
| `socket` | 41 | Create socket | TCP/IP |
| `bind` | 49 | Bind address | TCP/IP |
| `listen` | 50 | Listen | TCP/IP |
| `accept` | 43 | Accept conn | TCP/IP |
| `connect` | 42 | Connect | TCP/IP |
| `sendto` | 44 | Send data | UDP |
| `recvfrom` | 45 | Recv data | UDP |
| `setsockopt` | 54 | Socket opts | TCP/IP |
| `getsockopt` | 55 | Socket opts | TCP/IP |
| `poll` | 7 | I/O multiplexing | Async |
| `select` | 23 | I/O multiplexing | Legacy |
| `epoll_create` | 213 | Epoll | Async |
| `epoll_ctl` | 233 | Epoll control | Async |
| `epoll_wait` | 232 | Epoll wait | Async |

#### Priorité 3 - Completeness

| Syscall | Linux # | Description |
|---------|---------|-------------|
| `access` | 21 | Check permissions |
| `chmod` | 90 | Change mode |
| `chown` | 92 | Change owner |
| `umask` | 95 | Set umask |
| `gettimeofday` | 96 | Get time |
| `getrlimit` | 97 | Resource limits |
| `setrlimit` | 160 | Set limits |
| `getrusage` | 98 | Resource usage |
| `sysinfo` | 99 | System info |
| `times` | 100 | Process times |
| `ptrace` | 101 | Debug |
| `syslog` | 103 | Kernel log |
| `setuid` | 105 | Set UID |
| `setgid` | 106 | Set GID |
| `setsid` | 112 | New session |
| `getpgid` | 121 | Get PGID |
| `setpgid` | 109 | Set PGID |
| `uname` | 63 | System name |
| `pread64` | 17 | Read at offset |
| `pwrite64` | 18 | Write at offset |
| `readv` | 19 | Vectored read |
| `writev` | 20 | Vectored write |
| `truncate` | 76 | Truncate file |
| `ftruncate` | 77 | Truncate FD |
| `symlink` | 88 | Create symlink |
| `readlink` | 89 | Read symlink |
| `link` | 86 | Hard link |
| `flock` | 73 | File lock |
| `futex` | 202 | Fast userspace mutex |
| `clone3` | 435 | New clone |
| `memfd_create` | 319 | Memory FD |

---

## 📊 STATISTIQUES

| Catégorie | Count | Pourcentage |
|-----------|-------|-------------|
| ✅ Implémentés | 11 | ~3% |
| 🟡 Stubs | 11 | ~3% |
| ❌ Manquants P0 | 7 | - |
| ❌ Manquants P1 | 18 | - |
| ❌ Manquants P2 | 15 | - |
| ❌ Manquants P3 | 35+ | - |
| **Total requis v1.0.0** | ~100 | 100% |

**Progression POSIX-X:** ~6% implémenté, ~94% à faire

---

## 🎯 PLAN D'IMPLÉMENTATION

### Sprint 1 (P0 - 2 semaines)
```
fork → execve → exit → wait4 → pipe → dup → dup2
```
**Résultat:** Shell peut lancer des programmes

### Sprint 2 (P1 Memory - 1 semaine)
```
mmap → munmap → mprotect → brk
```
**Résultat:** Programmes peuvent allouer de la mémoire

### Sprint 3 (P1 Signals - 1 semaine)
```
rt_sigaction → rt_sigprocmask → rt_sigreturn → kill
```
**Résultat:** Ctrl+C fonctionne

### Sprint 4 (P1 FS - 1 semaine)
```
stat → fstat → mkdir → rmdir → unlink → rename → getcwd → chdir → getdents64
```
**Résultat:** `ls`, `cd`, `mkdir` fonctionnent

### Sprint 5 (P2 Network - 2 semaines)
```
socket → bind → listen → accept → connect → sendto → recvfrom
```
**Résultat:** Connexion TCP basique

### Sprint 6 (P2 Async - 1 semaine)
```
poll → select → epoll_*
```
**Résultat:** Serveurs asynchrones

### Sprint 7+ (P3 - Ongoing)
Compléter le reste pour compatibilité musl

---

## 📁 STRUCTURE DE FICHIERS RECOMMANDÉE

```
kernel/src/posix_x/syscalls/
├── mod.rs                    # Dispatch principal
│
├── fast_path/               # < 50 cycles
│   ├── mod.rs
│   ├── info.rs              # getpid, gettid, getuid, etc.
│   ├── time.rs              # clock_gettime, gettimeofday
│   └── process.rs           # getpriority, setpriority
│
├── hybrid_path/             # 50-500 cycles
│   ├── mod.rs
│   ├── io.rs                # read, write, open, close (✅)
│   ├── fd.rs                # dup, dup2, fcntl (NEW)
│   ├── pipe.rs              # pipe, pipe2 (NEW)
│   ├── stat.rs              # stat, fstat, lstat
│   ├── dir.rs               # mkdir, rmdir, chdir, getcwd (NEW)
│   ├── memory.rs            # mmap, munmap, mprotect, brk (NEW)
│   ├── signals.rs           # rt_sig* (NEW)
│   └── socket.rs            # socket API (NEW)
│
└── legacy_path/             # > 500 cycles
    ├── mod.rs
    ├── fork.rs              # fork, vfork, clone
    ├── exec.rs              # execve, execveat
    ├── wait.rs              # wait4, waitpid (NEW)
    └── sysv_ipc.rs          # shmget, semget, etc.
```

---

## 🔧 TEMPLATE D'IMPLÉMENTATION

```rust
//! Example: sys_mkdir implementation
//! File: kernel/src/posix_x/syscalls/hybrid_path/dir.rs

use crate::fs::vfs;
use crate::posix_x::translation::errno::Errno;
use core::ffi::CStr;

/// mkdir - Create a directory
/// 
/// # Arguments
/// * `pathname` - Path to create
/// * `mode` - Permission mode (e.g., 0755)
/// 
/// # Returns
/// * 0 on success
/// * -errno on error
pub fn sys_mkdir(pathname: usize, mode: u32) -> i64 {
    // 1. Validate pointer
    if pathname == 0 {
        return -(Errno::EFAULT as i64);
    }
    
    // 2. Read path from userspace
    let path = unsafe {
        match CStr::from_ptr(pathname as *const i8).to_str() {
            Ok(s) => s,
            Err(_) => return -(Errno::EINVAL as i64),
        }
    };
    
    // 3. Call VFS
    match vfs::create_dir(path) {
        Ok(_) => 0,
        Err(crate::fs::FsError::AlreadyExists) => -(Errno::EEXIST as i64),
        Err(crate::fs::FsError::NotFound) => -(Errno::ENOENT as i64),
        Err(crate::fs::FsError::PermissionDenied) => -(Errno::EACCES as i64),
        Err(_) => -(Errno::EIO as i64),
    }
}
```

---

## 📈 MÉTRIQUES DE SUCCÈS

| Jalon | Critère | Test |
|-------|---------|------|
| M1 | Shell lance `/bin/ls` | `fork + execve` |
| M2 | `ls` affiche fichiers | `getdents64 + stat` |
| M3 | `cat file` fonctionne | `open + read + write` |
| M4 | Pipes fonctionnent | `ls \| grep` |
| M5 | Ctrl+C tue process | Signals |
| M6 | Programme C (musl) | Tous P0+P1 |
| M7 | TCP echo server | Network syscalls |
| M8 | musl test suite | 80%+ pass |

---

**🎯 Objectif v1.0.0:** 100+ syscalls, 0 ENOSYS pour cas d'usage courants
