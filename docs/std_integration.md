# Intégration Complète de `library/std` (`target_os = "exo"`) dans Exo-OS

Ce document décrit l'architecture et l'implémentation de la bibliothèque standard Rust officielle (`rust-lang/rust` `library/std`) portée nativement sur Exo-OS (`target_os = "exo"`, cible `x86_64-unknown-exo.json`).

---

## 1. Frontière Architecturale `no_std` / `std`

Conformément au modèle de sécurité Zero-Trust et de tolérance aux pannes **ExoPhoenix**, la séparation entre `no_std` et `std` suit les anneaux de privilège matériels :

| Anneau / Couche | Crates & Composants | Environnement Rust | Stratégie de Panique |
| :--- | :--- | :--- | :--- |
| **Ring 0 (Noyau & Boot)** | `kernel/`, `exo-boot/` | `#![no_std]` strict (`core` + `alloc`) | `panic = "abort"` |
| **ABI & Types Partagés** | `libs/exo_types`, `libs/exo-phoenix-ssr`, `servers/syscall_abi` | `#![no_std]` strict | `panic = "abort"` |
| **Ring 1 Critique (Vagues 0–2)** | `init_server`, `ipc_router`, `memory_server`, `device_server`, `crypto_server`, pilotes matériels | `#![no_std]` (`core` + `alloc`) | `panic = "abort"` |
| **Bibliothèques Runtime** | `libs/exo-syscall`, `libs/exo-alloc`, `libs/exo-rt`, `libs/exo-libc` | `#![no_std]` (fondations bas-niveau) | `panic = "abort"` |
| **Ring 1 Haut-Niveau & Ring 3** | `servers/exosh`, `userspace/apps/coreutils` (35 binaires) | **Vrai `std` Rust** (`-Z build-std=core,alloc,std,panic_abort`) | `panic = "abort"` |

---

## 2. Spécification de la Cible `x86_64-unknown-exo.json`

Le fichier [`x86_64-unknown-exo.json`](../x86_64-unknown-exo.json) (répliqué dans [`targets/x86_64-unknown-exo.json`](../targets/x86_64-unknown-exo.json)) définit la cible compilateur pour l'espace utilisateur Exo-OS :

- **`os`** : `"exo"` (`cfg(target_os = "exo")`)
- **`llvm-target`** : `"x86_64-unknown-none-elf"`
- **`has-thread-local`** : `true` (`#[thread_local]` natif via `%fs`, modèle `initial-exec`)
- **`panic-strategy`** : `"abort"`
- **Arguments éditeur de liens (`rust-lld`)** :
  - `--no-pie`
  - `--gc-sections`
  - `-z separate-loadable-segments` (garantit que chaque segment `PT_LOAD` possède un offset fichier aligné sur 4 Kio, évitant tout partage de page fichier lors du demand paging dans `ExoFsElfLoader`)
  - `--image-base=0x0000010000000000`

---

## 3. Support Noyau (Ring 0) pour `std`

### 3.1 TLS Statique (`PT_TLS`) et `GenericTcb` (`%fs:0`)
Le chargeur ELF du noyau ([`kernel/src/fs/elf_loader_impl/mod.rs`](../kernel/src/fs/elf_loader_impl/mod.rs)) extrait le segment `PT_TLS` de l'exécutable et alloue la région TLS initiale à `0x0000_7FFF_FF50_0000` :
- Les données `.tdata` et `.tbss` sont placées aux offsets négatifs par rapport à `tls_base` (Variante II x86_64 ABI).
- Le `GenericTcb` est initialisé à `tls_base` (`MSR_FS_BASE`) :
  - `[%fs:0]` (`tls_end`) = `tls_base`
  - `[%fs:8]` (`tls_len`) = taille totale allouée
  - `[%fs:16]` (`tcb_ptr`) = auto-pointeur `tls_base`
  - `[%fs:24]` (`tcb_len`) = `PAGE_SIZE` (4096)
  - `[%fs:32]` (`os_specific`) = adresse virtuelle de la page `SignalTcb`

### 3.2 Page `SignalTcb` Partagée Kernel / Userspace (`AT_SIGNAL_TCB = 51`)
Chaque processus reçoit une page `SignalTcb` mappée avec `VmaFlags::DONTEXPAND` à `0x0000_7FFF_FF60_0000 + offset_kaslr` :
- Permet l'enregistrement de gestionnaires de signaux (`sigaction`) en **0-syscall** via `LOCK CMPXCHG16B` sur les entrées `SigActionSlot` alignées sur 16 octets.
- Permet la modification du masque de signaux (`sigprocmask`) en **0-syscall** via `AtomicU64`.
- Synchronisée avec la livraison de signaux Ring 0 ([`kernel/src/process/signal/delivery.rs`](../kernel/src/process/signal/delivery.rs)), qui sauvegarde et restaure l'état FPU/SSE (`fxsave64`/`fxrstor64`) dans la trame signal avant de sauter vers le trampoline `exo_sigreturn_trampoline` (`SYS_RT_SIGRETURN = 15`).

### 3.3 Vecteur Auxiliaire ELF (`auxv`)
La pile initiale construite dans [`kernel/src/fs/elf_loader_impl/stack.rs`](../kernel/src/fs/elf_loader_impl/stack.rs) expose :
- `AT_PHDR` (`3`), `AT_PHENT` (`4`), `AT_PHNUM` (`5`), `AT_PAGESZ` (`6`), `AT_BASE` (`7`), `AT_ENTRY` (`9`)
- `AT_RANDOM` (`25`) : 16 octets d'entropie générés sans instruction `rdrand` inconditionnelle (compatible `qemu64` sans `#UD`) pour initialiser le hachage `HashMap` de `std`.
- `AT_SIGNAL_TCB` (`51`) : pointeur vers la page `SignalTcb`.
- `AT_CAP_TOKEN` (`52`) : pointeur vers le `CapTokenUser` initial du processus.

---

## 4. Architecture de `library/std` pour Exo-OS

Le portage de `library/std` dans [`library/std/src/`](../library/std/src/) s'articule en trois couches :

### 4.1 PAL Bas-Niveau (`library/std/src/sys/pal/exo_os/`)
- [`mod.rs`](../library/std/src/sys/pal/exo_os/mod.rs) :
  - Point d'entrée assembleur `_start` dans `.ltext._start` (alignement pile 16 octets, appel de `__exo_crt_start`).
  - `__exo_crt_start` : parse `argc`, `argv`, `envp` et `auxv` via `runtime::exo_rt_init_from_stack`, initialise l'environnement (`sys::env::init_from_envp`), invoque le symbole `main` généré par `rustc` (`std::rt::lang_start`), puis termine proprement via `SYS_EXIT_GROUP` (`231`).
- [`runtime.rs`](../library/std/src/sys/pal/exo_os/runtime.rs) :
  - `GenericTcb` / `ExoTcb`, `SignalTcb`, `sigaction_0syscall` (`LOCK CMPXCHG16B`), `sigprocmask_0syscall`, `exo_sigreturn_trampoline`, `fork()`, `execve()`.
- [`syscall.rs`](../library/std/src/sys/pal/exo_os/syscall.rs) :
  - Appels système directs en assembleur inline `syscall` x86_64 couvrant les descripteurs POSIX (`read`, `write`, `open`, `close`, `stat`, `lseek`, `dup`, `dup2`, `pipe2`, `fcntl`, `getcwd`, `chdir`, `mkdir`, `rmdir`, `unlink`, `rename`, `getdents64`), la mémoire (`mmap`, `mprotect`, `munmap`, `mremap`), les processus/threads (`clone`, `fork`, `execve`, `exit`, `exit_group`, `wait4`, `kill`, `getpid`, `gettid`, `arch_prctl`, `futex`, `sched_yield`), le temps (`clock_gettime`, `nanosleep`), les sockets réseau (`socket`, `connect`, `accept`, `sendto`, `recvfrom`, `bind`, `listen`, `setsockopt`, `getsockopt`, `shutdown`), ainsi que les appels natifs ExoFS (`500..=520`), ExoPledge (`530..=531`) et ExoPhoenix (`548`).
- [`alloc_dlmalloc.rs`](../library/std/src/sys/pal/exo_os/alloc_dlmalloc.rs) :
  - Implémentation 100 % Rust de l'algorithme Doug Lea (`dlmalloc`) adossée à `SYS_MMAP`, `SYS_MREMAP` et `SYS_MUNMAP`, protégée par un spinlock atomique sur `SYS_FUTEX` et intégrant le marquage `PhoenixSafe` (`SYS_MPROTECT` avec `PROT_PHOENIX_SAFE = 0x10`). Aucun symbole C externe.
- [`futex.rs`](../library/std/src/sys/pal/exo_os/futex.rs) :
  - Primitives `futex_wait` et `futex_wake` utilisées par `std::sync::{Mutex, RwLock, Condvar, Once}` et `std::thread::park`.

### 4.2 Sous-Systèmes `library/std/src/sys/*/exo.rs`
Chaque sous-système modulaire de `std::sys` dispose d'un backend dédié pour `target_os = "exo"` :
- [`sys/alloc/exo.rs`](../library/std/src/sys/alloc/exo.rs) : `#[stable] impl GlobalAlloc for System` branché sur `alloc_dlmalloc`.
- [`sys/args/exo.rs`](../library/std/src/sys/args/exo.rs) & [`sys/env/exo.rs`](../library/std/src/sys/env/exo.rs) : `std::env::args()`, `std::env::vars()`, `getenv`, `setenv`, `unsetenv`, `getcwd`, `chdir`.
- [`sys/fd/exo.rs`](../library/std/src/sys/fd/exo.rs) : `FileDesc` sur `OwnedFd` (`read`, `write`, `read_vectored`, `write_vectored`, `duplicate`, `try_clone`, `set_cloexec`, `set_nonblocking`).
- [`sys/fs/exo.rs`](../library/std/src/sys/fs/exo.rs) : `File`, `FileAttr`, `ReadDir`, `DirEntry`, `OpenOptions`, `FilePermissions`, `FileTimes`, `FileType`, `DirBuilder`, `stat`, `lstat`, `unlink`, `rename`, `rmdir`, `readdir`.
- [`sys/io/error/exo.rs`](../library/std/src/sys/io/error/exo.rs) : décodage des codes `errno` en `io::ErrorKind`.
- [`sys/net/connection/socket/exo.rs`](../library/std/src/sys/net/connection/socket/exo.rs) : `TcpStream`, `TcpListener`, `UdpSocket`, et résolution DNS locale (`LookupHost`).
- [`sys/pipe/exo.rs`](../library/std/src/sys/pipe/exo.rs) : tubes anonymes `AnonPipe` via `SYS_PIPE2`.
- [`sys/process/exo.rs`](../library/std/src/sys/process/exo.rs) : `Command`, `Stdio`, `ChildPipe`, `Process`, `ExitStatus`, `ExitCode` via `fork()` + redirections `dup2` + `execve()` + `wait4()`.
- [`sys/random/exo.rs`](../library/std/src/sys/random/exo.rs) : génération aléatoire via `SYS_GETRANDOM` (`318`) avec repli déterministe sur `AT_RANDOM` + compteur atomique.
- [`sys/stdio/exo.rs`](../library/std/src/sys/stdio/exo.rs) : `Stdin`, `Stdout`, `Stderr` et `panic_output()` sur les descripteurs 0, 1, 2.
- [`sys/thread/exo.rs`](../library/std/src/sys/thread/exo.rs) : création de threads (`SYS_CLONE` + pile anonyme avec page de garde + initialisation `ExoTcb`), `yield_now`, `sleep`, `available_parallelism`.

### 4.3 Extensions Publiques `std::os::exo` (`library/std/src/os/exo/`)
Le module [`std::os::exo`](../library/std/src/os/exo/mod.rs) expose les capacités propres à Exo-OS aux applications `std` :
- **`std::os::exo::ffi`** : `OsStrExt`, `OsStringExt` (tranches d'octets UTF-8/brutes).
- **`std::os::exo::io`** : `AsFd`, `AsRawFd`, `BorrowedFd`, `FromRawFd`, `IntoRawFd`, `OwnedFd`, `RawFd`.
- **`CapTokenExt`** : inspection du `CapTokenUser` initial reçu via `AT_CAP_TOKEN = 52`, `clone_for_child(reduced_rights)`, `delegate(target_pid)`.
- **`ExoFsExt`** : opérations natives ExoFS sur `std::fs::File` (`snapshot`, `bptree_search`, `xattr_get`, `xattr_set`).
- **`PhoenixSafeExt`** : wrapper `PhoenixSafe<T>` allouant dans une arène marquée `PROT_PHOENIX_SAFE` (`0x10`) et déclenchant `SYS_PHOENIX_CHECKPOINT` (`548`) pour la reprise instantanée après faute.
- **`ExoPledgeExt`** : confinement Zero-Trust par processus via `pledge(promises)` (`SYS_EXO_PLEDGE = 530`) et `unveil(path, permissions)` (`SYS_EXO_UNVEIL = 531`).

---

## 5. Compilation et Intégration Système (`Makefile`)

Lors de la cible `make iso` / `make qemu` :
1. Le [`Makefile`](../Makefile) synchronise automatiquement [`library/std/`](../library/std/) vers le sysroot de la toolchain Rust nightly (`$(rustc --print sysroot)/lib/rustlib/src/rust/library/std/`).
2. `servers/exosh` et les 35 binaires de `userspace/apps/coreutils` sont compilés avec :
   ```bash
   cargo +nightly build --target x86_64-unknown-exo.json \
     -Z build-std=core,alloc,std,panic_abort \
     -Z build-std-features=compiler-builtins-mem \
     -Z json-target-spec
   ```
3. Les exécutables ELF64 produits sont intégrés dans `/bin` de l'image racine `exofs-root.img`.
