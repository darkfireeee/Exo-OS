# ExoOS — Corrections ExoFS : Couche Syscall & Translation Layer

**Version** : 1.0  
**Date** : Avril 2026  
**Périmètre** : `kernel/src/syscall/table.rs`, `kernel/src/syscall/fs_bridge.rs`, `kernel/src/syscall/handlers/fs_posix.rs`, `kernel/src/fs/exofs/posix_bridge/`, `servers/vfs_server/src/main.rs`  
**Méthode** : lecture complète des sources + croisement avec `ExoFS_Translation_Layer_v5_FINAL.md`

---

## 0. Résumé exécutif

Le cœur ExoFS (150K lignes, GC tricolore, crypto, dédup, snapshot, cache) est solide. Le problème est en amont : la **plomberie syscall → posix_bridge est incomplète**. Plusieurs fonctions Ring 0 sont entièrement implémentées dans `vfs_compat.rs` mais ne sont jamais appelées parce que ni `fs_bridge.rs` ni `table.rs` ne les exposent.

| Réf. | Titre | Gravité | Effort |
|---|---|---|---|
| FS-BUG-01 | `fs_posix.rs` : fichier zombie (24 fonctions ENOSYS, non câblées) | P1 | Suppression |
| FS-BUG-02 | `rename` implémenté dans posix_bridge, non câblé dans bridge+table | P0 | 2h |
| FS-BUG-03 | `truncate`/`ftruncate` implémentés, non câblés | P0 | 1h |
| FS-BUG-04 | `flock` (F_SETLK/F_GETLK) implémenté dans `fcntl_lock.rs`, non exposé | P0 | 2h |
| FS-BUG-05 | `fcntl` bridge : F_SETLK/F_GETLK tombent dans `_ => Invalid` | P0 | 1h |
| FS-GAP-01 | `poll`/`epoll` : aucune implémentation Ring 0 | P1 | Long |
| FS-GAP-02 | `pipe` : aucune implémentation | P1 | Long |
| FS-GAP-03 | `sendfile` : aucune implémentation | P2 | Moyen |
| FS-GAP-04 | `vfs_server` : 4 ops sur ~15 spécifiées par TL v5 | P1 | Long |
| FS-NOTE-01 | `INCOH-02/03` : doc TL v5 vs réalité — action de documentation | P1 | 1h |

---

## 1. Réflexion sur la structure du problème

Avant les correctifs mécaniques, il faut comprendre pourquoi ces trous existent.

### La dichotomie "implémenté mais non câblé"

`vfs_compat.rs` expose `vfs_rename()`, `vfs_truncate()`, et plusieurs autres opérations complètement fonctionnelles. Pourtant `SYS_RENAME` et `SYS_TRUNCATE` tombent dans `_ => sys_enosys` dans `table.rs`. Comment ?

La réponse est dans l'historique du développement. Le fichier `fs_posix.rs` (326 lignes, 24 fonctions, toutes `ENOSYS`) porte la trace d'une refactorisation incomplète :

```
Intention initiale :
  table.rs → handlers/fs_posix.rs → fs/ (futur)

Refactorisation P0-04 (corriger read/write/open/close) :
  table.rs → fs_bridge.rs → posix_bridge/ (réel)

Résultat :
  - fs_bridge.rs couvre ~18 opérations (câblées dans table.rs)
  - fs_posix.rs reste en place (non câblé, non supprimé)
  - vfs_compat.rs a été enrichi avec rename/truncate/flock
  - fs_bridge.rs n'a jamais reçu fs_rename/fs_truncate/fs_flock
  - table.rs n'a jamais reçu SYS_RENAME/SYS_TRUNCATE/SYS_FLOCK
```

Les FS-BUG-02 à 05 sont **tous de la plomberie manquante**, pas des implémentations manquantes. Le code Ring 0 fonctionne. Il n'est jamais appelé.

### La distinction "implémenté vs spécifié"

Pour `poll`, `pipe`, `sendfile`, et les extensions vfs_server, la situation est différente : ces opérations ne sont **nulle part** dans `kernel/src/`. Il n'y a pas de `poll_wake`, pas de `do_pipe`, pas de structure epoll. Ce sont de vraies implémentations manquantes, pas des connexions manquantes.

Les deux catégories ont des corrections fondamentalement différentes et des efforts très différents.

---

## 2. FS-BUG-01 — Fichier zombie `handlers/fs_posix.rs`

### Observation

`kernel/src/syscall/handlers/fs_posix.rs` : 326 lignes, 24 fonctions publiques (`sys_stat`, `sys_mkdir`, `sys_rename`, `sys_truncate`, `sys_ftruncate`, `sys_flock`, etc.). Toutes retournent `ENOSYS` en corps après validation de base. Le fichier a bien `pub mod fs_posix;` dans `handlers/mod.rs`.

**Mais aucune de ces 24 fonctions n'apparaît dans `table.rs`.**

`table.rs` définit ses propres versions inline de `sys_stat`, `sys_mkdir`, etc., qui appellent directement `fs_bridge`. `handlers/fs_posix.rs` n'est jamais appelé.

### Impact

- Confusion de lecture : quelqu'un qui cherche l'implémentation de `sys_rename` trouvera d'abord `fs_posix.rs` avec sa validation et son `ENOSYS`, et conclura à tort que `rename` n'est pas implémenté.
- Code mort : 326 lignes maintenues pour rien.
- Piège de refactorisation : les 24 fonctions déclarent `pub` et sont compilées, ce qui peut masquer des erreurs de linkage.

### Correction

```bash
# Supprimer le fichier
rm kernel/src/syscall/handlers/fs_posix.rs

# Retirer la déclaration de module dans handlers/mod.rs
# Avant :
pub mod fd;
pub mod fs_posix;   # ← supprimer
pub mod memory;
...
# Après :
pub mod fd;
pub mod memory;
...
```

**Aucun test ne devrait casser** : le fichier n'est utilisé par personne. Si `cargo build` révèle des imports cassés, c'est que le fichier était utilisé quelque part de façon indirecte — dans ce cas relever et corriger avant suppression.

---

## 3. FS-BUG-02 — `rename` implémenté, non câblé

### Preuve de l'implémentation existante

`kernel/src/fs/exofs/posix_bridge/vfs_compat.rs` ligne 802 :

```rust
pub fn vfs_rename(
    old_parent: ObjectIno,
    old_name: &[u8],
    new_parent: ObjectIno,
    new_name: &[u8],
) -> ExofsResult<()>
```

Cette fonction :
- Valide que les deux parents sont des répertoires
- Vérifie l'existence de la source dans `DIRECTORY_REGISTRY`
- Appelle `DIRECTORY_REGISTRY.rename()`
- Met à jour l'inode source dans `INODE_EMULATION`

C'est une implémentation complète. `SYS_RENAME = 82` et `SYS_RENAMEAT = 264` tombent dans `_ => sys_enosys`.

### Correction — Étape 1 : ajouter `fs_rename` dans `fs_bridge.rs`

```rust
/// `rename(oldpath, newpath)`.
///
/// Déplace/renomme un fichier ou répertoire. Les deux chemins doivent être
/// dans le même point de montage. Ne supporte pas les déplacements cross-device.
pub fn fs_rename(
    old_path: &[u8],
    new_path: &[u8],
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;

    // Résoudre les deux chemins → (parent_ino, name)
    let (old_parent, old_name) = resolve_parent_and_name(old_path)?;
    let (new_parent, new_name) = resolve_parent_and_name(new_path)?;

    use crate::fs::exofs::posix_bridge::vfs_compat::vfs_rename;
    vfs_rename(old_parent, old_name, new_parent, new_name)
        .map(|_| 0i64)
        .map_err(exofs_to_bridge_error)
}

/// `renameat(olddirfd, oldpath, newdirfd, newpath)`.
///
/// Variante AT : les chemins relatifs sont résolus depuis les dirfd fournis.
/// Pour l'instant, si les dirfd valent AT_FDCWD (-100), déléguer à fs_rename.
pub fn fs_renameat(
    olddirfd: i32,
    old_path: &[u8],
    newdirfd: i32,
    new_path: &[u8],
    pid: u32,
) -> Result<i64, FsBridgeError> {
    const AT_FDCWD: i32 = -100;
    if olddirfd != AT_FDCWD || newdirfd != AT_FDCWD {
        // Chemins relatifs à un dirfd : nécessite dirfd → ObjectIno
        // Phase 2 : implémenter resolve_from_dirfd()
        return Err(FsBridgeError::Invalid);
    }
    fs_rename(old_path, new_path, pid)
}
```

**Fonction utilitaire à ajouter dans `fs_bridge.rs`** :

```rust
/// Résout un chemin POSIX en (parent_ino, composant_nom).
/// Utilisé par rename, link, symlink, unlink, mkdir.
fn resolve_parent_and_name(path: &[u8])
    -> Result<(ObjectIno, &[u8]), FsBridgeError>
{
    // Trouver le dernier '/'
    let sep = path.iter().rposition(|&b| b == b'/');
    let (parent_path, name) = match sep {
        Some(i) if i > 0 => (&path[..i], &path[i+1..]),
        Some(0)          => (b"/" as &[u8], &path[1..]),
        None             => (b"." as &[u8], path),
        Some(_)          => return Err(FsBridgeError::BadPath),
    };
    if name.is_empty() || name.len() > 255 {
        return Err(FsBridgeError::BadPath);
    }
    // Résoudre le répertoire parent
    use crate::fs::exofs::path::path_index::PATH_INDEX;
    let parent_blob = PATH_INDEX
        .resolve(parent_path)
        .ok_or(FsBridgeError::NotFound)?;
    use crate::fs::exofs::posix_bridge::inode_emulation::INODE_EMULATION;
    let parent_ino = INODE_EMULATION
        .blob_to_ino(parent_blob)
        .ok_or(FsBridgeError::NotFound)?;
    Ok((parent_ino, name))
}
```

### Correction — Étape 2 : câbler dans `table.rs`

```rust
// Dans get_handler(), section "I/O, Fichiers" :
SYS_RENAME => {
    |old_ptr: u64, new_ptr: u64, _a3, _a4, _a5, _a6| -> i64 {
        stat_inc(SYS_RENAME);
        let old = match read_user_path(old_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
        let new = match read_user_path(new_ptr)  { Ok(p) => p, Err(e) => return e.to_errno() };
        let pid = crate::syscall::fast_path::syscall_current_pid();
        use crate::syscall::fs_bridge;
        fs_bridge::bridge_result(fs_bridge::fs_rename(old.as_bytes(), new.as_bytes(), pid))
    } as SyscallHandler
},
SYS_RENAMEAT => {
    |oldfd: u64, old_ptr: u64, newfd: u64, new_ptr: u64, _a5, _a6| -> i64 {
        stat_inc(SYS_RENAMEAT);
        let old = match read_user_path(old_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
        let new = match read_user_path(new_ptr)  { Ok(p) => p, Err(e) => return e.to_errno() };
        let pid = crate::syscall::fast_path::syscall_current_pid();
        use crate::syscall::fs_bridge;
        fs_bridge::bridge_result(fs_bridge::fs_renameat(
            oldfd as i32, old.as_bytes(), newfd as i32, new.as_bytes(), pid,
        ))
    } as SyscallHandler
},
```

---

## 4. FS-BUG-03 — `truncate`/`ftruncate` implémentés, non câblés

### Preuve de l'implémentation existante

`vfs_compat.rs` ligne 865 — `vfs_truncate(ino: ObjectIno, new_size: u64)` : implémentation complète avec extension (remplissage zéro) et troncature (`.truncate()`). Persistance via `persist_inode_data()`.

`SYS_TRUNCATE = 76` et `SYS_FTRUNCATE = 77` tombent dans `_ => sys_enosys`.

### Correction — `fs_bridge.rs`

```rust
/// `truncate(path, length)` — tronque ou étend un fichier par chemin.
pub fn fs_truncate(path: &[u8], length: i64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    if length < 0 { return Err(FsBridgeError::Invalid); }
    let _ = pid;
    // Résoudre le chemin → ino
    use crate::fs::exofs::path::path_index::PATH_INDEX;
    use crate::fs::exofs::posix_bridge::inode_emulation::INODE_EMULATION;
    use crate::fs::exofs::posix_bridge::vfs_compat::vfs_truncate;
    let blob = PATH_INDEX.resolve(path).ok_or(FsBridgeError::NotFound)?;
    let ino  = INODE_EMULATION.blob_to_ino(blob).ok_or(FsBridgeError::NotFound)?;
    vfs_truncate(ino, length as u64)
        .map(|_| 0i64)
        .map_err(exofs_to_bridge_error)
}

/// `ftruncate(fd, length)` — tronque ou étend un fichier ouvert.
pub fn fs_ftruncate(fd: u32, length: i64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    if length < 0 { return Err(FsBridgeError::Invalid); }
    let _ = pid;
    // Récupérer l'ino depuis la table des fd ouverts
    let entry = OBJECT_TABLE.get(fd).map_err(exofs_to_bridge_error)?;
    use crate::fs::exofs::posix_bridge::vfs_compat::vfs_truncate;
    // entry.object_id est un ObjectId ; on a besoin de l'ObjectIno
    use crate::fs::exofs::posix_bridge::inode_emulation::INODE_EMULATION;
    let ino = INODE_EMULATION
        .object_id_to_ino(entry.object_id)
        .ok_or(FsBridgeError::BadFd)?;
    vfs_truncate(ino, length as u64)
        .map(|_| 0i64)
        .map_err(exofs_to_bridge_error)
}
```

### Correction — `table.rs`

```rust
SYS_TRUNCATE => {
    |path_ptr: u64, length: u64, _a3, _a4, _a5, _a6| -> i64 {
        stat_inc(SYS_TRUNCATE);
        let path = match read_user_path(path_ptr) { Ok(p) => p, Err(e) => return e.to_errno() };
        let pid  = crate::syscall::fast_path::syscall_current_pid();
        use crate::syscall::fs_bridge;
        fs_bridge::bridge_result(fs_bridge::fs_truncate(path.as_bytes(), length as i64, pid))
    } as SyscallHandler
},
SYS_FTRUNCATE => {
    |fd: u64, length: u64, _a3, _a4, _a5, _a6| -> i64 {
        stat_inc(SYS_FTRUNCATE);
        let pid = crate::syscall::fast_path::syscall_current_pid();
        use crate::syscall::fs_bridge;
        fs_bridge::bridge_result(fs_bridge::fs_ftruncate(fd as u32, length as i64, pid))
    } as SyscallHandler
},
```

---

## 5. FS-BUG-04 — `flock` : `FCNTL_LOCK_TABLE` implémenté, non exposé

### Preuve de l'implémentation existante

`kernel/src/fs/exofs/posix_bridge/fcntl_lock.rs` expose :

```rust
impl FcntlLockTable {
    pub fn acquire(&self, lock: ByteRangeLock) -> ExofsResult<()>
    pub fn release(&self, object_id: u64, pid: u64) -> ExofsResult<()>
    pub fn release_all_pid(&self, pid: u64)
    pub fn test_lock(&self, candidate: &ByteRangeLock) -> ExofsResult<Option<LockInfo>>
}
pub static FCNTL_LOCK_TABLE: FcntlLockTable = ...;
pub fn make_lock(object_id, pid, kind, start, len) -> ByteRangeLock
pub fn validate_lock(lock: &ByteRangeLock) -> ExofsResult<()>
```

`SYS_FLOCK = 73` tombe dans `_ => sys_enosys`. `fs_bridge::fs_fcntl()` ne traite pas `F_SETLK`/`F_GETLK`/`F_SETLKW`.

### Correction — Étape 1 : étendre `fs_fcntl` dans `fs_bridge.rs`

```rust
// Ajouter les constantes de commandes fcntl manquantes :
const F_SETLK:  u32 = 6;
const F_SETLKW: u32 = 7;  // bloquant → traité comme SetLk (noyau non-préemptif)
const F_GETLK:  u32 = 5;

// Structure flock ABI Linux x86-64 (passée via arg = ptr userspace)
#[repr(C)]
struct FlockAbi {
    l_type:   i16,   // F_RDLCK=0 / F_WRLCK=1 / F_UNLCK=2
    l_whence: i16,   // SEEK_SET=0
    l_start:  i64,
    l_len:    i64,   // 0 = jusqu'à EOF
    l_pid:    i32,
}
const FLOCK_ABI_SIZE: usize = core::mem::size_of::<FlockAbi>();

// Dans fs_fcntl(), ajouter ces bras au match :
F_SETLK | F_SETLKW => {
    // arg = pointeur userspace vers struct flock
    let fl = read_user_typed::<FlockAbi>(arg)
        .map_err(|_| FsBridgeError::Fault)?;
    // Résoudre fd → ObjectId
    let entry = OBJECT_TABLE.get(fd).map_err(exofs_to_bridge_error)?;
    let obj_id = entry.object_id.as_u64();

    use crate::fs::exofs::posix_bridge::fcntl_lock::{
        FCNTL_LOCK_TABLE, LockKind, make_lock, validate_lock,
    };

    if fl.l_type == 2 {
        // F_UNLCK
        FCNTL_LOCK_TABLE
            .release(obj_id, pid as u64)
            .map(|_| 0i64)
            .map_err(exofs_to_bridge_error)
    } else {
        let kind = if fl.l_type == 0 { LockKind::Read } else { LockKind::Write };
        let start = fl.l_start.max(0) as u64;
        let len   = if fl.l_len == 0 { u64::MAX } else { fl.l_len as u64 };
        let lock  = make_lock(obj_id, pid as u64, kind, start, len);
        validate_lock(&lock).map_err(exofs_to_bridge_error)?;
        FCNTL_LOCK_TABLE
            .acquire(lock)
            .map(|_| 0i64)
            .map_err(exofs_to_bridge_error)
    }
}

F_GETLK => {
    let fl = read_user_typed::<FlockAbi>(arg)
        .map_err(|_| FsBridgeError::Fault)?;
    let entry = OBJECT_TABLE.get(fd).map_err(exofs_to_bridge_error)?;
    let obj_id = entry.object_id.as_u64();

    use crate::fs::exofs::posix_bridge::fcntl_lock::{
        FCNTL_LOCK_TABLE, LockKind, make_lock,
    };
    let kind = if fl.l_type == 0 { LockKind::Read } else { LockKind::Write };
    let start = fl.l_start.max(0) as u64;
    let len   = if fl.l_len == 0 { u64::MAX } else { fl.l_len as u64 };
    let candidate = make_lock(obj_id, pid as u64, kind, start, len);
    match FCNTL_LOCK_TABLE.test_lock(&candidate) {
        Ok(None) => {
            // Aucun conflit : réécrire l_type = F_UNLCK
            let unlck = FlockAbi { l_type: 2, l_whence: 0, l_start: 0, l_len: 0, l_pid: 0 };
            write_user_typed(arg, &unlck).map_err(|_| FsBridgeError::Fault)?;
            Ok(0)
        }
        Ok(Some(info)) => {
            // Conflit : remplir avec les infos du verrou existant
            let fl_out = FlockAbi {
                l_type: if info.kind == LockKind::Read { 0 } else { 1 },
                l_whence: 0,
                l_start: info.start as i64,
                l_len:   if info.len == u64::MAX { 0 } else { info.len as i64 },
                l_pid:   info.pid as i32,
            };
            write_user_typed(arg, &fl_out).map_err(|_| FsBridgeError::Fault)?;
            Ok(0)
        }
        Err(e) => Err(exofs_to_bridge_error(e)),
    }
}
```

### Correction — Étape 2 : câbler `SYS_FLOCK` dans `table.rs`

```rust
SYS_FLOCK => {
    |fd: u64, how: u64, _a3, _a4, _a5, _a6| -> i64 {
        stat_inc(SYS_FLOCK);
        // flock(2) Linux : how = LOCK_SH(1) / LOCK_EX(2) / LOCK_UN(8) / LOCK_NB(4)
        // On traduit vers fcntl F_SETLK avec start=0, len=0 (fichier entier)
        const LOCK_SH: u64 = 1;
        const LOCK_EX: u64 = 2;
        const LOCK_UN: u64 = 8;
        const LOCK_NB: u64 = 4;  // ignoré (non-bloquant par défaut)
        let how_clean = how & !LOCK_NB;
        let l_type: i16 = match how_clean {
            LOCK_SH => 0,  // F_RDLCK
            LOCK_EX => 1,  // F_WRLCK
            LOCK_UN => 2,  // F_UNLCK
            _ => return EINVAL,
        };
        // Construire un FlockAbi couvrant le fichier entier
        let fl = crate::syscall::fs_bridge::FlockAbi {
            l_type, l_whence: 0, l_start: 0, l_len: 0, l_pid: 0,
        };
        // Allouer en stack et passer via ptr → fs_fcntl lit via read_user_typed
        // Alternative propre : ajouter fs_flock() dans fs_bridge
        let pid = crate::syscall::fast_path::syscall_current_pid();
        use crate::syscall::fs_bridge;
        fs_bridge::bridge_result(fs_bridge::fs_flock(fd as u32, l_type, pid))
    } as SyscallHandler
},
```

**Note** : ajouter `pub fn fs_flock(fd: u32, l_type: i16, pid: u32)` dans `fs_bridge.rs` comme raccourci de `fs_fcntl` avec `cmd = F_SETLK` et `FlockAbi{start=0, len=0}` pour éviter de manipuler des pointeurs userspace dans `table.rs`.

---

## 6. FS-BUG-05 — `fs_fcntl` : F_SETLK/F_GETLK tombent dans `_ => Invalid`

Ce bug est résolu par la correction FS-BUG-04 ci-dessus qui ajoute les bras `F_SETLK | F_SETLKW` et `F_GETLK` dans le `match cmd` de `fs_fcntl`.

Il faut s'assurer que le bras `_ => Err(FsBridgeError::Invalid)` reste en fin de match pour les commandes vraiment non supportées.

**À ajouter dans `FsBridgeError` → `to_errno()`** :

```rust
FsBridgeError::Deadlock => -35,  // EDEADLK — verrou cyclique détecté
FsBridgeError::WouldBlock => -11, // EAGAIN — flock non-bloquant échoue
```

---

## 7. FS-GAP-01 — `poll` / `epoll` : pas d'implémentation Ring 0

### État réel

`SYS_POLL = 7`, `SYS_EPOLL_CREATE = 213`, `SYS_EPOLL_CTL = 233`, `SYS_EPOLL_WAIT = 232` sont déclarés dans `numbers.rs` et tombent dans `_ => sys_enosys`.

Il n'existe aucun fichier `poll.rs` ni `epoll.rs` dans `kernel/src/`. Aucune structure `PollFd`, aucun `wait_queue`, aucun `poll_table`.

### Ce qu'il faudrait

`poll(2)` et `epoll(2)` nécessitent un mécanisme de **notification asynchrone depuis le FS vers le processus**. ExoFS est actuellement synchrone : `vfs_read()` retourne des données ou une erreur, il n'y a pas de notion de "ce fd sera prêt plus tard".

La spec TL v5 documente `poll_wake` dans `posix_bridge/` comme un stub Ring 0 à implémenter. La conception minimale :

```rust
// kernel/src/fs/exofs/posix_bridge/poll_wake.rs (NOUVEAU)

/// Table de descripteurs en attente de readiness.
/// Associe un fd à une liste de threads à réveiller.
pub struct PollTable {
    // slots: [PollSlot; MAX_POLL_FDS]
    // wait_queue: per-slot list of ThreadId
}

/// Ajoute un fd à la poll table du thread courant.
/// Retourne immédiatement si le fd est déjà prêt.
pub fn poll_add(fd: u32, events: u16, thread_id: u32) -> ExofsResult<u16>

/// Appelé par vfs_write/vfs_read quand un objet devient prêt.
/// Réveille tous les threads qui attendent ce fd.
pub fn poll_wake(object_id: ObjectId, ready_events: u16)
```

### Plan d'implémentation (Phase 2)

1. Ajouter `poll_wake.rs` dans `posix_bridge/` avec `PollTable` et les primitives de base.
2. Appeler `poll_wake()` en fin de `vfs_write()` (pour notifier les lecteurs) et `vfs_read()` (pour notifier les writers si mode pipe).
3. Ajouter `fs_poll()` dans `fs_bridge.rs`.
4. Câbler `SYS_POLL` dans `table.rs`.
5. Pour `epoll` : ajouter un `EpollSet` par processus dans le PCB, puis `SYS_EPOLL_CREATE/CTL/WAIT`.

**Estimation** : 3-4 semaines. Bloque l'utilisation de `select(2)` et `epoll(2)` par les applications (serveurs réseau, shells interactifs).

---

## 8. FS-GAP-02 — `pipe` : pas d'implémentation

### État réel

`SYS_PIPE = 22` est déclaré, tombe dans `_ => sys_enosys`. Il n'existe aucun `pipe.rs` dans `kernel/src/`.

### Ce qu'il faudrait

Un `pipe` est une paire de fds (lecture, écriture) connectés via un buffer circulaire en mémoire. Il est indépendant de ExoFS — c'est un objet kernel anonyme, pas un objet sur disque.

```rust
// kernel/src/ipc/pipe.rs (NOUVEAU) — ou kernel/src/fs/pipe.rs

const PIPE_BUF_SIZE: usize = 65536; // POSIX minimum

pub struct PipeBuf {
    buf: [u8; PIPE_BUF_SIZE],
    read_pos:  AtomicUsize,
    write_pos: AtomicUsize,
    readers:   AtomicU32,  // nombre de fds côté lecture ouverts
    writers:   AtomicU32,  // nombre de fds côté écriture ouverts
}

pub fn pipe_create(pid: u32) -> Result<(u32, u32), KernelError>
// Retourne (fd_lecture, fd_écriture)
// Les deux fds pointent vers le même PipeBuf avec des flags différents.
```

Les fds `pipe` doivent être intégrés dans la même table de fd que les fds ExoFS (ou dans une table séparée avec un tag de type). L'intégration avec `poll_wake` est nécessaire pour que `poll(pipe_fd)` fonctionne.

**Estimation** : 1-2 semaines. Bloque les shells (`cmd1 | cmd2`), les redirections, et de nombreux programmes POSIX.

---

## 9. FS-GAP-03 — `sendfile` : pas d'implémentation

### État réel

`SYS_SENDFILE = 40` déclaré, `_ => sys_enosys`. Aucune implémentation.

### Ce qu'il faudrait

`sendfile(out_fd, in_fd, offset, count)` copie des données d'un fd vers un autre en espace kernel, sans copie userspace. La spec TL v5 place `sendfile_kernel` dans `posix_bridge/` Phase 2.

```rust
// kernel/src/fs/exofs/posix_bridge/sendfile_kernel.rs (NOUVEAU)

pub fn sendfile_kernel(
    out_fd: u32,
    in_fd: u32,
    offset: Option<u64>,
    count: usize,
    pid: u32,
) -> ExofsResult<usize> {
    // 1. Lire depuis in_fd via vfs_read() dans un buffer kernel temporaire
    // 2. Écrire dans out_fd via vfs_write()
    // 3. Mettre à jour l'offset si fourni
}
```

L'implémentation naïve (read + write en kernel) est correcte pour la Phase 2. L'optimisation zero-copy (page remapping) est Phase 3.

**Estimation** : 3-5 jours. Débloque les serveurs HTTP et les transferts de fichiers.

---

## 10. FS-GAP-04 — `vfs_server` : 4 ops sur ~15 spécifiées

### État réel

`servers/vfs_server/src/main.rs` (436 lignes) implémente 4 ops IPC :
- `VFS_MOUNT (0)` : monter un FS
- `VFS_UMOUNT (1)` : démonter
- `VFS_RESOLVE (2)` : résoudre chemin → blob_id
- `VFS_OPEN (3)` : ouvrir un fichier

La spec TL v5 décrit un répertoire `ops/` avec ~15 opérations supplémentaires et un répertoire `compat/` avec procfs/sysfs stubs.

### Plan d'expansion — Phases

**Phase 1 (court terme, déblocage applicatif)** — ajouter dans `main.rs` :

```
VFS_TRUNCATE  (4) : délègue → SYS_EXOFS* → vfs_truncate
VFS_RENAME    (5) : délègue → SYS_EXOFS* → vfs_rename
VFS_STAT      (6) : résout chemin → SYS_EXOFS_OBJECT_STAT
VFS_FLOCK     (7) : délègue → fcntl_lock via SYS_FCNTL
```

Ces 4 opérations correspondent exactement aux corrections kernel FS-BUG-02 à 05. Le vfs_server est le relais Ring 1 des mêmes primitives.

**Phase 2 (moyen terme)** — créer le répertoire `ops/` :

```
servers/vfs_server/src/ops/
  truncate.rs   : VFS_TRUNCATE
  rename.rs     : VFS_RENAME
  flock.rs      : VFS_FLOCK
  poll.rs       : VFS_POLL (dépend FS-GAP-01)
  pipe.rs       : VFS_PIPE (dépend FS-GAP-02)
  sendfile.rs   : VFS_SENDFILE (dépend FS-GAP-03)
  statx.rs      : VFS_STATX
  fallocate.rs  : VFS_FALLOCATE
```

**Phase 3 (long terme)** — créer `compat/` :

```
servers/vfs_server/src/compat/
  procfs.rs  : /proc/self/*, /proc/[pid]/*
  sysfs.rs   : /sys/block/*, /sys/class/*
  statfs.rs  : statfs(2) / statfs64(2)
```

### Correction immédiate — vfs_server Phase 1

```rust
// Ajouter dans servers/vfs_server/src/main.rs

const VFS_TRUNCATE: u32 = 4;
const VFS_RENAME:   u32 = 5;
const VFS_STAT:     u32 = 6;
const VFS_FLOCK:    u32 = 7;

fn handle_truncate(payload: &[u8]) -> VfsReply {
    // payload: [path_len:u32][path:...][new_size:u64]
    if payload.len() < 12 { return err_reply(EINVAL); }
    let path_len = u32::from_le_bytes(payload[0..4].try_into().unwrap_or_default()) as usize;
    if path_len == 0 || path_len + 12 > payload.len() { return err_reply(EINVAL); }
    let path     = &payload[4..4 + path_len];
    let new_size = u64::from_le_bytes(payload[4 + path_len..12 + path_len].try_into().unwrap_or_default());
    // Déléguer via SYS_EXOFS_PATH_RESOLVE + vfs_truncate
    let blob_id = match syscall::sys_exofs_path_resolve(path) {
        Ok(b) => b,
        Err(_) => return err_reply(ENOENT),
    };
    match syscall::sys_exofs_object_set_size(blob_id, new_size) {
        Ok(_) => ok_reply(0),
        Err(e) => err_reply(e),
    }
}

// Dans handle_request() :
VFS_TRUNCATE => handle_truncate(&req.payload),
VFS_RENAME   => handle_rename(&req.payload),
VFS_STAT     => handle_stat(&req.payload),
VFS_FLOCK    => handle_flock(&req.payload),
```

---

## 11. FS-NOTE-01 — Documentation TL v5 vs réalité

### Le problème

`ExoFS_Translation_Layer_v5_FINAL.md` se présente comme "Production-Ready · ~95% POSIX+ext4 core". L'en-tête est incorrect par rapport à l'état réel du code.

Ce que le document décrit comme existant :
- `servers/vfs_server/src/ops/` (15 fichiers) → **n'existe pas**
- `servers/vfs_server/src/compat/` → **n'existe pas**
- `kernel/src/fs/exofs/posix_bridge/flock_kernel.rs` → **n'existe pas**
- `kernel/src/fs/exofs/posix_bridge/truncate_kernel.rs` → **n'existe pas**
- (et 7 autres modules posix_bridge absents)

### Correction de documentation

Ajouter en tête de `ExoFS_Translation_Layer_v5_FINAL.md` :

```markdown
> **⚠️ AVERTISSEMENT DE COHÉRENCE — Avril 2026**
>
> Cette spec décrit l'**architecture cible**. L'implémentation actuelle en diffère :
>
> **Implémenté et fonctionnel (chemin critique) :**
> - `syscall/table.rs` → `fs_bridge.rs` : read, write, open, close, lseek, stat, fstat, lstat,
>   mkdir, rmdir, unlink, symlink, dup, dup2, fcntl (F_DUP/F_GETFL/F_SETFL), getdents64,
>   readlink, openat, symlinkat, readlinkat
> - `posix_bridge/` : vfs_compat (lookup/create/open/read/write/getattr/mkdir/unlink/rmdir/
>   rename/readdir/truncate/symlink), fcntl_lock (POSIX byte-range locks), mmap, inode_emulation
>
> **Spécifié mais non câblé (correction triviale, voir ExoOS_Corrections_11_ExoFS.md) :**
> - rename, truncate, ftruncate, flock — implémentés dans posix_bridge, pas encore dans bridge+table
>
> **Spécifié mais non implémenté (Phase 2-3) :**
> - poll, epoll, pipe, sendfile, fallocate, msync_kernel, seek_sparse, copy_range_kernel
> - vfs_server ops/ et compat/ — architecture décrite, code à écrire
>
> **Score réel actuel :** ~65% (chemin critique + posix_bridge de base)
> **Score post-corrections triviales :** ~75%
> **Score Phase 2 complète :** ~90%
```

---

## 12. Tableau de priorité et ordre d'exécution

### Graphe de dépendances

```
FS-BUG-01 (supprimer fs_posix.rs)          — indépendant, 30 min
     │
FS-BUG-02 (rename câblé)                   — indépendant, 2h
FS-BUG-03 (truncate/ftruncate câblés)       — indépendant, 1h
FS-BUG-04 + 05 (flock/fcntl câblés)        — indépendant, 2h
     │
     ├─ vfs_server Phase 1 (4 ops IPC)     — dépend FS-BUG-02/03/04, 3h
     │
FS-GAP-01 (poll_wake Ring 0)               — nouveau, 3-4 semaines
     │
     ├─ FS-GAP-02 (pipe)                   — dépend poll_wake, 1-2 semaines
     │
     ├─ vfs_server Phase 2 (ops/)          — dépend GAP-01/02/03
     │
FS-GAP-03 (sendfile)                       — semi-indépendant, 3-5 jours
```

### Tableau récapitulatif

| Réf. | Gravité | Effort estimé | Dépendances | Impact utilisateur |
|---|---|---|---|---|
| FS-BUG-01 | P1 | 30 min | — | Clarté code |
| FS-BUG-02 | P0 | 2h | — | `mv`, `rename()` fonctionnels |
| FS-BUG-03 | P0 | 1h | — | `truncate()`, fichiers temporaires |
| FS-BUG-04+05 | P0 | 2h | — | `flock()`, bases de données (SQLite) |
| vfs_server P1 | P1 | 3h | FS-BUG-02/03/04 | Cohérence Ring 1 |
| FS-NOTE-01 | P1 | 1h | — | Documentation honnête |
| FS-GAP-03 | P2 | 3-5j | — | Serveurs HTTP |
| FS-GAP-01 | P1 | 3-4 sem | — | Serveurs réseau, shells |
| FS-GAP-02 | P1 | 1-2 sem | FS-GAP-01 | Shells, pipes |
| vfs_server P2 | P2 | 4-6 sem | GAP-01/02/03 | POSIX complet |

**Effort total correctifs immédiats (FS-BUG-01 à 05 + vfs_server P1 + NOTE-01)** : ~10 heures.  
**Impact** : le score de maturité passe de ~65% à ~75%, et les applications utilisant `rename`/`truncate`/`flock` deviennent fonctionnelles (SQLite en particulier).

---

## 13. Ce qui est correct et ne doit pas être touché

- **ExoFS Core** : GC tricolore, crypto, dédup, compression, snapshot, cache, path_index — solides, ne pas modifier.
- **Chemin `read`/`write`/`open`/`close`** : câblé et fonctionnel de bout en bout. Ne pas refactoriser.
- **`SYS_EXOFS_*` (500-520)** : API native ExoFS complète et correcte. Fondation stable.
- **`fcntl_lock.rs`** : implémentation POSIX byte-range correcte avec `acquire`/`release`/`test_lock`. Ne pas réécrire — seulement câbler.
- **`vfs_compat.rs`** : toutes les fonctions présentes sont correctes. Le problème est leur absence dans la chaîne d'appel, pas leur logique interne.
- **`ZERO_BLOB_ID_4K`** : constante pré-calculée, correcte, ne pas recalculer en Ring 0.

---

*Corrections ExoFS — Couche Syscall & Translation Layer — Avril 2026*  
*Analyse directe : `kernel/src/syscall/`, `kernel/src/fs/exofs/posix_bridge/`, `servers/vfs_server/`*
