# `process/lifecycle/` — Cycle de vie des processus

> Sources : `kernel/src/process/lifecycle/{create,fork,exec,exit,wait,reap}.rs`

---

## Table des matières

1. [create.rs — Création d'un processus](#1-creaters--création-dun-processus)
2. [fork.rs — Duplication Copy-on-Write](#2-forkrs--duplication-copy-on-write)
3. [exec.rs — Remplacement d'image (execve)](#3-execrs--remplacement-dimage-execve)
4. [exit.rs — Terminaison](#4-exitrs--terminaison)
5. [wait.rs — Attente de terminaison (waitpid)](#5-waitrs--attente-de-terminaison-waitpid)
6. [reap.rs — Reaper kthread](#6-reapers--reaper-kthread)

---

## 1. `create.rs` — Création d'un processus

### Séquence `create_process()`

```
  1. PID_ALLOCATOR.alloc()           → obtenir un PID
  2. TID_ALLOCATOR.alloc()           → obtenir le TID du thread principal
  3. ProcessThread::new(...)         → allouer stack kernel + TCB scheduler
  4. ProcessControlBlock::new(...)   → allouer le PCB (Box<>)
  5. PROCESS_REGISTRY.insert(pid)    → enregistrer sous spinlock
  6. run_queue(cpu).enqueue(tcb)     → insérer dans la run queue (préemption désactivée)
  7. retourner ProcessHandle { pid, tid, ... }
```

### `CreateParams`

```rust
pub struct CreateParams {
    pub ppid:       Pid,           // Parent (défaut : Pid::INIT)
    pub creds:      Credentials,   // Identité (défaut : uid=1000, gid=1000)
    pub cr3:        u64,           // Espace d'adressage initial (0 = vide)
    pub addr_space: usize,         // ptr opaque UserAddressSpace
    pub policy:     SchedPolicy,   // Politique ordonnancement (Normal)
    pub priority:   Priority,      // Priorité (NORMAL_DEFAULT)
    pub target_cpu: u32,           // CPU d'enqueue (défaut : 0)
    pub fd_limit:   usize,         // Limite fds (défaut : 1024)
}
```

### `CreateError`

| Variante | Cause |
|----------|-------|
| `PidExhausted` | Plus de PIDs disponibles |
| `TidExhausted` | Plus de TIDs disponibles |
| `OutOfMemory` | Échec d'allocation stack ou PCB |
| `RegistryError` | Slot registry plein ou déjà utilisé |
| `InvalidCpu` | CPU cible hors des CPUs actifs |

### `create_kthread()`

Crée un thread kernel pur (PID=1, `KTHREAD` flag) directement inséré dans la run queue du noyau. Utilisé pour le thread idle et les kthreads internes (reaper, etc.).

---

## 2. `fork.rs` — Duplication Copy-on-Write

### `ForkFlags` — Flags CLONE_*

```rust
pub struct ForkFlags(pub u32);

impl ForkFlags {
    pub const CLONE_FILES:   u32 = 1 << 0;  // Partager les fds
    pub const CLONE_VM:      u32 = 1 << 1;  // Partager l'espace d'adressage (thread)
    pub const CLONE_SIGHAND: u32 = 1 << 2;  // Partager les handlers de signaux
    pub const CLONE_NEWPID:  u32 = 1 << 3;  // Nouveau namespace PID
    pub const VFORK:         u32 = 1 << 4;  // Parent bloqué jusqu'à exec
    pub const CLONE_THREAD:  u32 = 1 << 5;  // Thread POSIX (même TGID)
}
```

### Trait `AddressSpaceCloner` — Injection de dépendance

**Règle PROC-01** : `process/` ne doit pas importer `memory/` directement.  
`fork.rs` définit un trait que `memory/cow/` implémente et enregistre.

```rust
pub trait AddressSpaceCloner: Send + Sync {
    fn clone_cow(
        &self,
        src_cr3:       u64,
        src_space_ptr: usize,
    ) -> Result<ClonedAddressSpace, AddrSpaceCloneError>;

    /// Flush le TLB parent après marquage CoW (RÈGLE PROC-06).
    fn flush_tlb_after_fork(&self, cr3: u64);
}

// Enregistrement au démarrage par memory/ :
pub fn register_addr_space_cloner(cloner: &'static dyn AddressSpaceCloner);
```

### Séquence `do_fork(flags, parent_thread, parent_pcb)`

```
  1. PID_ALLOCATOR.alloc()               → PID fils
  2. TID_ALLOCATOR.alloc()               → TID fils
  3. ADDR_SPACE_CLONER.clone_cow()        → dupliquer l'espace d'adressage en CoW
  4. fork PCB :                           → copier creds, files (si !CLONE_FILES), etc.
  5. fork ProcessThread :                 → nouvelle stack kernel + TCB
  6. ADDR_SPACE_CLONER.flush_tlb_after_fork(parent_cr3)   ← PROC-06 / PROC-08
  7. PROCESS_REGISTRY.insert(fils_pid)
  8. run_queue.enqueue(fils_tcb)
  9. retourner (fils_pid pour parent, 0 pour fils)
```

### `ForkError`

| Variante | Cause |
|----------|-------|
| `PidExhausted` | Plus de PIDs |
| `TidExhausted` | Plus de TIDs |
| `NoAddressSpaceCloner` | `register_addr_space_cloner()` non appelé |
| `AddressSpaceCloneFailed` | CoW impossible (mémoire insuffisante) |
| `OutOfMemory` | PCB ou stack kernel |
| `RegistryError` | Registry pleine |

---

## 3. `exec.rs` — Remplacement d'image (execve)

### Trait `ElfLoader` — Injection de dépendance (PROC-01)

```rust
pub trait ElfLoader: Send + Sync {
    fn load_elf(
        &self,
        path:   &str,
        argv:   &[&str],
        envp:   &[&str],
        cr3_in: u64,
    ) -> Result<ElfLoadResult, ElfLoadError>;
}

// Enregistrement au démarrage par fs/ :
pub fn register_elf_loader(loader: &'static dyn ElfLoader);
```

### `ElfLoadResult`

```rust
pub struct ElfLoadResult {
    pub entry_point:       u64,   // Adresse d'entrée du binaire
    pub initial_stack_top: u64,   // RSP initial
    pub tls_base:          u64,   // Base TLS statique (.tdata/.tbss)
    pub tls_size:          usize,
    pub brk_start:         u64,   // Début du heap (juste après .bss)
    pub cr3:               u64,   // CR3 du nouvel espace d'adressage
    pub addr_space_ptr:    usize, // ptr opaque UserAddressSpace
}
```

### Séquence `do_execve(path, argv, envp, thread, pcb)`

```
  1. Vérifier permis (creds).
  2. ELF_LOADER.load_elf(path, argv, envp, old_cr3) → ElfLoadResult
  3. Fermer les fds O_CLOEXEC (files.close_on_exec()).
  4. Mettre à jour thread.addr (entry_point, initial_rsp, tls_base).
  5. Mettre à jour pcb (cr3, address_space, brk, flags |= EXEC_DONE).
  6. reset_signals_on_exec() : tous les handlers → SIG_DFL.
  7. Retourner vers userspace avec les registres mis à jour.
```

### `ExecError`

| Variante | Cause |
|----------|-------|
| `ElfLoadFailed(e)` | ELF non chargeable |
| `PermissionDenied` | Capabilities insuffisantes |
| `ArgListTooLong` | `E2BIG` |
| `NameTooLong` | Chemin > `PATH_MAX` |
| `ProcessExiting` | Flag `EXITING` déjà positionné |
| `NoLoader` | `register_elf_loader()` non appelé |

---

## 4. `exit.rs` — Terminaison

### `do_exit(thread, pcb, exit_code) -> !`

Termine le processus courant (ou déclenche la terminaison du groupe de threads). **Ne retourne jamais.**

```
  1. pcb.set_exiting()                      → flag EXITING (atome)
  2. pcb.exit_code.store(exit_code)
  3. pcb.dec_threads()                      → décrémenter compteur
  4. Si remaining == 0 (dernier thread) :
       • Fermer tous les fds ouverts (pcb.files.lock() + close loop)
       • send_signal_to_pid(ppid, SIGCHLD)  ← notifier le parent
       • pcb.set_state(ProcessState::Zombie)
  5. thread.set_state(TaskState::Dead)
  6. TID_ALLOCATOR.free(thread.tid)         → TID réutilisable immédiatement
  7. REAPER_QUEUE.enqueue(pid, tid)         ← libération async (PROC-07)
  8. unsafe { schedule_block(rq, tcb) }     → ne revient jamais quand Dead
  9. loop { hlt }                           → satisfaire le type `!`
```

**Règle PROC-07** : la libération du PCB et du ProcessThread est **toujours asynchrone** via le kthread reaper. Jamais de `Box::from_raw(pcb)` inline dans `do_exit()`.

### `do_exit_thread(thread, pcb, return_val) -> !`

Variante pour `pthread_exit()` : termine uniquement le thread courant.

- Stocke `return_val` dans `thread.join_result`.
- Positionne `thread.join_done = true` → réveille les éventuels `thread_join()`.
- Si `dec_threads() == 0` : délègue à `do_exit(exit_code=0)`.
- Sinon : transition `Dead` + `REAPER_QUEUE.enqueue()` + `schedule_block()`.

---

## 5. `wait.rs` — Attente de terminaison (waitpid)

### `WaitOptions`

```rust
pub const WNOHANG:    u32 = 1 << 0;  // Retour immédiat si aucun fils terminé
pub const WUNTRACED:  u32 = 1 << 1;  // Reporter les fils stoppés
pub const WCONTINUED: u32 = 1 << 2;  // Reporter les fils repris
pub const WALL:       u32 = 1 << 3;  // N'importe quel fils (pid = -1)
```

### `WaitResult`

```rust
pub struct WaitResult {
    pub pid:     Pid,
    pub wstatus: u32,       // status POSIX (exit_code << 8 pour exit normal)
    pub reason:  WaitReason, // Exited | Signaled | Stopped | Continued
}
```

Constructeurs : `WaitResult::exited(pid, code)`, `WaitResult::signaled(pid, sig, core_dumped)`.

### Algorithme `do_waitpid(caller_pid, target_pid, options, tcb)`

```
  1. Valider target_pid.
  2. Scanner PROCESS_REGISTRY :
       → trouver le premier fils de caller_pid avec état Zombie
         (ou Stopped si WUNTRACED, Continued si WCONTINUED).
  3. Si trouvé : retourner WaitResult.
  4. Si non trouvé et WNOHANG : retourner WaitError::WouldBlock.
  5. Sinon : enregistrer dans WaitTable + schedule_block() sur wait_queue.
  6. À la réception de SIGCHLD : réveiller la wait_queue → retour à 1.
```

### `WaitTable`

Table statique des parents en attente. Chaque entrée : `(parent_pid, wait_queue_slot)`.  
Réveillée par `send_signal_to_pid(ppid, SIGCHLD)` dans `do_exit()`.

### `WaitError`

| Variante | errno POSIX |
|----------|-------------|
| `NoChild` | `ECHILD` |
| `WouldBlock` | `EAGAIN` (avec `WNOHANG`) |
| `Interrupted` | `EINTR` |
| `InvalidPid` | `EINVAL` |

---

## 6. `reap.rs` — Reaper kthread

**Règle PROC-07** : la destruction des ressources d'un processus zombie est toujours réalisée par un kthread dédié, jamais inline dans `do_exit()`.

### `REAPER_QUEUE`

File d'attente statique (`AtomicRingBuffer<(Pid, Tid)>`) partagée entre tous les appelants `do_exit*()`.

```rust
pub static REAPER_QUEUE: ReaperQueue;

pub fn enqueue(pid: Pid, tid: Tid);    // appelé par exit.rs
pub fn dequeue() -> Option<(Pid, Tid)>; // appelé par kthread_reaper()
```

### `kthread_reaper()` — boucle principale

```
loop {
  attendre signal dans REAPER_QUEUE (via wait_queue ou polling 1ms)
  while let Some((pid, tid)) = REAPER_QUEUE.dequeue() {
    // 1. Retirer le PCB de la registry.
    if let Some(pcb) = PROCESS_REGISTRY.remove(pid) {
      // 2. Libérer le PID.
      PID_ALLOCATOR.free(pid.0);
      // 3. Drop le Box<PCB> → libère toute la mémoire PCB.
      drop(Box::from_raw(pcb));
    }
    // Le TID est déjà libéré dans do_exit() (immédiatement réutilisable).
  }
}
```

**Invariant** : `Box::from_raw(pcb)` est sûr ici car :
- le PCB est dans l'état `Dead` (plus aucun thread actif),
- aucun autre thread ne peut obtenir une référence (retiré de la registry),
- le reaper est le seul consommateur de `REAPER_QUEUE`.
