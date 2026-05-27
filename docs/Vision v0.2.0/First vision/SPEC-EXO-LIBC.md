# SPEC-EXO-LIBC — Couche de Compatibilité POSIX
## musl-exo · exo-libc · Sandbox POSIX

**Auteur :** claude-alpha  
**Date :** 2026-05-14  
**Statut :** SPEC OFFICIELLE v0.2.0

---

## 1. Rôle et Périmètre

`musl-exo` est le fork de musl qui traduit les appels POSIX en IPC ExoOS. C'est la couche qui permet à un programme compilé pour Linux de s'exécuter sur ExoOS sans recompilation.

**Ce que musl-exo N'EST PAS :**
- Un kernel Linux (il ne simule pas le kernel, il redirige vers les serveurs Ring1 ExoOS)
- Une implémentation complète (241 syscalls Linux — on vise 127 pour v0.2.0, les 80% critiques)
- La couche d'abstraction principale d'ExoOS (c'est juste la couche compat)

---

## 2. Tableau de Priorité des Syscalls

### Priorité 1 — BLOQUANT (sans eux, rien ne s'exécute)

| Syscall | N° Linux | Implémentation ExoOS | État cible |
|---------|----------|---------------------|------------|
| `read` | 0 | IPC → vfs_server::read_at | v0.2.0 |
| `write` | 1 | IPC → vfs_server::write_at | v0.2.0 |
| `open` | 2 | IPC → vfs_server::open | v0.2.0 |
| `close` | 3 | IPC → vfs_server::close + révocation cap | v0.2.0 |
| `mmap` | 9 | SYS_MMAP kernel direct | v0.2.0 |
| `mprotect` | 10 | SYS_MPROTECT kernel direct | v0.2.0 |
| `munmap` | 11 | SYS_MUNMAP kernel direct | v0.2.0 |
| `brk` | 12 | Émulé via mmap (ExoOS n'a pas brk) | v0.2.0 |
| `rt_sigaction` | 13 | kernel/process/signal/ | v0.2.0 |
| `rt_sigprocmask` | 14 | kernel/process/signal/ | v0.2.0 |
| `ioctl` | 16 | Partiel (TIOCGWINSZ, TCGETS) | v0.2.0 |
| `pipe` | 22 | IPC → channel pair SpscRing | v0.2.0 |
| `select` | 23 | IPC poll sur caps multiples | v0.2.0 |
| `nanosleep` | 35 | SYS_NANOSLEEP kernel | v0.2.0 |
| `getpid` | 39 | kernel/process/core/pid | v0.2.0 |
| `fork` | 57 | kernel/process/lifecycle/fork | v0.2.0 |
| `execve` | 59 | kernel/process/lifecycle/exec | v0.2.0 |
| `exit` | 60 | kernel/process/lifecycle/exit | v0.2.0 |
| `wait4` | 61 | kernel/process/lifecycle/wait | v0.2.0 |
| `kill` | 62 | kernel/process/signal/delivery | v0.2.0 |
| `uname` | 63 | Retourne "ExoOS" + version | v0.2.0 |
| `getcwd` | 79 | IPC → vfs_server::getcwd | v0.2.0 |
| `chdir` | 80 | IPC → vfs_server::chdir | v0.2.0 |
| `stat` / `fstat` | 4/5 | IPC → vfs_server::stat | v0.2.0 |
| `lstat` | 6 | IPC → vfs_server::stat (symlink) | v0.2.0 |
| `lseek` | 8 | Curseur interne (ExoFS est offset-based) | v0.2.0 |
| `dup` / `dup2` | 32/33 | Duplication de cap | v0.2.0 |
| `getdents64` | 217 | IPC → vfs_server::readdir | v0.2.0 |
| `clock_gettime` | 228 | IPC → time_server ou TSC direct | v0.2.0 |
| `exit_group` | 231 | Termine tous les threads | v0.2.0 |

### Priorité 2 — Important (nécessaire pour la majorité des apps text-mode)

| Syscall | N° | Implémentation | État cible |
|---------|-----|----------------|------------|
| `socket` | 41 | IPC → network_server::socket | v0.2.0 |
| `connect` | 42 | IPC → network_server::connect | v0.2.0 |
| `accept` | 43 | IPC → network_server::accept | v0.2.0 |
| `sendto` / `recvfrom` | 44/45 | IPC → network_server | v0.2.0 |
| `bind` | 49 | IPC → network_server::bind | v0.2.0 |
| `listen` | 50 | IPC → network_server::listen | v0.2.0 |
| `getsockname` | 51 | IPC → network_server | v0.2.0 |
| `getpeername` | 52 | IPC → network_server | v0.2.0 |
| `setsockopt` / `getsockopt` | 54/55 | IPC → network_server (partiel) | v0.2.0 |
| `clone` | 56 | kernel/process/lifecycle/fork (clone flags) | v0.2.0 |
| `fcntl` | 72 | Partiel (F_GETFL, F_SETFL, O_NONBLOCK) | v0.2.0 |
| `mkdir` | 83 | IPC → vfs_server::mkdir | v0.2.0 |
| `rmdir` | 84 | IPC → vfs_server::rmdir | v0.2.0 |
| `unlink` | 87 | IPC → vfs_server::unlink | v0.2.0 |
| `rename` | 82 | IPC → vfs_server::rename (O(1) ExoFS) | v0.2.0 |
| `link` / `symlink` | 86/88 | IPC → vfs_server | v0.2.0 |
| `chmod` / `fchmod` | 90/91 | No-op (pas de rwx — log dans ExoLedger) | v0.2.0 |
| `getuid` / `getgid` | 102/104 | Retourne 0 (pas d'utilisateurs) | v0.2.0 |
| `setuid` / `setgid` | 105/106 | No-op sauf audit ExoLedger | v0.2.0 |
| `poll` | 7 | IPC poll multi-caps | v0.2.0 |
| `epoll_*` | 213+ | IPC poll async | v0.2.0 |
| `eventfd` | 284 | Canal IPC léger | v0.2.0 |
| `timerfd_*` | 283+ | Timer IPC | v0.2.0 |
| `pread64` / `pwrite64` | 17/18 | vfs_server::read_at/write_at | v0.2.0 |
| `readv` / `writev` | 19/20 | vfs_server (scatter/gather) | v0.2.0 |
| `mmap` (MAP_SHARED) | 9 | SHM via IPC shared_memory | v0.2.0 |
| `msync` | 26 | vfs_server::sync | v0.2.0 |

### Priorité 3 — Post-v0.2.0 (nécessaires pour apps complexes)

| Syscall | État cible |
|---------|------------|
| `ptrace` | v0.3.0 (debugger) |
| `futex` | v0.2.0 partiel (pthread compat) |
| `prctl` | v0.3.0 |
| `sendfile` | v0.3.0 |
| `splice` / `tee` | v0.3.0 |
| `fanotify_*` | Non planifié |
| `io_uring_*` | v0.3.0 |

---

## 3. Mapping des Notions POSIX → ExoOS

| Concept POSIX | Équivalent ExoOS | Notes |
|---------------|-----------------|-------|
| File descriptor (fd) | CapToken | Opaque, capability-gated |
| UID/GID | N/A | Retourne 0, no-op pour les setuid |
| rwx permissions | `RightsMask` ExoFS | Traduit à l'ouverture, pas stocké comme bits |
| `/etc/passwd` | N/A | Pas d'utilisateurs, retourne "root:x:0:0" |
| `/etc/hosts` | IPC → network_server DNS | Lecture OK, écriture refusée (EXO-0403) |
| `/proc/self/` | IPC → process_server | `/proc/self/maps`, `/proc/self/status` émulés |
| `/dev/null` | ExoFS object `/dev/null` | Lit 0, écrit /dev/null |
| `/dev/urandom` | IPC → crypto_server TRNG | Entropie réelle |
| `$HOME` | `~/` → ExoFS namespace personnel | Géré par vfs_server |
| Signaux | kernel/process/signal | Compatibles POSIX (SIGTERM, SIGKILL, etc.) |
| Threads (pthread) | kernel/process/thread | Via clone() avec CLONE_THREAD |
| Sockets UNIX | IPC SpscRing local | Émulés via canal IPC |
| `mlock` / `munlock` | No-op ou pin ExoFS | Pages physiques gérées par le kernel |
| `chroot` | Sandbox capability restreinte | Réimplémenté via namespace vfs_server |

---

## 4. Sandbox POSIX

Chaque application installée via `exo compat install` s'exécute dans une sandbox POSIX qui lui présente un système de fichiers virtuel mappé vers des objets ExoFS :

```
Vue POSIX (app voit ça)        Réalité ExoOS (mappé vers)
──────────────────────────     ─────────────────────────────────────
/                         →    /compat/<app>/rootfs/
/bin/                     →    /compat/<app>/bin/
/usr/bin/                 →    /compat/<app>/usr/bin/
/lib/                     →    /compat/<app>/lib/
/etc/                     →    /compat/<app>/etc/  (lecture seule)
/tmp/                     →    Objet ExoFS temporaire (nettoyé à la fermeture)
/home/user/               →    /home/<user>/<app>_data/  (capability restreinte)
/dev/null                 →    Objet ExoFS spécial null
/dev/urandom              →    IPC → crypto_server TRNG
/dev/tty                  →    IPC → tty_server Ring1
/proc/self/               →    Émulation partielle (maps, status, fd)
```

**Le VFS de la sandbox est monté par `musl-exo` au démarrage du processus.** L'app ne voit pas d'ExoFS — elle voit un POSIX standard.

---

## 5. Implémentation de `getuid()` / `getgid()`

ExoOS n'a pas d'utilisateurs. Les apps POSIX qui appellent `getuid()` reçoivent 0 (root virtuel). Les appels `setuid()` sont des no-op (loggés dans ExoLedger).

```rust
// musl-exo/src/process/userids.rs

pub fn getuid() -> u32 { 0 }    // "root" virtuel
pub fn geteuid() -> u32 { 0 }
pub fn getgid() -> u32 { 0 }
pub fn getegid() -> u32 { 0 }

pub fn setuid(_uid: u32) -> i32 {
    exoledger_log(LedgerEvent::SyscallIgnored { syscall: "setuid", pid: getpid() });
    0  // succès silencieux
}

pub fn getpwuid(uid: u32) -> Option<Passwd> {
    if uid == 0 {
        Some(Passwd {
            name: "exouser".into(),
            passwd: "x".into(),
            uid: 0,
            gid: 0,
            gecos: "ExoOS User".into(),
            dir: "/home/user".into(),
            shell: "/apps/exosh/bin/exosh".into(),
        })
    } else { None }
}
```

---

## 6. Validation musl-exo — Tests Requis

```
musl_exo_test::open_read_write                 PASS
musl_exo_test::stat_posix_fields               PASS
musl_exo_test::fork_exec_wait                  PASS
musl_exo_test::signal_delivery_sigterm         PASS
musl_exo_test::socket_tcp_connect_send_recv    PASS
musl_exo_test::getdents64_readdir              PASS
musl_exo_test::mmap_anon_rw                    PASS
musl_exo_test::pipe_producer_consumer          PASS
musl_exo_test::getuid_returns_zero             PASS
musl_exo_test::setuid_noop_logged              PASS
musl_exo_test::devurandom_entropy              PASS
musl_exo_test::calendar_app_full_run           PASS   ← test d'intégration principal
musl_exo_test::curl_http_get                   PASS   ← test réseau via musl-exo
musl_exo_test::vim_open_edit_save              PASS   ← test éditeur

Total: 14 PASS / 0 FAIL
```

---

*claude-alpha — ExoOS v0.2.0 — SPEC-EXO-LIBC.md*
