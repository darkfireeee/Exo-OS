# SPEC-EXO-LIBC-STRATA — Couche de Compatibilité POSIX
## musl-exo · 127 Syscalls Prioritaires · Sandbox POSIX

**Auteur :** claude-alpha
**Date :** 2026-05-26
**Statut :** RÉFÉRENCE — remplace SPEC-EXO-LIBC.md

---

## 1. Rôle et Périmètre

`musl-exo` est le fork de musl qui traduit les appels POSIX en IPC ExoOS.
Il permet à un programme compilé pour Linux de s'exécuter sur ExoOS **via `exo compat`**, sans recompilation.

**Ce que musl-exo N'EST PAS :**
- Un kernel Linux (pas de simulation, redirection vers Ring1 ExoOS)
- Une implémentation complète des 241 syscalls Linux (cible : 127 pour Strata)
- La couche d'abstraction principale ExoOS (c'est la couche compat uniquement)

**Ce qui ne passe JAMAIS par musl-exo :**
- Les syscalls des apps natives ExoOS (elles utilisent l'IPC ExoOS directement)
- Les services Ring1 (ils n'ont pas de libc)

---

## 2. Traitement Spécial des Syscalls POSIX Linux dans ExoOS

### Permissions POSIX → Capabilities ExoOS

ExoOS n'a ni `uid`, ni `gid`, ni `rwx` au sens POSIX. musl-exo gère ces appels comme suit :

```rust
// chmod, fchmod, chown, fchown, lchown → No-op sécurisé
// Retourne 0 (succès) + entrée ExoLedger
fn sys_chmod(_path: &str, _mode: u32) -> i64 {
    exoledger_log(Event::PosixNoOp { syscall: "chmod", pid: current_pid() });
    0 // Success sans effet réel
}

// getuid, getgid, geteuid, getegid → Retourne 0 (root simulé dans sandbox)
fn sys_getuid() -> u32 { 0 }
fn sys_getgid() -> u32 { 0 }

// setuid, setgid → No-op + log (pas d'élévation possible dans sandbox)
fn sys_setuid(_uid: u32) -> i64 {
    exoledger_log(Event::PosixSetuidAttempt { pid: current_pid() });
    0
}
```

---

## 3. Tableau Complet des 127 Syscalls Prioritaires

### Priorité 1 — BLOQUANT (sans eux, rien ne s'exécute)

| Syscall | N° Linux | Implémentation ExoOS | Strata |
|---|---|---|---|
| `read` | 0 | IPC → vfs_server::read_at | v0.2.0 |
| `write` | 1 | IPC → vfs_server::write_at | v0.2.0 |
| `open` | 2 | IPC → vfs_server::open | v0.2.0 |
| `close` | 3 | IPC → vfs_server::close + révocation cap | v0.2.0 |
| `stat` | 4 | IPC → vfs_server::stat | v0.2.0 |
| `fstat` | 5 | IPC → vfs_server::fstat | v0.2.0 |
| `lstat` | 6 | IPC → vfs_server::stat (symlink target) | v0.2.0 |
| `poll` | 7 | IPC poll multi-caps | v0.2.0 |
| `lseek` | 8 | Curseur interne + read_at | v0.2.0 |
| `mmap` | 9 | SYS_MMAP kernel direct | v0.2.0 |
| `mprotect` | 10 | SYS_MPROTECT kernel direct | v0.2.0 |
| `munmap` | 11 | SYS_MUNMAP kernel direct | v0.2.0 |
| `brk` | 12 | Émulé via mmap (ExoOS n'a pas brk) | v0.2.0 |
| `rt_sigaction` | 13 | kernel/process/signal/ | v0.2.0 |
| `rt_sigprocmask` | 14 | kernel/process/signal/ | v0.2.0 |
| `ioctl` | 16 | Partiel : TIOCGWINSZ, TCGETS, TCSETS | v0.2.0 |
| `pread64` | 17 | IPC → vfs_server::read_at | v0.2.0 |
| `pwrite64` | 18 | IPC → vfs_server::write_at | v0.2.0 |
| `readv` | 19 | vfs_server (scatter) | v0.2.0 |
| `writev` | 20 | vfs_server (gather) | v0.2.0 |
| `pipe` | 22 | IPC → channel pair SpscRing | v0.2.0 |
| `select` | 23 | IPC poll multi-caps | v0.2.0 |
| `mremap` | 25 | SYS_MREMAP kernel direct | v0.2.0 |
| `msync` | 26 | IPC → vfs_server::sync | v0.2.0 |
| `dup` | 32 | Duplication CapToken | v0.2.0 |
| `dup2` | 33 | Duplication CapToken avec fd cible | v0.2.0 |
| `nanosleep` | 35 | SYS_NANOSLEEP kernel | v0.2.0 |
| `getpid` | 39 | kernel/process/core | v0.2.0 |
| `fork` | 57 | kernel/process/lifecycle/fork | v0.2.0 |
| `execve` | 59 | kernel/process/lifecycle/exec | v0.2.0 |
| `exit` | 60 | kernel/process/lifecycle/exit | v0.2.0 |
| `wait4` | 61 | kernel/process/lifecycle/wait | v0.2.0 |
| `kill` | 62 | kernel/process/signal/delivery | v0.2.0 |
| `uname` | 63 | Retourne "ExoOS" + version Strata | v0.2.0 |
| `getcwd` | 79 | IPC → vfs_server::getcwd | v0.2.0 |
| `chdir` | 80 | IPC → vfs_server::chdir | v0.2.0 |
| `getdents64` | 217 | IPC → vfs_server::readdir | v0.2.0 |
| `clock_gettime` | 228 | IPC → ktime ou TSC direct | v0.2.0 |
| `exit_group` | 231 | Termine tous les threads du groupe | v0.2.0 |

### Priorité 2 — Important (apps text-mode)

| Syscall | N° | Implémentation | Strata |
|---|---|---|---|
| `socket` | 41 | IPC → network_server::socket | v0.2.0 |
| `connect` | 42 | IPC → network_server::connect | v0.2.0 |
| `accept` | 43 | IPC → network_server::accept | v0.2.0 |
| `sendto` | 44 | IPC → network_server | v0.2.0 |
| `recvfrom` | 45 | IPC → network_server | v0.2.0 |
| `bind` | 49 | IPC → network_server::bind | v0.2.0 |
| `listen` | 50 | IPC → network_server::listen | v0.2.0 |
| `getsockname` | 51 | IPC → network_server | v0.2.0 |
| `getpeername` | 52 | IPC → network_server | v0.2.0 |
| `setsockopt` | 54 | IPC → network_server (partiel) | v0.2.0 |
| `getsockopt` | 55 | IPC → network_server (partiel) | v0.2.0 |
| `clone` | 56 | kernel/process + CLONE_THREAD/VM/FS | v0.2.0 |
| `fcntl` | 72 | Partiel : F_GETFL, F_SETFL, O_NONBLOCK | v0.2.0 |
| `rename` | 82 | IPC → vfs_server::rename O(1) | v0.2.0 |
| `mkdir` | 83 | IPC → vfs_server::mkdir | v0.2.0 |
| `rmdir` | 84 | IPC → vfs_server::rmdir | v0.2.0 |
| `link` | 86 | IPC → vfs_server (relation ExoFS) | v0.2.0 |
| `unlink` | 87 | IPC → vfs_server::unlink | v0.2.0 |
| `symlink` | 88 | IPC → vfs_server (relation typée) | v0.2.0 |
| `chmod` | 90 | No-op + ExoLedger | v0.2.0 |
| `fchmod` | 91 | No-op + ExoLedger | v0.2.0 |
| `chown` | 92 | No-op + ExoLedger | v0.2.0 |
| `getuid` | 102 | Retourne 0 | v0.2.0 |
| `getgid` | 104 | Retourne 0 | v0.2.0 |
| `setuid` | 105 | No-op + ExoLedger | v0.2.0 |
| `setgid` | 106 | No-op + ExoLedger | v0.2.0 |
| `geteuid` | 107 | Retourne 0 | v0.2.0 |
| `getegid` | 108 | Retourne 0 | v0.2.0 |
| `epoll_create` | 213 | IPC poll async | v0.2.0 |
| `epoll_ctl` | 233 | IPC poll async | v0.2.0 |
| `epoll_wait` | 232 | IPC poll async | v0.2.0 |
| `eventfd` | 284 | Canal IPC léger | v0.2.0 |
| `timerfd_create` | 283 | Timer IPC | v0.2.0 |
| `timerfd_settime` | 286 | Timer IPC | v0.2.0 |
| `timerfd_gettime` | 287 | Timer IPC | v0.2.0 |
| `futex` | 202 | Partiel : FUTEX_WAIT, FUTEX_WAKE | v0.2.0 |
| `openat` | 257 | IPC → vfs_server::open_at | v0.2.0 |
| `mkdirat` | 258 | IPC → vfs_server::mkdir_at | v0.2.0 |
| `unlinkat` | 263 | IPC → vfs_server::unlink_at | v0.2.0 |
| `renameat` | 264 | IPC → vfs_server::rename_at | v0.2.0 |
| `fstatat` | 262 | IPC → vfs_server::stat_at | v0.2.0 |
| `getppid` | 110 | kernel/process | v0.2.0 |
| `getpgrp` | 111 | kernel/process (group) | v0.2.0 |
| `setsid` | 112 | kernel/process (session) | v0.2.0 |
| `gettimeofday` | 96 | ktime + epoch conversion | v0.2.0 |
| `time` | 201 | ktime → seconds | v0.2.0 |
| `mmap2 (MAP_SHARED)` | 9 | SHM via IPC shared_memory | v0.2.0 |
| `pipe2` | 293 | IPC → channel pair avec flags | v0.2.0 |
| `dup3` | 292 | Duplication CapToken + close-on-exec | v0.2.0 |
| `readlink` | 89 | IPC → vfs_server::readlink | v0.2.0 |
| `readlinkat` | 267 | IPC → vfs_server::readlink_at | v0.2.0 |
| `access` | 21 | IPC → vfs_server::access (caps check) | v0.2.0 |
| `faccessat` | 269 | IPC → vfs_server::access_at | v0.2.0 |
| `truncate` | 76 | IPC → vfs_server::truncate | v0.2.0 |
| `ftruncate` | 77 | IPC → vfs_server::ftruncate | v0.2.0 |
| `sendmsg` | 46 | IPC → network_server | v0.2.0 |
| `recvmsg` | 47 | IPC → network_server | v0.2.0 |
| `shutdown` | 48 | IPC → network_server::shutdown | v0.2.0 |
| `socketpair` | 53 | IPC → deux SpscRing reliés | v0.2.0 |
| `getrusage` | 98 | Métriques process depuis scheduler_server | v0.2.0 |
| `sysinfo` | 99 | Métriques système depuis monitor_server | v0.2.0 |
| `times` | 100 | Temps process depuis scheduler_server | v0.2.0 |
| `umask` | 95 | No-op (pas de rwx dans ExoOS) | v0.2.0 |
| `sigreturn` | 15 | kernel/process/signal | v0.2.0 |
| `rt_sigreturn` | 15 | kernel/process/signal | v0.2.0 |
| `rt_sigtimedwait` | 128 | kernel/process/signal | v0.2.0 |
| `signalfd` | 282 | kernel/process/signal | v0.2.0 |

### Priorité 3 — Post-Strata

| Syscall | État |
|---|---|
| `ptrace` | v0.3.0 (debugger) |
| `prctl` | v0.3.0 |
| `sendfile` | v0.3.0 |
| `splice`/`tee` | v0.3.0 |
| `io_uring_*` | v0.3.0 |
| `fanotify_*` | Non planifié |
| `inotify_*` | v0.3.0 |
| `perf_event_open` | Non planifié |

---

## 4. Mapping Spécial : Appels Fichiers POSIX → ExoFS

Le système de fichiers POSIX que voit une app `exo compat` est un **mount namespace virtuel** construit par musl-exo :

```
App POSIX voit :          ExoFS réel :
/bin/calendar         →   /compat/calendar/bin/calendar   [ExoFS object]
/usr/lib/libc.so      →   /compat/calendar/lib/libc.so    [ExoFS object]
/home/user/           →   /home/eric/                      [ExoFS object]
/tmp/                 →   /tmp/calendar_tmp/               [ExoFS epoch-cleared]
/etc/localtime        →   /etc/posix/localtime             [ExoFS config object]
/proc/self/           →   Virtuel — construit depuis kernel info
/proc/self/maps       →   Virtuel — VMA tree du processus
/proc/self/status     →   Virtuel — stats process
/dev/null             →   Virtuel — IPC sink
/dev/urandom          →   IPC → crypto_server::get_entropy()
/dev/tty              →   IPC → tty_server
```

---

## 5. Redirections Vers Serveurs Ring1

```
SYSCALL (POSIX)          →  SERVEUR RING1  →  PRIMITIVE EXOOS
────────────────────────────────────────────────────────────────
read/write/open/stat     →  vfs_server     →  ExoFS read_at/write_at
socket/connect/send/recv →  network_server →  smoltcp IPC
clock_gettime            →  kernel direct  →  ktime_get_ns() / TSC
fork/execve/wait         →  kernel direct  →  process lifecycle
mmap/munmap/mprotect     →  kernel direct  →  VMM
futex                    →  kernel direct  →  futex table
/dev/urandom             →  crypto_server  →  TRNG
/dev/tty                 →  tty_server     →  TTY IPC
```

---

## 6. Tests Requis

```
musl_exo_test::priority1_read_write         PASS
musl_exo_test::priority1_open_stat_close    PASS
musl_exo_test::priority1_mmap_munmap        PASS
musl_exo_test::priority1_fork_exec_wait     PASS
musl_exo_test::priority1_signal_delivery    PASS
musl_exo_test::priority1_getcwd_chdir       PASS
musl_exo_test::priority1_getdents64         PASS
musl_exo_test::priority2_socket_tcp         PASS
musl_exo_test::priority2_epoll              PASS
musl_exo_test::priority2_futex_wait_wake    PASS
musl_exo_test::compat_calendar_runs         PASS  ← milestone
musl_exo_test::compat_curl_https            PASS  ← milestone
musl_exo_test::posix_chmod_noop_no_crash    PASS
musl_exo_test::posix_getuid_returns_0       PASS
musl_exo_test::devurandom_returns_entropy   PASS
musl_exo_test::proc_self_status_readable    PASS
```

---

*claude-alpha — ExoOS v0.2.0 — Strata — SPEC-EXO-LIBC-STRATA.md*
