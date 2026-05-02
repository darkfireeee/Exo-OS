# ExoOS — Analyse de couverture POSIX et plan de corrections vers 95%

**Version** : 1.0  
**Date** : Avril 2026  
**Question** : les corrections des documents 10 et 11 suffisent-elles pour atteindre 95% POSIX ?  
**Réponse courte** : **non** — score actuel ~62%, corrections précédentes → ~72%, cible 95% demande 3 groupes de corrections supplémentaires.

---

## 0. Découvertes préliminaires importantes

### fork et execve sont implémentés — pas dans table.rs

`kernel/src/syscall/dispatch.rs` intercepte `SYS_FORK`, `SYS_VFORK` et `SYS_EXECVE` **avant** de consulter `table.rs`. Les stubs `ENOSYS` dans `table.rs` ne sont jamais atteints pour ces trois syscalls.

```
dispatch() ligne 156 : if nr == SYS_FORK   → handle_fork_like_inplace()  → do_fork() ✅
dispatch() ligne 164 : if nr == SYS_VFORK  → handle_fork_like_inplace()  → do_fork() ✅
dispatch() ligne 177 : if nr == SYS_EXECVE → handle_execve_inplace()     → do_execve() ✅
```

Ces trois appels sont fonctionnels avec CoW, TLB flush, ELF loader. C'est une bonne nouvelle — la base fork/exec/clone est complète.

### Le PCB n'a ni cwd, ni umask, ni rlimits

`ProcessControlBlock` contient : `pid`, `ppid`, `tgid`, `sid`, `pgid`, `state`, `flags`, `creds`, `files`, `address_space`, `sig_handlers`, namespaces. Ce qui est absent :

```rust
// Champs manquants dans ProcessControlBlock :
// pub cwd: SpinLock<ObjectIno>,        // répertoire de travail courant
// pub umask: AtomicU32,                // masque de création de fichiers
// pub rlimits: SpinLock<RlimitTable>,  // limites de ressources (getrlimit/setrlimit)
```

`getcwd`, `chdir`, `fchdir`, `umask`, `getrlimit`, `setrlimit` ne peuvent pas être implémentés sans modifier le PCB. Ce n'est pas une correction de plomberie — c'est une extension de structure.

### Les syscalls réseau ne sont pas routés vers network_server

`network_server` existe et implémente une table de sockets, une pile TCP/UDP, des bindings BSD. Mais aucun chemin dans `dispatch.rs` ou `table.rs` ne route `SYS_SOCKET`, `SYS_CONNECT`, `SYS_BIND`, etc. vers ce serveur. Le réseau est orphelin côté kernel.

---

## 1. État réel par groupe — bilan complet

### Groupe A — Fonctionnel aujourd'hui (62 syscalls)

| Catégorie | Syscalls |
|---|---|
| FS base | read, write, open, close, stat, fstat, lstat, lseek, dup, dup2, fcntl(partiel), mkdir, rmdir, unlink, symlink, openat, getdents64, readlink, symlinkat, readlinkat |
| FS (après doc 11) | rename, renameat, truncate, ftruncate, flock, F_SETLK/F_GETLK dans fcntl |
| Mémoire | mmap, munmap, mprotect, brk |
| Processus | **fork** ✅, **vfork** ✅, **execve** ✅ (dispatch), clone, exit, exit_group, wait4 |
| Signaux | rt_sigaction, rt_sigprocmask, rt_sigreturn, kill, tgkill, sigaltstack |
| Temps | nanosleep, futex, clock_gettime, gettimeofday |
| Identité | getpid, gettid, getuid, geteuid, getgid, getegid, getppid, getcpu, sched_yield |

**Score actuel (après doc 11)** : ~72% des programmes courants peuvent démarrer et effectuer des opérations de base.

---

### Groupe B — Handlers implémentés, non câblés dans table.rs (corrections triviales)

Ces handlers ont du code réel dans `handlers/*.rs` mais ne sont **pas dans `get_handler()`**. Ce sont des oublis de câblage purs.

| Syscall | Handler | Implémentation | Impact |
|---|---|---|---|
| `arch_prctl` | `misc.rs` | ✅ RÉELLE — `wrmsrl(FS_BASE)` | **CRITIQUE** — musl TLS, pthreads |
| `set_tid_address` | `misc.rs` | ✅ RÉELLE — retourne `gettid()` | **CRITIQUE** — `pthread_create` |
| `uname` | `misc.rs` | ✅ RÉELLE — écrit struct `utsname` | Important — la plupart des programmes |
| `waitid` | `process.rs` | ✅ RÉELLE — `do_waitpid()` | Important — gestion enfants |
| `clock_nanosleep` | `time.rs` | ✅ RÉELLE — délègue `sys_nanosleep` | Utile — timers précis |

**Effort : 1 heure. Correction dans `get_handler()` uniquement.**

```rust
// Ajouter dans get_handler(), section "Processus" :
SYS_ARCH_PRCTL   => crate::syscall::handlers::misc::sys_arch_prctl,
SYS_SET_TID_ADDRESS => crate::syscall::handlers::misc::sys_set_tid_address,
SYS_UNAME        => crate::syscall::handlers::misc::sys_uname,
SYS_WAITID       => crate::syscall::handlers::process::sys_waitid,
SYS_CLOCK_NANOSLEEP => crate::syscall::handlers::time::sys_clock_nanosleep,
```

**Note critique sur `arch_prctl`** : sans `ARCH_SET_FS`, musl ne peut pas initialiser le TLS de l'application (`errno` thread-local, etc.). Tout programme lié à musl crashe immédiatement après `_start` sans ce syscall. C'est le correctif de plus fort impact du groupe B.

---

### Groupe C — Handlers à corps ENOSYS, nécessitent une implémentation

Ces fonctions passent la validation d'arguments puis retournent `ENOSYS`. Il faut à la fois les câbler ET implémenter le corps.

#### C-1 : `pread64` / `pwrite64` — lecture/écriture à offset sans déplacer le curseur

**Handler** : `handlers/fd.rs` — validation OK, corps `ENOSYS`.

Ces opérations sont fondamentales pour les bases de données (SQLite, PostgreSQL) et tout code qui fait de l'accès aléatoire sans modifier la position courante.

```rust
// fs_bridge.rs — ajouter :
pub fn fs_pread64(fd: u32, buf_ptr: u64, count: usize, offset: u64, pid: u32)
    -> Result<i64, FsBridgeError>
{
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let buf = UserBuf::validate(buf_ptr, count, IO_BUF_MAX)
        .map_err(|_| FsBridgeError::Fault)?;
    // Sauvegarder la position courante, lire à l'offset, restaurer
    let current_pos = OBJECT_TABLE.get_position(fd).map_err(exofs_to_bridge_error)?;
    OBJECT_TABLE.seek(fd, offset as i64, SEEK_SET).map_err(exofs_to_bridge_error)?;
    let n = OBJECT_TABLE.read(fd, buf.as_slice_mut()).map_err(exofs_to_bridge_error)?;
    OBJECT_TABLE.seek(fd, current_pos as i64, SEEK_SET).map_err(exofs_to_bridge_error)?;
    Ok(n as i64)
}

// table.rs — câbler :
SYS_PREAD64 => |fd, buf, count, offset, _, _| -> i64 {
    stat_inc(SYS_PREAD64);
    let pid = syscall_current_pid();
    fs_bridge::bridge_result(fs_bridge::fs_pread64(fd as u32, buf, count as usize, offset, pid))
},
SYS_PWRITE64 => // symétrique
```

**Effort : 2 heures.**

#### C-2 : `readv` / `writev` — scatter-gather I/O

Très utilisés par les serveurs réseau, curl, openssl, et toute application qui évite des copies de buffer. ExoFS `io/io_uring.rs` mentionne les opcodes Readv/Writev mais il n'y a pas d'implémentation syscall.

```rust
// Structure iovec ABI Linux
#[repr(C)]
struct IoVec { iov_base: u64, iov_len: u64 }

pub fn fs_readv(fd: u32, iov_ptr: u64, iovcnt: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if iovcnt == 0 || iovcnt > 1024 { return Err(FsBridgeError::Invalid); }
    let mut total: i64 = 0;
    for i in 0..iovcnt as usize {
        // Lire chaque iovec depuis userspace
        let iov_addr = iov_ptr + (i * size_of::<IoVec>()) as u64;
        let iov: IoVec = read_user_typed(iov_addr).map_err(|_| FsBridgeError::Fault)?;
        if iov.iov_len == 0 { continue; }
        if iov.iov_len > IO_BUF_MAX as u64 { return Err(FsBridgeError::Invalid); }
        let n = fs_read(fd, iov.iov_base, iov.iov_len as usize, pid)?;
        if n == 0 { break; } // EOF
        total += n;
        if (n as u64) < iov.iov_len { break; } // lecture partielle
    }
    Ok(total)
}
// fs_writev symétrique
```

**Effort : 3 heures.**

#### C-3 : `access` — vérification d'accessibilité

Utilisé par `ls`, scripts shell (`if [ -f file ]`), `configure` autotools. Fréquemment appelé.

```rust
pub fn fs_access(path: &[u8], mode: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    // F_OK (0) : vérifier existence uniquement
    // R_OK (4), W_OK (2), X_OK (1) : ExoFS n'a pas de modèle Unix permissions
    // → vérifier existence pour F_OK, retourner 0 pour R/W/X (tout le monde peut tout)
    //   ou retourner EACCES si le process n'est pas owner
    let _ = pid;
    use crate::fs::exofs::path::path_index::PATH_INDEX;
    match PATH_INDEX.resolve(path) {
        Some(_) => Ok(0),   // existe → accessible
        None => Err(FsBridgeError::NotFound),
    }
}
```

**Note** : sans modèle de permissions Unix dans ExoFS, `access(R_OK)` retourne toujours 0 si le fichier existe. C'est acceptable pour 95% des cas d'usage (la plupart des programmes vérifient juste F_OK).

**Effort : 1 heure.**

#### C-4 : `fsync` / `fdatasync` — flush vers le stockage persistant

ExoFS a `SYS_EXOFS_EPOCH_COMMIT` qui effectue 3 barrières NVMe — c'est exactement la sémantique de `fsync`. Il faut juste le raccorder.

```rust
pub fn fs_fsync(fd: u32, _data_only: bool, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (fd, pid);
    // Déclencher un epoch commit — équivalent à fdatasync + fsync pour ExoFS
    // ExoFS garantit la durabilité via les 3 barrières NVMe de epoch_commit
    use crate::fs::exofs::epoch::EPOCH_MANAGER;
    EPOCH_MANAGER.commit_current()
        .map(|_| 0i64)
        .map_err(|_| FsBridgeError::Io)
}

// table.rs :
SYS_FSYNC => |fd, _, _, _, _, _| -> i64 {
    stat_inc(SYS_FSYNC);
    let pid = syscall_current_pid();
    fs_bridge::bridge_result(fs_bridge::fs_fsync(fd as u32, false, pid))
},
SYS_FDATASYNC => |fd, _, _, _, _, _| -> i64 {
    stat_inc(SYS_FDATASYNC);
    let pid = syscall_current_pid();
    fs_bridge::bridge_result(fs_bridge::fs_fsync(fd as u32, true, pid))
},
```

**Effort : 2 heures.** Débloque SQLite WAL, PostgreSQL, tout système de journalisation.

#### C-5 : `creat` — alias open(O_CREAT|O_WRONLY|O_TRUNC)

```rust
// table.rs :
SYS_CREAT => |path_ptr, mode, _, _, _, _| -> i64 {
    stat_inc(SYS_CREAT);
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let pid  = syscall_current_pid();
    const O_CREAT: u32 = 0x0040; const O_WRONLY: u32 = 0x0001; const O_TRUNC: u32 = 0x0200;
    use crate::syscall::fs_bridge;
    fs_bridge::bridge_result(fs_bridge::fs_open(
        path.as_bytes(), O_CREAT | O_WRONLY | O_TRUNC, mode as u32, pid,
    ))
},
```

**Effort : 30 minutes.**

---

### Groupe D — PCB à étendre (CWD, umask, rlimits)

Ces fonctionnalités nécessitent d'ajouter des champs au PCB. C'est une modification de structure qui implique `fork.rs` (propagation) et `exec.rs` (reset).

#### D-1 : Répertoire de travail courant (`getcwd`, `chdir`, `fchdir`)

```rust
// Dans ProcessControlBlock — ajouter :
/// Répertoire de travail courant (inode ExoFS).
/// Initialisé à INO_ROOT au boot, propagé par fork, réinitialisé optionnellement par exec.
pub cwd: SpinLock<ObjectIno>,
```

```rust
// Initialisation dans ProcessControlBlock::new() :
cwd: SpinLock::new(posix_bridge::inode_emulation::INO_ROOT),

// Dans do_fork() — propager :
child_pcb.cwd = SpinLock::new(*parent_pcb.cwd.lock());

// Syscall chdir :
pub fn fs_chdir(path: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    let blob = PATH_INDEX.resolve(path).ok_or(FsBridgeError::NotFound)?;
    let ino  = INODE_EMULATION.blob_to_ino(blob).ok_or(FsBridgeError::NotFound)?;
    // Vérifier que c'est un répertoire
    let entry = INODE_EMULATION.get_entry(ino).ok_or(FsBridgeError::NotFound)?;
    if entry.flags & inode_flags::DIRECTORY == 0 { return Err(FsBridgeError::NotDir); }
    let pcb = PROCESS_REGISTRY.find_by_pid(Pid(pid)).ok_or(FsBridgeError::PermDenied)?;
    *pcb.cwd.lock() = ino;
    Ok(0)
}

// Syscall getcwd :
pub fn fs_getcwd(buf_ptr: u64, size: usize, pid: u32) -> Result<i64, FsBridgeError> {
    let pcb = PROCESS_REGISTRY.find_by_pid(Pid(pid)).ok_or(FsBridgeError::PermDenied)?;
    let cwd_ino = *pcb.cwd.lock();
    // Reconstruire le chemin depuis cwd_ino → racine (remontée via parent)
    let path = INODE_EMULATION.ino_to_path(cwd_ino).ok_or(FsBridgeError::Io)?;
    if path.len() + 1 > size { return Err(FsBridgeError::Invalid); } // ERANGE
    copy_to_user(buf_ptr, path.as_bytes())?;
    Ok(path.len() as i64 + 1)
}
```

**Effort : 1 jour** (PCB + fork + exec + syscall + path reconstruction).  
**Impact** : shell fonctionnel, tous les programmes utilisant des chemins relatifs.

#### D-2 : `umask` — masque de création de fichiers

```rust
// Dans ProcessControlBlock — ajouter :
pub umask: AtomicU32, // default: 0o022

// Initialisation : umask: AtomicU32::new(0o022),
// fork : child.umask.store(parent.umask.load(Relaxed), Relaxed);

// Syscall :
SYS_UMASK => |mask, _, _, _, _, _| -> i64 {
    stat_inc(SYS_UMASK);
    let pid = syscall_current_pid();
    let pcb = match PROCESS_REGISTRY.find_by_pid(Pid(pid)) {
        Some(p) => p, None => return ESRCH,
    };
    let old = pcb.umask.swap((mask as u32) & 0o777, Ordering::Relaxed);
    old as i64  // retourne l'ancien masque (POSIX)
},
```

**Effort : 2 heures.** La valeur `umask` doit aussi être appliquée dans `fs_open()` sur le `mode` argument — `effective_mode = mode & !umask`.

#### D-3 : `getrlimit` / `setrlimit` — limites de ressources

```rust
// Dans ProcessControlBlock — ajouter :
pub rlimits: SpinLock<[Rlimit; RLIMIT_NLIMITS]>,

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Rlimit { pub rlim_cur: u64, pub rlim_max: u64 }

const RLIMIT_NLIMITS: usize = 16;
// Ressources utiles : RLIMIT_NOFILE(7)=1024, RLIMIT_STACK(3)=8MB, RLIMIT_AS(9)=ulimitée

// Syscalls :
SYS_GETRLIMIT => |resource, rlim_ptr, _, _, _, _| -> i64 {
    let pid = syscall_current_pid();
    let pcb = match PROCESS_REGISTRY.find_by_pid(Pid(pid)) { Some(p) => p, None => return ESRCH };
    if resource as usize >= RLIMIT_NLIMITS { return EINVAL; }
    let rl = pcb.rlimits.lock()[resource as usize];
    write_user_typed(rlim_ptr, &rl).map(|_| 0).unwrap_or(EFAULT)
},
```

**Effort : 3 heures.** Beaucoup de programmes appellent `getrlimit(RLIMIT_NOFILE)` pour savoir combien de fds ils peuvent ouvrir.

---

### Groupe E — AT variants (`mkdirat`, `unlinkat`, `newfstatat`, `fchmodat`, `faccessat`)

Ces variants sont utilisés par les programmes modernes (glibc depuis 2005 génère `openat`/`unlinkat`/`newfstatat` à la place des variantes sans `at`).

La stratégie est uniforme : si `dirfd == AT_FDCWD (-100)` → déléguer au handler de base. Si `dirfd` est un vrai fd → résoudre depuis ce répertoire (Phase 2).

```rust
// table.rs :
SYS_MKDIRAT => |dirfd, path_ptr, mode, _, _, _| -> i64 {
    stat_inc(SYS_MKDIRAT);
    const AT_FDCWD: i64 = -100;
    if dirfd as i64 != AT_FDCWD { return ENOSYS; } // Phase 2 : dirfd réel
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let pid  = syscall_current_pid();
    fs_bridge::bridge_result(fs_bridge::fs_mkdir(path.as_bytes(), mode as u32, pid))
},

SYS_UNLINKAT => |dirfd, path_ptr, flags, _, _, _| -> i64 {
    stat_inc(SYS_UNLINKAT);
    const AT_FDCWD: i64 = -100;
    const AT_REMOVEDIR: u64 = 0x200;
    if dirfd as i64 != AT_FDCWD { return ENOSYS; }
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let pid  = syscall_current_pid();
    if flags & AT_REMOVEDIR != 0 {
        fs_bridge::bridge_result(fs_bridge::fs_rmdir(path.as_bytes(), pid))
    } else {
        fs_bridge::bridge_result(fs_bridge::fs_unlink(path.as_bytes(), pid))
    }
},

SYS_NEWFSTATAT => |dirfd, path_ptr, stat_ptr, flags, _, _| -> i64 {
    stat_inc(SYS_NEWFSTATAT);
    const AT_FDCWD: i64 = -100;
    const AT_EMPTY_PATH: u64 = 0x1000;
    if dirfd as i64 != AT_FDCWD { return ENOSYS; }
    if flags & AT_EMPTY_PATH != 0 { return ENOSYS; } // fstat via empty path
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let pid  = syscall_current_pid();
    fs_bridge::bridge_result(fs_bridge::fs_stat(path.as_bytes(), stat_ptr, pid))
},

SYS_FACCESSAT => |dirfd, path_ptr, mode, _, _, _| -> i64 {
    stat_inc(SYS_FACCESSAT);
    const AT_FDCWD: i64 = -100;
    if dirfd as i64 != AT_FDCWD { return ENOSYS; }
    let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
    let pid  = syscall_current_pid();
    fs_bridge::bridge_result(fs_bridge::fs_access(path.as_bytes(), mode as u32, pid))
},
```

**Effort : 2 heures.** Ces variants sont indispensables pour la glibc/musl moderne.

---

### Groupe F — `statfs` / `fstatfs` — informations sur le filesystem

Utilisés par `df`, Python `os.statvfs()`, Go `syscall.Statfs()`, beaucoup de programs d'administration.

```rust
// Struct statfs ABI Linux x86-64 (88 bytes)
#[repr(C)]
struct Statfs {
    f_type:    i64,   // EXOFS_MAGIC = 0x4558_4F46
    f_bsize:   i64,   // taille de bloc = 4096
    f_blocks:  u64,   // total blocs = capacité / 4096
    f_bfree:   u64,   // blocs libres
    f_bavail:  u64,   // blocs disponibles (= f_bfree pour ExoFS sans quota)
    f_files:   u64,   // inodes totaux ≈ capacité / 256
    f_ffree:   u64,   // inodes libres
    f_fsid:    [i32; 2],
    f_namelen: i64,   // 255
    f_frsize:  i64,   // fragment = 4096
    f_flags:   i64,   // 0
    _reserved: [i64; 4],
}

// Remplir depuis EPOCH_MANAGER.storage_stats() et BLOB_CACHE.stats()
pub fn fs_statfs(path_or_fd: Either<&[u8], u32>, stat_ptr: u64, pid: u32)
    -> Result<i64, FsBridgeError>
{
    use crate::fs::exofs::epoch::EPOCH_MANAGER;
    let stats  = EPOCH_MANAGER.storage_stats(); // { total_bytes, used_bytes, object_count }
    let bsize  = 4096u64;
    let total  = stats.total_bytes / bsize;
    let used   = stats.used_bytes  / bsize;
    let sf = Statfs {
        f_type:    0x4558_4F46i64,  // EXOFS_MAGIC
        f_bsize:   bsize as i64,
        f_blocks:  total,
        f_bfree:   total.saturating_sub(used),
        f_bavail:  total.saturating_sub(used),
        f_files:   stats.object_count,
        f_ffree:   u64::MAX / 2,    // ExoFS illimité
        f_fsid:    [0x4558, 0x4F46],
        f_namelen: 255,
        f_frsize:  bsize as i64,
        f_flags:   0,
        _reserved: [0; 4],
    };
    write_user_typed(stat_ptr, &sf).map(|_| 0i64).map_err(|_| FsBridgeError::Fault)
}
```

**Effort : 2 heures.** Dépend d'un accès aux stats de stockage ExoFS.

---

### Groupe G — Réseau : relier network_server aux syscalls socket

`network_server` implémente `NETWORK_MSG_SOCKET_OPEN`, `NETWORK_MSG_BIND`, `NETWORK_MSG_CONNECT`, `NETWORK_MSG_SEND`, `NETWORK_MSG_RECV`, `NETWORK_MSG_CLOSE`. Il faut créer le pont kernel → IPC → network_server pour les syscalls 41-55.

La stratégie est un proxy IPC depuis le kernel. Chaque syscall réseau devient un message IPC synchrone (`SYS_EXO_IPC_CALL`) vers PID network_server.

```rust
// Nouveau module : kernel/src/syscall/net_bridge.rs

pub fn net_socket(domain: u32, socktype: u32, protocol: u32, pid: u32) -> i64 {
    let req = NetworkRequest {
        msg_type: NETWORK_MSG_SOCKET_OPEN,
        sender_pid: pid,
        payload: encode_socket_open(domain, socktype, protocol),
    };
    let reply = ipc_call_sync(PID_NETWORK_SERVER, &req);
    if reply.status == 0 { reply.fd as i64 } else { -(reply.errno as i64) }
}
// Similaire pour bind, connect, accept, sendto, recvfrom, setsockopt, getsockopt, shutdown

// table.rs :
SYS_SOCKET   => |dom, typ, prot, _, _, _| -> i64 { net_bridge::net_socket(...) },
SYS_BIND     => |fd, addr, addrlen, _, _, _| -> i64 { net_bridge::net_bind(...) },
SYS_CONNECT  => |fd, addr, addrlen, _, _, _| -> i64 { net_bridge::net_connect(...) },
SYS_ACCEPT   => |fd, addr, addrlen, _, _, _| -> i64 { net_bridge::net_accept(...) },
SYS_SENDTO   => |fd, buf, len, flags, addr, alen| -> i64 { net_bridge::net_sendto(...) },
SYS_RECVFROM => |fd, buf, len, flags, addr, alen| -> i64 { net_bridge::net_recvfrom(...) },
SYS_SHUTDOWN => |fd, how, _, _, _, _| -> i64 { net_bridge::net_shutdown(...) },
SYS_GETSOCKNAME => ...,
SYS_GETPEERNAME => ...,
SYS_SETSOCKOPT  => ...,
SYS_GETSOCKOPT  => ...,
SYS_SOCKETPAIR  => ...,
```

**Effort : 1 semaine** (protocole IPC sync kernel→Ring1, proxy des 13 syscalls, tests TCP/UDP basiques).

---

### Groupe H — `pipe`, `poll`, `select` (long terme)

Ces éléments sont documentés dans `ExoOS_Corrections_11_ExoFS.md` (FS-GAP-01 et FS-GAP-02). Ils sont rappelés ici pour la complétude du plan.

| Syscall | Effort | Impact |
|---|---|---|
| `pipe` (22) | 1-2 semaines | Shell pipelines, redirections |
| `poll` (7) | 3-4 semaines | Serveurs réseau, I/O multiplexé |
| `select` (23) | 1 semaine | Applications legacy (dépend de poll) |
| `epoll_*` (213/232/233) | 1 semaine | Serveurs hautes performances (dépend de poll) |

Sans `pipe` et `poll`, les shells (`bash`, `sh`) ne peuvent pas faire `cmd1 | cmd2` ni attendre sur plusieurs fds. L'objectif 95% POSIX est inaccessible sans ces deux primitives.

---

## 2. Tableau de couverture — avant et après corrections

| Groupe de corrections | Syscalls ajoutés | Score estimé | Effort total |
|---|---|---|---|
| **Baseline actuel (doc 11 inclus)** | 62 | ~72% | — |
| **+ Groupe B** (câblage) | +5 | ~76% | 1h |
| **+ Groupe C** (pread/pwrite/readv/writev/access/fsync/creat) | +7 | ~82% | 10h |
| **+ Groupe D** (PCB : cwd/umask/rlimit) | +6 | ~86% | 2 jours |
| **+ Groupe E** (AT variants) | +5 | ~88% | 2h |
| **+ Groupe F** (statfs/fstatfs) | +2 | ~89% | 2h |
| **+ Groupe G** (réseau) | +13 | ~93% | 1 semaine |
| **+ Groupe H** (pipe+poll+select) | +4 | **~96%** | 5-6 semaines |

---

## 3. Plan d'exécution recommandé

### Sprint 1 — 2 jours — de 72% à 88%

Travail purement mécanique, faible risque de régression :

1. **Câblez le Groupe B** (1h) : `arch_prctl`, `set_tid_address`, `uname`, `waitid`, `clock_nanosleep` dans `get_handler()`.
2. **Implémentez le Groupe C** (10h) : `pread64`, `pwrite64`, `readv`, `writev`, `access`, `fsync`, `fdatasync`, `creat`.
3. **Implémentez le Groupe E** (2h) : AT variants avec politique `AT_FDCWD only`.
4. **Implémentez le Groupe F** (2h) : `statfs`, `fstatfs`.

**Résultat** : +16% en 2 jours. musl TLS fonctionne, SQLite fonctionne, Python s'initialise.

### Sprint 2 — 4-5 jours — de 88% à 91%

5. **Étendez le PCB** (Groupe D, 2 jours) : `cwd` + `getcwd`/`chdir`/`fchdir`, `umask`, `rlimits`/`getrlimit`/`setrlimit`. Mettre à jour `do_fork()` et `do_execve()`.

**Résultat** : +3%. Shells utilisant des chemins relatifs et `cd` fonctionnent. Python `os.getcwd()` fonctionne.

### Sprint 3 — 1 semaine — de 91% à 93%

6. **Réseau** (Groupe G) : créer `net_bridge.rs`, connecter les 13 syscalls au `network_server` via IPC synchrone.

**Résultat** : +2%. `curl`, sockets TCP/UDP, applications réseau fonctionnent.

### Sprint 4 — 5-6 semaines — de 93% à 96%

7. **pipe** : buffer circulaire kernel + intégration fd table.
8. **poll** / `select` : wait queue + notification depuis vfs_read/vfs_write.
9. **epoll** : surcouche de poll.

**Résultat** : +3%. `bash -c "ls | grep foo"` fonctionne. Serveurs réseau utilisant poll/epoll fonctionnent.

---

## 4. Ce que 95% POSIX ne couvre pas (hors périmètre acceptable)

Les éléments suivants ne sont pas nécessaires pour 95% et peuvent rester `ENOSYS` :

| Syscall | Raison |
|---|---|
| `chmod`, `fchmod`, `chown`, `fchown`, `lchown` | ExoFS n'a pas de modèle Unix permissions — retourner 0 (succès silencieux) est acceptable |
| `link` (hard links) | ExoFS objet model — hard links n'ont pas de sens direct ; `ENOSYS` ou `EPERM` correct |
| `mknod` | Fichiers device non nécessaires (ExoOS n'a pas de `/dev` traditionnel) |
| `shmget`, `shmat`, `msgget`, `semget` (SysV IPC) | Remplacés par IPC Exo-OS natif |
| `ptrace` | Hors portée Phase 1 |
| `sendfile` | Optimisation, `read`+`write` suffit pour 95% |
| `epoll_pwait`, `ppoll`, `pselect6` | Variantes signal-safe, dépendent de poll (Phase 4) |
| `ioctl` | Seul `TIOCGWINSZ` compte — retourner `ENOTTY` pour le reste |

**Sur `chmod`/`chown`** : retourner `0` (succès silencieux) plutôt que `ENOSYS` est la bonne décision. Beaucoup de scripts shell font `chmod +x foo` sans vérifier le résultat. Si cela retourne `ENOSYS`, certains scripts s'arrêtent. Si cela retourne `0`, ils continuent. ExoFS a son propre modèle de permissions par capability, pas par bits Unix.

---

## 5. Synthèse — réponse directe

Les corrections des documents 10 et 11 portent le score de ~62% à ~72%.

**Pour atteindre 95%, il faut en plus :**

```
Sprint 1 (2 jours)   : câblage + pread/readv/access/fsync/creat + AT variants + statfs → 88%
Sprint 2 (4-5 jours) : PCB cwd/umask/rlimit + getcwd/chdir/umask/getrlimit          → 91%
Sprint 3 (1 semaine) : connexion réseau (13 syscalls → network_server)               → 93%
Sprint 4 (6 semaines): pipe + poll + select + epoll                                   → 96%
─────────────────────────────────────────────────────────────────────────────────────
Total : ~8 semaines de développement pour passer de 72% à 96%
```

Le goulot d'étranglement n'est pas la quantité de syscalls — c'est `pipe` et `poll`. Sans eux, les shells ne peuvent pas faire de pipelines et les serveurs ne peuvent pas multiplexer leurs connexions. Ce sont des primitives d'isolation temporelle (attente d'événements) qui nécessitent une infrastructure de wait queue inexistante aujourd'hui dans ExoOS.

---

*Analyse couverture POSIX ExoOS — Avril 2026*  
*Sources : `kernel/src/syscall/`, `kernel/src/process/core/pcb.rs`, `servers/network_server/`*
