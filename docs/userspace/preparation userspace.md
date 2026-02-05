# 🧭 Préparation Userspace — Exo-OS (Phase 1)

**Objectif:** lancer un userspace **réel, fonctionnel et propre** (aucun stub, aucune simplification).  
**Principe:** un temps long est accepté si le résultat est excellent.  
**Architecture VFS:** **kernelspace (hybride)** par défaut pour la performance. Si des mesures réelles montrent une régression, bascule vers userspace envisagée.

---

## 🚨 Golden Rules (obligatoires)

- **NO STUBS**
- **NO TODOs**
- **NO PLACEHOLDERS**
- **Code quality standards** : gestion d’erreurs complète, documentation des fonctions, tests de validation, aucune API « fake »

---

## 🔎 VFS Kernelspace — état et structure (référence)

Structure observée dans `kernel/src/fs/` :

- `vfs/` : cœur du VFS (résolution, inodes, dentry, montages)
- `pseudo_fs/` : devfs, procfs, sysfs, tmpfs
- `ipc_fs/` : pipes, sockets, symlinks
- `operations/` : buffer, locks, fdtable, cache
- `advanced/` : io_uring, zero-copy, mmap, quota, namespace, acl, notify
- `page_cache.rs` : cache I/O
- `core.rs`, `descriptor.rs`, `mod.rs` : types, traits, init
- `real_fs/` : FAT32/ext4 (actuellement commenté)

**Décision:** VFS reste en kernelspace pour l’instant. Les services userspace ne font **pas** de VFS userspace tant que les performances restent correctes.

---

## ✅ Pré‑requis kernel indispensables avant tout userspace

**Ces points doivent être fonctionnels (pas partiels) :**

### 1) I/O + FD table (bloquant)
- `FdTable` complète et utilisée par les syscalls
- `open / close / read / write` connectés au VFS
- gestion d’erreurs cohérente (errno)

### 2) exec() réel
- chargement ELF via VFS
- mapping des segments `PT_LOAD`
- stack utilisateur avec `argc/argv/envp`
- gestion des permissions mémoire

### 3) Processus de base
- `fork`, `wait4`, `exit` stables
- clonage de l’address space correct
- signal minimal (`SIGCHLD`, `SIGTERM`)

### 4) Mount de base
- `mount`, `umount` fonctionnels pour tmpfs/devfs/procfs

### 5) Console I/O
- stdout/stderr fiables
- input minimal (si requis par le shell)

---

## 🧩 Services userspace indispensables (délégation agent)

**Objectif:** démarrage minimal + validation des syscalls + base pour tests.

### P0 — Indispensable pour « boot userspace »

1) **`init`** (processus PID 1)
   - monte `/tmp` (tmpfs) si nécessaire
   - lance `fs_service` puis `shell`
   - gère `SIGCHLD` (reaper)

2) **`fs_service` (daemon minimal)**
   - **Phase 1:** simple présence + monitoring
   - prépare la migration future si VFS passe en userspace

3) **`shell`** (existant, à finaliser)
   - commandes de base pour valider VFS, process et signaux

4) **`tests userspace`** (binaires C/Rust)
   - `test_hello`, `test_args`, `test_fork`, `test_pipe`, `test_signals`, `test_mount`

### P1 — Services utiles après stabilisation P0

5) **`service_registry`** (existant à durcir)
6) **`logger` userspace** (si nécessaire, sinon kernel log)
7) **`net_service`** (seulement après P0 complet)

---

## 🧱 Modules userspace nécessaires (API & libs)

### Modules exo_std requis (fonctionnels)
- `process` : fork/exec/wait4/getpid/exit
- `fs` : open/read/write/close/mount/umount/dir
- `signal` : signal/kill/pause
- `io` : stdin/stdout/stderr

### Toolchain userspace
- musl libc (statique)
- compile userspace tests via `musl-gcc -static`

---

## ✅ Liste des fonctionnalités minimum pour démarrer l’userspace

**Fonctionnalités kernel nécessaires (priorité absolue) :**

1. VFS path resolution cohérente
2. FdTable + syscalls I/O complets
3. exec() ELF depuis VFS
4. fork/wait/exit stables
5. signaux de base
6. mount/umount pour tmpfs/devfs/procfs

**Fonctionnalités userspace minimales :**

1. init (PID 1)
2. fs_service minimal
3. shell fonctionnel
4. binaires de test (C/Rust)

---

## 🧭 Plan d’action priorisé (sans timeline figée)

### Phase A — Kernel Readiness (bloquant)
- FdTable + sys_read/write/open/close
- exec() VFS ELF
- fork/wait/exit + address space
- mount/umount

**Gate A:** `exec /bin/test_hello` doit afficher un message

### Phase B — Userspace minimal (P0)
- implémenter `init`
- implémenter `fs_service` minimal
- finaliser `shell`
- ajouter binaires tests

**Gate B:** boot → init → fs_service → shell

### Phase C — Tests de validation
- tests utilisateurs exécutables depuis le shell
- vérification I/O, signals, mount, pipe

**Gate C:** tous les tests passent sans crash

---

## 🧪 Tests de validation obligatoires

- `test_hello` : stdout OK
- `test_args` : argv/envp OK
- `test_fork` : fork + wait + exit code
- `test_pipe` : IPC pipe read/write
- `test_signals` : handler + SIGTERM
- `test_mount` : mount/umount tmpfs

---

## 📦 Livrables attendus (Phase 1)

- `/init` fonctionnel (PID 1)
- `/fs_service` (daemon minimal)
- `/shell` complet
- `/bin/test_*` exécutables
- Documentation des services (1 page chacun)

---

## ✅ Critères d’acceptation

- Aucun stub, aucune fonction vide
- Aucun commentaire « TODO » dans le userspace
- Gestion des erreurs documentée
- Tests passent en QEMU sans intervention manuelle

---

## 🔐 Qualité & gouvernance

- Revue systématique du code avant merge
- Validation par tests réels (pas de simulation)
- Journal de changements (changelog)

---

**Ce document remplace l’ancienne version.**
