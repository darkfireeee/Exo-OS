# `process/core/` — Structures fondamentales

> Sources : `kernel/src/process/core/{pid,tcb,pcb,registry}.rs`

---

## Table des matières

1. [pid.rs — Identifiants et allocateurs](#1-pidrs--identifiants-et-allocateurs)
2. [tcb.rs — ProcessThread et KernelStack](#2-tcbrs--processthread-et-kernelstack)
3. [pcb.rs — ProcessControlBlock](#3-pcbrs--processcontrolblock)
4. [registry.rs — ProcessRegistry](#4-registryrs--processregistry)

---

## 1. `pid.rs` — Identifiants et allocateurs

### Types de base

```rust
/// Identifiant de processus (PID).
pub struct Pid(pub u32);

/// Identifiant de thread (TID).
pub struct Tid(pub u32);
```

| Constante | Valeur | Rôle |
|-----------|--------|------|
| `Pid::IDLE` | `0` | Processus idle (noyau) |
| `Pid::INIT` | `1` | Init (PID 1) |
| `Pid::INVALID` | `0` | Valeur sentinelle "pas de PID" |
| `PID_MAX` | `32 767` | Maximum de PIDs simultanés |
| `TID_MAX` | `131 071` | Maximum de TIDs (4× PIDs) |
| `PID_FIRST_USABLE` | `2` | Premier PID libre (0 et 1 réservés) |
| `TID_FIRST_USABLE` | `1` | Premier TID libre |

### `PidBitmap<N>` — Allocateur bitmap

```
Mots : [u64; N]    (AtomicU64)
Bit = 1 → slot LIBRE
Bit = 0 → slot OCCUPÉ

Allocation :
  1. Parcourir les mots depuis l'offset min.
  2. trailing_zeros() → premier bit libre dans le mot courant.
  3. CAS(expected, expected & !mask) → atomiquement marquer occupé.
  4. Si CAS échoue → réessayer dans la boucle (contention rare).
```

Complexité : **O(1) amorti** — le prochain mot libre est mémorisé implicitement par la boucle.

### `PidAllocator`

```rust
pub struct PidAllocator {
    bitmap:       &'static PidBitmap<N>,
    alloc_count:  AtomicU64,   // monotone
    free_count:   AtomicU64,   // monotone
    current_used: AtomicU32,   // instantané
    peak_used:    AtomicU32,   // haute-eau
    exhausted:    AtomicU64,   // compteur d'échec d'alloc
}
```

Méthodes principales :

| Méthode | Description |
|---------|-------------|
| `alloc() -> Result<u32, PidAllocError>` | Alloue le prochain ID libre |
| `free(id: u32)` | Libère un ID (set bit à 1) |
| `stats() -> PidStats` | Retourne les compteurs de telémétrie |

### Globaux

```rust
pub static PID_ALLOCATOR: PidAllocator;  // PID_BITMAP_WORDS=512
pub static TID_ALLOCATOR: PidAllocator;  // TID_BITMAP_WORDS=2048

/// À appeler une seule fois au boot.
pub unsafe fn init(_max_pids: u32, _max_tids: u32);
// ↳ réserve PID 0 (idle) et PID 1 (init) dès l'appel.
```

---

## 2. `tcb.rs` — ProcessThread et KernelStack

### `ThreadAddress` — Layout mémoire du thread

```rust
pub struct ThreadAddress {
    pub stack_base:       u64,   // Base de la pile utilisateur
    pub stack_size:       u64,   // Taille de la pile utilisateur
    pub entry_point:      u64,   // Adresse d'entrée (userspace)
    pub initial_rsp:      u64,   // RSP initial au démarrage
    pub tls_base:         u64,   // Base TLS (GS.base userspace)
    pub pthread_ptr:      u64,   // struct pthread* (userspace)
    pub sigaltstack_base: u64,   // Base pile alternative signaux
    pub sigaltstack_size: u64,   // Taille pile alternative
}
```

Méthode : `sigaltstack_top() -> u64` = `sigaltstack_base + sigaltstack_size`.

### `KernelStack` — Pile noyau d'un thread

```rust
pub struct KernelStack {
    ptr:  NonNull<u8>,
    // taille : KSTACK_SIZE = 16 384 octets (16 KiB)
    // alignement : 16 octets
    // canari : STACK_CANARY = 0xDEAD_BEEF_CAFE_BABE (u64 au bas)
}
```

| Constante | Valeur |
|-----------|--------|
| `KSTACK_SIZE` | `16 384` (16 KiB) |
| `STACK_CANARY` | `0xDEAD_BEEF_CAFE_BABE` |

Méthodes :

| Méthode | Description |
|---------|-------------|
| `new() -> Option<Self>` | Alloue et initialise la pile (écrit le canari en bas) |
| `top_addr() -> u64` | Adresse haute de la pile (RSP initial) |
| `check_canary() -> bool` | Vérifie que le canari est intact |
| `Drop` | Libère la mémoire via `alloc::dealloc()` |

### `ProcessThread` — Thread d'un processus

Structure centrale reliant le niveau processus au TCB scheduler.

```rust
pub struct ProcessThread {
    // Identifiants
    pub pid:         Pid,
    pub tid:         Tid,

    // TCB scheduler (alloué sur le tas, pointer stable)
    pub sched_tcb:   Box<ThreadControlBlock>,   // 128 bytes, 2 cache lines
    pub kstack:      KernelStack,               // pile noyau 16 KiB

    // Adressage userspace
    pub addr:        ThreadAddress,

    // TLS
    tls_base_atomic: AtomicU64,   // GS.base actuel
    tls_key_count:   AtomicU32,   // nombre de clés TLS actives

    // État de join
    pub detached:    AtomicBool,
    pub join_done:   AtomicBool,
    pub join_result: AtomicU64,   // valeur de retour pthread_exit

    // Signaux
    pub sig_queue:    SigQueue,     // signaux standard (1..31)
    pub rt_sig_queue: RTSigQueue,   // RT signaux (32..63)
}
```

Méthodes principales :

| Méthode | Description |
|---------|-------------|
| `new(pid, tid, policy, priority, addr) -> Option<Box<Self>>` | Crée un ProcessThread complet |
| `new_kthread(pid, tid, entry) -> Option<Box<Self>>` | Crée un thread kernel pur |
| `tcb_ptr() -> *mut ThreadControlBlock` | Donne accès brut au TCB scheduler |
| `check_stack_canary() -> bool` | Vérifie le canari de la pile |
| `raise_signal_pending()` | Positionne `signal_pending=true` dans le TCB |
| `set_state(state: TaskState)` | Change l'état du TCB atomiquement |

---

## 3. `pcb.rs` — ProcessControlBlock

### `ProcessState` — États du processus

```rust
#[repr(u8)]
pub enum ProcessState {
    Creating = 0,  // En cours d'allocation
    Running  = 1,  // Actif (au moins un thread en run queue)
    Sleeping = 2,  // En attente (E/S, mutex, etc.)
    Stopped  = 3,  // Stoppé (SIGSTOP)
    Zombie   = 4,  // Terminé, en attente de waitpid()
    Dead     = 5,  // Récolté par le reaper
}
```

### Flags de processus (`process_flags`)

| Flag | Valeur | Sémantique |
|------|--------|------------|
| `FORKED` | bit 0 | Processus créé par fork |
| `EXEC_DONE` | bit 1 | execve() réussi |
| `SESSION_LEADER` | bit 2 | Leader de session |
| `DAEMON` | bit 3 | Processus daemon |
| `EXITING` | bit 4 | `do_exit()` en cours |
| `SETUID` | bit 5 | Bit setuid activé |
| `SETGID` | bit 6 | Bit setgid activé |
| `NO_DUMP` | bit 7 | Pas de core dump |
| `TRACED` | bit 8 | Tracé (ptrace) |
| `IN_PID_NS` | bit 9 | Dans un PID namespace non-racine |
| `VFORK_DONE` | bit 10 | vfork terminé (parent débloqué) |

### `Credentials` — Identité POSIX

```rust
pub struct Credentials {
    pub uid:   u32,  // Real UID
    pub gid:   u32,  // Real GID
    pub euid:  u32,  // Effective UID
    pub egid:  u32,  // Effective GID
    pub suid:  u32,  // Saved-set UID
    pub sgid:  u32,  // Saved-set GID
    pub fsuid: u32,  // FS UID (access checks)
    pub fsgid: u32,  // FS GID
}
```

Constante : `Credentials::ROOT` (tous les champs à 0).  
Méthode : `is_root() -> bool` (euid == 0).

### `OpenFileTable` — Table des descripteurs

```rust
pub struct OpenFileTable {
    fds:         Vec<Option<FileDescriptor>>,
    fd_limit:    usize,
    next_hint:   usize,       // prochain slot à essayer (optimisation)
    open_count:  AtomicU64,   // compteur monotone
    close_count: AtomicU64,
}

pub struct FileDescriptor {
    pub fd:     i32,
    pub handle: u64,  // handle opaque (géré par fs/)
    pub flags:  u32,  // O_CLOEXEC, etc.
}
```

Méthodes :

| Méthode | Signature | Description |
|---------|-----------|-------------|
| `install` | `(handle: u64, flags: u32) -> Option<i32>` | Installe un fd, retourne son numéro |
| `close` | `(fd: i32) -> Option<u64>` | Retire le fd, retourne le handle |
| `get` | `(fd: i32) -> Option<&FileDescriptor>` | Lecture sans retrait |
| `close_on_exec` | `() -> Vec<u64>` | Retire tous les fds O_CLOEXEC (pour execve) |
| `clone_for_fork` | `() -> Self` | Duplique la table (pour fork) |

### `ProcessControlBlock` — Structure principale

```rust
pub struct ProcessControlBlock {
    // ── Identifiants (lecture seule après création) ──
    pub pid:           Pid,
    pub ppid:          AtomicU32,
    pub tgid:          Pid,              // Thread Group ID = PID du leader
    pub sid:           AtomicU32,        // Session ID
    pub pgid:          AtomicU32,        // Process Group ID

    // ── État (chemin chaud, atomique) ──
    pub state:         AtomicU32,        // ProcessState as u32
    pub flags:         AtomicU32,        // process_flags::*
    pub exit_code:     AtomicU32,

    // ── Threads ──
    pub thread_count:       AtomicU32,
    pub main_thread_rawptr: AtomicPtr<ProcessThread>,

    // ── Sécurité ──
    pub creds:         SpinLock<Credentials>,

    // ── Fichiers ──
    pub files:         SpinLock<OpenFileTable>,

    // ── Mémoire ──
    pub cr3:           AtomicU64,        // CR3 physique
    pub address_space: AtomicUsize,      // ptr opaque UserAddressSpace
    pub brk:           AtomicUsize,      // heap brk courante

    // ── Statistiques (accès fréquent, AtomicU64) ──
    pub utime:         AtomicU64,        // CPU user (ns)
    pub stime:         AtomicU64,        // CPU kernel (ns)
    pub page_faults:   AtomicU64,
    pub io_bytes:      AtomicU64,

    // ── Signaux ──
    pub sig_handlers:  SpinLock<SigHandlerTable>,

    // ── Namespaces (indices dans les tables globales) ──
    pub pid_ns_idx:    u32,
    pub mnt_ns_idx:    u32,
    pub net_ns_idx:    u32,
    pub uts_ns_idx:    u32,
    pub user_ns_idx:   u32,
}
```

Méthodes de convenance :

| Méthode | Description |
|---------|-------------|
| `state() -> ProcessState` | Lit l'état courant |
| `set_state(s)` | Écrit l'état avec `Release` |
| `set_exiting()` | Positionne le flag `EXITING` |
| `dec_threads() -> u32` | Décrémente le compteur de threads, retourne le reste |
| `inc_threads()` | Incrémente le compteur |
| `main_thread_ptr() -> *mut ProcessThread` | Retourne le raw ptr du thread principal |

---

## 4. `registry.rs` — ProcessRegistry

### Principe de conception

- **Lectures lockless** : chaque slot contient un `AtomicPtr<PCB>` avec ordering `Acquire`.
- **Écritures sous spinlock** : `write_lock: SpinLock<()>` protège `insert()` et `remove()`.
- Capacité : `PID_MAX + 1 = 32 768` slots (tableau statique).

### Types

```rust
struct RegistrySlot {
    pcb_ptr:  AtomicPtr<ProcessControlBlock>,
    refcount: AtomicU32,
}

pub struct ProcessRegistry {
    slots:      *mut RegistrySlot,   // tableau à plat
    capacity:   usize,
    write_lock: SpinLock<()>,
    // compteurs telémétrie
    current_count:  AtomicU32,
    total_inserts:  AtomicU64,
    total_removes:  AtomicU64,
    total_lookups:  AtomicU64,
}

pub struct RegistryStats {
    pub current_count:  u32,
    pub total_inserts:  u64,
    pub total_removes:  u64,
    pub total_lookups:  u64,
    pub capacity:       usize,
}
```

### API

| Méthode | Signature | Complexité |
|---------|-----------|------------|
| `insert` | `(pid: Pid, pcb: *mut PCB) -> Result<(), RegistryError>` | O(1), sous lock |
| `remove` | `(pid: Pid) -> bool` | O(1), sous lock |
| `find_by_pid` | `(pid: Pid) -> Option<&PCB>` | O(1), **lockless** |
| `for_each` | `(f: impl Fn(&PCB))` | O(N) |
| `stats` | `() -> RegistryStats` | O(1) |

### Global

```rust
pub static PROCESS_REGISTRY: ProcessRegistry;
```

Initialisé au boot par `process::init()` (allocation du tableau de slots via `alloc`).

### Invariants

1. `find_by_pid()` est **safe à appeler depuis tout contexte** (interruptions comprises).
2. Le slot `pid=0` (idle) et `pid=1` (init) sont toujours présents après `init()`.
3. `remove()` ne libère **pas** le PCB — c'est la responsabilité du reaper (`lifecycle/reap.rs`).
4. Le refcount sert de compteur de références pour les lookups concurrents.
