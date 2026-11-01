# Audit Approfondi #53 : Compatibilité Rust Native std, Stabilité IPC & Cohérence ExoFS

**Date :** 1er Novembre 2026  
**Auteur :** Antigravity (Advanced Agentic Coding)  
**Domaines audités :**
1. **SYSCALL / Rust std Compatibility :** `sys_newfstatat`, `sys_faccessat`, `sys_statx`, `sys_preadv2`, `sys_pwritev2`, routage de `SYS_GETDENTS`.
2. **FS Bridge / ExoFS POSIX Convergence :** `fs_statx` avec `AT_EMPTY_PATH` sur descripteurs ouverts, synchronisation bidirectionnelle des descripteurs de fichiers (`open_by_path` & `object_open` → `pcb.files`).
3. **FS Storage / Heap Coalesce :** Bornage de l'ordre buddy et validation de la plage entière du voisin dans `HeapCoalescer`.
4. **IPC Concurrency & UAF Elimination :** Drains coopératifs et comptage d'opérations actives (`active_ops`) sur `AsyncChannel` et `MpmcChannel`, élimination des retries aveugles sur erreurs RPC non-transitoires.

---

## 1. Contexte & Enjeu Rust `std`

L'environnement utilisateur Exo-OS et ses composants vitaux (shell `exosh`, utilitaires `exo_coreutils`, pile d'affichage) s'exécutent sur la véritable bibliothèque standard Rust (`rust-lang/rust library/std`, cible `target_os = "exo"`). Les primitives de la bibliothèque standard `std::fs`, `std::io`, `std::net`, `std::sync` reposent sur la conformité stricte aux sémantiques POSIX / Linux et aux appels système spécialisés d'ExoFS.

---

## 2. Vulnérabilités & Incohérences Détectées et Corrigées

### 2.1 SYSCALL : Support de `AT_EMPTY_PATH` dans `sys_newfstatat` et `sys_statx`
- **Fichiers :** `kernel/src/syscall/table/fs/path_ops.rs`, `kernel/src/syscall/fs_bridge/path_ops/metadata.rs`.
- **Problème :** `sys_newfstatat` rejetait tout appel avec `dirfd != AT_FDCWD` avec `ENOSYS`. Cependant, la bibliothèque standard Rust (`std::fs::File::metadata` et `std::os::unix::fs::MetadataExt`) appelle `newfstatat(fd, "", &stat, AT_EMPTY_PATH)` ou `statx(fd, "", AT_EMPTY_PATH, ...)` pour inspecter un fichier déjà ouvert par son descripteur sans passer par un chemin textuel.
- **Correction :**
  - Dans `sys_newfstatat`, acceptation de `AT_EMPTY_PATH` (0x1000) et `AT_NO_AUTOMOUNT` (0x800). Lorsque `path.is_empty()` et que le flag `AT_EMPTY_PATH` est positionné :
    - Si `dirfd == AT_FDCWD` : inspection du répertoire courant `.`.
    - Si `dirfd >= 0` : délégation directe à `sys_fstat(dirfd, stat_ptr)`.
  - Dans `fs_statx`, résolution du descripteur `dirfd` dans la table de fichiers du processus appelant (`resolve_fd(pid, dirfd)`) afin d'extraire le `blob_id` et de renseigner la structure `LinuxStatx`.
  - Dans `sys_faccessat`, acceptation des drapeaux `AT_EACCESS` (0x200) et `AT_SYMLINK_NOFOLLOW` (0x100) sans échouer avec `ENOSYS`.

### 2.2 SYSCALL : Sémantique de curseur dans `preadv2` et `pwritev2` (`offset = -1`)
- **Fichier :** `kernel/src/syscall/table/fs/io.rs`.
- **Problème :** Selon la spécification POSIX/Linux (man 2 `preadv2`), lorsque l'offset passé vaut `-1` (`u64::MAX`), l'opération d'I/O vectorielle doit utiliser et mettre à jour le curseur courant du fichier. Le code kernel transmettait `u64::MAX` à `fs_preadv` / `fs_pwritev`, tentant une lecture au-delà de la fin du fichier sans faire progresser le curseur.
- **Correction :** Détection de `offset == u64::MAX || (offset as i64) == -1` dans `sys_preadv2` et `sys_pwritev2` avec délégation vers `sys_readv` et `sys_writev`, garantissant la progression atomique du curseur de fichier pour les écritures/lectures séquentielles de `std::io::IoSlice`.

### 2.3 SYSCALL : Mappage de `SYS_GETDENTS` (78)
- **Fichier :** `kernel/src/syscall/table/mod.rs`.
- **Problème :** Le numéro d'appel système `SYS_GETDENTS` (78) était défini dans `numbers.rs` mais omis dans la table de dispatch du noyau, retournant `-ENOSYS` pour les runtimes ou binaires effectuant cet appel.
- **Correction :** Mappage unifié `SYS_GETDENTS | SYS_GETDENTS64 => sys_getdents64`.

### 2.4 ExoFS & POSIX Bridge : Unification des descripteurs (`open_by_path` & `object_open`)
- **Fichiers :** `kernel/src/fs/exofs/syscall/open_by_path.rs`, `kernel/src/fs/exofs/syscall/object_open.rs`.
- **Problème :** Lors de l'ouverture d'un fichier via `sys_exofs_open_by_path` (syscall 519) ou `sys_exofs_object_open` (syscall 500), le descripteur alloué dans `OBJECT_TABLE` (e.g. fd ≥ 4) n'était pas enregistré dans la table de descripteurs locale du processus (`pcb.files`). Toute tentative ultérieure d'utiliser les syscalls POSIX standards (`read`, `write`, `fstat`, `close`) avec cet identifiant échouait avec `EBADF`.
- **Correction :** Enregistrement miroir systématique via `install_process_fd_at(pid, fd, fd as u64, fd_flags)` à l'ouverture, permettant aux runtimes Rust `std` et musl d'utiliser indifféremment l'API ExoFS native et l'API POSIX sur le même descripteur.

### 2.5 ExoFS Storage : Robustesse du `HeapCoalescer`
- **Fichier :** `kernel/src/fs/exofs/storage/heap_coalesce.rs`.
- **Problèmes :**
  1. `are_buddies` : `size.checked_mul(2)` provoquait un débordement arithmétique pour `order >= 63`.
  2. `try_merge_buddies` : seule la première page (`buddy`) était vérifiée comme libre avec `map.is_free(buddy)`, alors qu'un buddy d'ordre $k$ s'étend sur $2^k$ blocs. Si le bloc adjacent était alloué, le coalesceur déclarait faussement une fusion.
- **Correction :**
  - Borne stricte `order < 63` dans `are_buddies` et vérification d'overflow sur `checked_mul`.
  - Parcours de l'intégralité des $2^k$ blocs du buddy (`for i in 0..buddy_size { if !map.is_free(buddy + i) { break; } }`) avant de valider la fusion.

### 2.6 IPC : Élimination du Use-After-Free sur `AsyncChannel` et `MpmcChannel`
- **Fichiers :** `kernel/src/ipc/channel/async.rs`, `kernel/src/ipc/channel/mpmc.rs`.
- **Problème :** Dans `async_channel_send`, `async_channel_try_recv`, `mpmc_channel_send`, `mpmc_channel_recv`, le pointeur vers le canal était extrait sous verrou puis le verrou était relâché avant d'exécuter l'opération sur une référence promue `&'static`. Une destruction concurrente (`*_destroy`) pouvait libérer le slot de la table globale et le réallouer avant la fin du `send`/`recv`.
- **Correction :**
  - Ajout du champ `pub active_ops: AtomicU32` dans `AsyncChannel` et `MpmcChannel`.
  - Incrémentation atomique (`fetch_add`) sous garde et décrémentation post-opération.
  - Dans les fonctions `destroy`, fermeture du canal suivie d'un drain coopératif (`cooperative_reschedule`) tant que `active_ops > 0` avant de libérer le slot.

### 2.7 IPC RPC : Politique de retry sur erreurs transitoires uniquement
- **Fichiers :** `kernel/src/ipc/rpc/protocol.rs`, `kernel/src/ipc/rpc/client.rs`.
- **Problème :** `RpcClient::call` exécutait aveuglément des boucles de retry avec backoff sur n'importe quel code d'erreur de retour serveur, y compris des erreurs déterministes comme `MethodNotFound` ou `Denied`.
- **Correction :** Introduction de `RpcStatus::is_retryable()`. Seules les erreurs transitoires (`ServerTimeout`, `ServerBusy`, `InternalError`) déclenchent un retry ; les erreurs déterministes retournent immédiatement l'erreur à l'appelant.

---

## 3. Résultats de la Suite de Tests

| Suite de Tests | Périmètre | Résultat |
| :--- | :--- | :--- |
| `cargo check (kernel)` | Compilation complète sans warnings | **SUCCÈS (0 erreur, 0 warning)** |
| `cargo test (ipc)` | 31 tests IPC (canaux, raw, rpc, mapping, shm) | **SUCCÈS (31/31 passed)** |
| `cargo test (fs_bridge)` | 22 tests POSIX VFS bridge | **SUCCÈS (22/22 passed)** |
| `cargo test (heap_coalesce)` | 7 tests d'allocation et coalescence buddy | **SUCCÈS (7/7 passed)** |
| `make test-drivers` | 22 tests pilotes matériels (PS2, TTY, VGA, VirtIO-Blk) | **SUCCÈS (22/22 passed)** |
| `make test-userspace` | 5 tests shell et utilitaires (exosh, coreutils, libexo) | **SUCCÈS (5/5 passed)** |
| `make test-exofs` | 2 943 tests unitaires et d'intégration ExoFS | **SUCCÈS (2 943/2 943 passed)** |

---

## 4. Synthèse

Le noyau Exo-OS offre désormais une prise en charge complète des primitives système requises par la bibliothèque standard native Rust `std` : métadonnées sur descripteurs ouverts (`newfstatat` et `statx` avec `AT_EMPTY_PATH`), entrées/sorties vectorielles avec maintien de curseur (`preadv2`/`pwritev2` à offset `-1`), parcours de répertoires (`getdents`/`getdents64`), unification absolue entre descripteurs ExoFS natifs et POSIX, et immunité contre les corruptions mémoire concurrentes dans les canaux asynchrones et MPMC.
