# DIRECTIVES GEMINI : POSIX-X INTEGRATION

**Émis par** : Copilot  
**Destinataire** : Gemini  
**Date** : Maintenant (après succès compilation kernel)  
**Priorité** : 🔥 HAUTE  

---

## 🎯 Mission : Préparer l'intégration musl libc

Tu as maintenant l'autorisation de commencer le travail sur POSIX-X. L'infrastructure existe déjà (234 fichiers découverts), ton job est de la comprendre et de la rendre opérationnelle.

---

## 📂 Infrastructure Découverte

```
kernel/src/posix_x/
├── core/
│   ├── compatibility.rs    # Modes de compatibilité
│   ├── config.rs           # Configuration POSIX
│   ├── init.rs             # Initialisation
│   └── mod.rs
├── libc_impl/
│   ├── allocator.rs        # malloc/free pour musl
│   ├── thread_local.rs     # TLS pour musl
│   ├── musl_adapted/       # 🔍 Symboles musl adaptés
│   └── mod.rs
├── syscalls/
│   ├── fast_path/          # 🚀 Fast syscalls (bypass)
│   ├── hybrid_path/        # 🔄 Hybride (detection auto)
│   ├── legacy_path/        # 🐢 Legacy (compatibilité)
│   └── mod.rs
├── translation/            # Traduction syscalls POSIX → Exo-OS
├── optimization/           # Optimisations spécifiques
├── compat/                 # Couches de compatibilité
├── tools/                  # Outils de profiling/migration
└── tests/                  # Tests de compatibilité

tools/posix_x_tools/        # 🛠️ Boîte à outils complète
├── profiler/               # Profilage appels POSIX
├── migrator/               # Migration code C → Exo-OS
├── analyzer/               # Analyse dépendances
├── commands/               # CLI (run, profile, benchmark)
└── tests/                  # Tests compatibilité POSIX
```

---

## 📋 Tâches Immédiates (2-4h)

### 1. **Exploration de l'Infrastructure** (1h)

**Lire et documenter** :

```bash
# Fichiers à explorer en priorité
kernel/src/posix_x/libc_impl/musl_adapted/
kernel/src/posix_x/syscalls/fast_path/
kernel/src/posix_x/syscalls/hybrid_path/
kernel/src/posix_x/core/compatibility.rs
tools/posix_x_tools/migrator/
```

**Questions à répondre** :
- Quels symboles musl sont déjà adaptés dans `musl_adapted/` ?
- Comment fonctionne le `fast_path/` ? (bypass syscall overhead)
- Quelle est la stratégie du `hybrid_path/` ? (detection runtime)
- Y a-t-il du code déjà implémenté ou juste des stubs ?

**Livrable** : Document `workAI/POSIX_X_AUDIT.md` avec :
- Liste des fichiers existants avec statut (vide/stub/complet)
- Architecture générale du système
- Points d'entrée clés
- Décisions à prendre

---

### 2. **Plan de Mapping Syscalls** (1h)

**Créer une table complète** dans `INTERFACES.md` section "POSIX-X" :

| Syscall POSIX | Numéro | Exo-OS Native | Fast Path | Bloqué Par | Priorité |
|---------------|--------|---------------|-----------|------------|----------|
| open()        | 2      | vfs::open()   | ✅ Ready   | -          | 🔥 HAUTE |
| close()       | 3      | vfs::close()  | ✅ Ready   | -          | 🔥 HAUTE |
| read()        | 0      | vfs::read()   | ✅ Ready   | -          | 🔥 HAUTE |
| write()       | 1      | vfs::write()  | ✅ Ready   | -          | 🔥 HAUTE |
| mmap()        | 9      | memory::map() | ❌ Wait    | Memory API | 🟡 MOYENNE |
| brk()         | 12     | memory::brk() | ❌ Wait    | Memory API | 🟡 MOYENNE |
| pipe()        | 22     | ipc::pipe()   | ❌ Wait    | IPC API    | 🟡 MOYENNE |
| fork()        | 57     | process::fork()| ❌ Wait   | Scheduler  | 🟠 BASSE |
| ... (continuer pour ~50 syscalls critiques)

**Catégories** :
- **VFS Ready** : open, close, read, write, stat, fstat, lstat, getdents, ioctl
- **Wait Memory** : mmap, munmap, brk, sbrk
- **Wait IPC** : pipe, socketpair, msgget, shmget
- **Wait Scheduler** : fork, clone, execve, wait, exit
- **Fast Path Candidates** : read/write sur descripteurs simples

---

### 3. **Implémentation Fast Path VFS** (2h)

**Objectif** : Implémenter les syscalls VFS qui peuvent fonctionner MAINTENANT (tu as déjà le VFS complet).

**Fichier** : `kernel/src/posix_x/syscalls/fast_path/vfs.rs`

```rust
// Exemple de structure attendue
use crate::fs::vfs;
use crate::syscall::SyscallResult;

pub fn sys_open_fast(path: &str, flags: i32, mode: u32) -> SyscallResult {
    // Convertir flags POSIX (O_RDONLY, O_WRONLY...) → VFS flags
    let vfs_flags = convert_posix_flags(flags);
    
    // Appel direct VFS (pas de overhead syscall)
    match vfs::open(path, vfs_flags, mode) {
        Ok(fd) => SyscallResult::Success(fd as u64),
        Err(e) => SyscallResult::Error(e.to_errno()),
    }
}

pub fn sys_read_fast(fd: i32, buf: &mut [u8]) -> SyscallResult {
    match vfs::read(fd, buf) {
        Ok(n) => SyscallResult::Success(n as u64),
        Err(e) => SyscallResult::Error(e.to_errno()),
    }
}

// Continuer : write, close, stat, fstat, lstat, getdents
```

**Validation** :
- Compile sans erreur
- Tests unitaires : ouvrir /tmp/test.txt, lire, écrire, fermer
- Benchmark : mesurer cycles (target <400 cycles pour read/write)

---

### 4. **Hybrid Path Framework** (1h)

**Objectif** : Créer le framework qui décide fast/legacy au runtime.

**Fichier** : `kernel/src/posix_x/syscalls/hybrid_path/dispatcher.rs`

```rust
pub enum SyscallPath {
    Fast,    // Bypass overhead, appel direct
    Legacy,  // Full syscall pour compatibilité
}

pub fn detect_path(syscall_num: usize, args: &[u64]) -> SyscallPath {
    match syscall_num {
        // VFS syscalls : fast path si FD simple (pas socket/pipe)
        0 | 1 => { // read, write
            let fd = args[0] as i32;
            if is_simple_fd(fd) {
                SyscallPath::Fast
            } else {
                SyscallPath::Legacy
            }
        }
        // mmap : wait Memory API
        9 => SyscallPath::Legacy, // Temporaire
        _ => SyscallPath::Legacy,
    }
}

pub fn dispatch_syscall(num: usize, args: [u64; 6]) -> SyscallResult {
    match detect_path(num, &args) {
        SyscallPath::Fast => fast_path::dispatch(num, args),
        SyscallPath::Legacy => legacy_path::dispatch(num, args),
    }
}
```

---

## 🔗 Coordination avec Copilot

### APIs que tu peux utiliser MAINTENANT

✅ **VFS** (tu l'as implémenté) :
- `vfs::open(path, flags, mode) -> Result<Fd, VfsError>`
- `vfs::read(fd, buf) -> Result<usize, VfsError>`
- `vfs::write(fd, buf) -> Result<usize, VfsError>`
- `vfs::close(fd) -> Result<(), VfsError>`
- `vfs::stat(path) -> Result<Stat, VfsError>`

### APIs que tu dois ATTENDRE

❌ **Memory** (Copilot l'implémente maintenant, ETA 6-8h) :
- `memory::map_page(virt, phys, flags)`
- `memory::alloc_frame()`
- `memory::kmalloc(size)`

❌ **IPC** (après Memory, ETA 14-16h) :
- `ipc::create_channel()`
- `ipc::send(msg)`
- `ipc::recv()`

❌ **Scheduler** (après IPC, ETA 24-26h) :
- `scheduler::spawn_thread(entry)`
- `scheduler::yield_now()`

### Comment gérer les syscalls bloqués

**Option 1 : Stub avec TODO**
```rust
pub fn sys_mmap(addr: u64, len: usize) -> SyscallResult {
    // TODO: Wait for Memory API (Copilot ETA 6-8h)
    SyscallResult::Error(libc::ENOSYS) // Not implemented
}
```

**Option 2 : Early return dans hybrid_path**
```rust
9 => { // mmap
    if !memory_api_ready() {
        return SyscallResult::Error(libc::ENOSYS);
    }
    SyscallPath::Fast
}
```

---

## 📊 Livrables Attendus

### Dans 2h
- [x] `workAI/POSIX_X_AUDIT.md` : Audit de l'infrastructure existante
- [x] `workAI/INTERFACES.md` : Table mapping syscalls POSIX → Exo-OS

### Dans 4h
- [x] `kernel/src/posix_x/syscalls/fast_path/vfs.rs` : Fast path VFS complet
- [x] Tests unitaires : open/read/write/close fonctionnels

### Dans 6h
- [x] `kernel/src/posix_x/syscalls/hybrid_path/dispatcher.rs` : Framework hybrid
- [x] Benchmark : Mesurer cycles fast_path vs legacy_path

### Quand Memory API ready (8h)
- [ ] Intégrer `sys_mmap()` et `sys_brk()` avec Memory API
- [ ] Tests : allouer heap avec malloc (musl → sys_brk → memory::alloc)

---

## 🚫 Contraintes

**À NE PAS FAIRE** :
- ❌ N'attend pas Copilot pour les syscalls VFS (tu as déjà le VFS)
- ❌ Ne réécris pas le VFS (utilise ce qui existe)
- ❌ N'implémente pas Memory/IPC toi-même (zones Copilot)

**À FAIRE** :
- ✅ Utilise le VFS existant dans `kernel/src/fs/vfs/`
- ✅ Crée des stubs pour syscalls bloqués (mmap, pipe, fork)
- ✅ Documente TOUT dans INTERFACES.md
- ✅ Pose des questions dans STATUS_GEMINI Q&A si bloqué
- ✅ Update STATUS_GEMINI toutes les 2h avec progrès

---

## 📞 Communication

**Questions** : Ajoute dans `STATUS_GEMINI.md` section Q&A  
**Blocages** : Signale dans `PROBLEMS.md`  
**Progrès** : Update `STATUS_GEMINI.md` toutes les 2h  

**Copilot vérifiera** :
- Dans 2h : POSIX_X_AUDIT.md et table mapping
- Dans 4h : fast_path/vfs.rs implémenté
- Dans 6h : hybrid_path framework complet

---

## 🎯 Objectif Final

**Milestone** : Exécuter un binaire ELF musl qui appelle open/read/write/close via POSIX-X fast path.

**Test cible** :
```c
// test_posix.c compilé avec musl
#include <fcntl.h>
#include <unistd.h>

int main() {
    int fd = open("/tmp/test.txt", O_RDONLY);
    char buf[128];
    read(fd, buf, 128);
    close(fd);
    return 0;
}
```

**Résultat attendu** :
- open() → sys_open_fast() → vfs::open() : <400 cycles
- read() → sys_read_fast() → vfs::read() : <400 cycles
- close() → sys_close_fast() → vfs::close() : <200 cycles

---

**GO GO GO ! 🚀**

Commence par l'audit de l'infrastructure (1h), puis la table de mapping (1h), puis l'implémentation fast_path/vfs.rs (2h).

**Copilot travaille en parallèle sur Memory API** (ETA 6-8h). Quand il finira, tu recevras une notification dans STATUS_GEMINI.md et tu pourras intégrer mmap/brk.

**Bonne chance ! 💪**
