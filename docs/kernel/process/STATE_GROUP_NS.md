# `process/state`, `process/group`, `process/namespace`, `process/resource`

> Sources :
> - `kernel/src/process/state/{transitions,wakeup}.rs`
> - `kernel/src/process/group/{session,pgrp,job_control}.rs`
> - `kernel/src/process/namespace/{pid_ns,mount_ns,net_ns,uts_ns,user_ns}.rs`
> - `kernel/src/process/resource/{rlimit,usage,cgroup}.rs`

---

## Table des matières

1. [state/ — Machine à états et intégration DMA](#1-state--machine-à-états-et-intégration-dma)
2. [group/ — Sessions, groupes de processus et contrôle de job](#2-group--sessions-groupes-de-processus-et-contrôle-de-job)
3. [namespace/ — Espaces de noms](#3-namespace--espaces-de-noms)
4. [resource/ — Limites et métriques](#4-resource--limites-et-métriques)

---

## 1. `state/` — Machine à états et intégration DMA

### `transitions.rs` — Machine à états du PCB

#### Graphe des transitions valides

```
  Creating ──Spawn──► Running
               ┌────────────────────►─────────────────┐
  Running  ──Sleep──► Sleeping  ──Wake──► Running      │
               └──Stop──► Stopped  ──Continue──► Running│
               └──ExitToZombie──► Zombie ──ZombieToDead──► Dead
  Sleeping ──Stop──► Stopped
  Sleeping ──ExitToZombie──► Zombie
  Stopped  ──ExitToZombie──► Zombie
```

#### `StateTransition` enum

```rust
pub enum StateTransition {
    Spawn,          // Allocation terminée → prêt
    Sleep,          // Mise en attente (E/S, mutex)
    Wake,           // Réveil
    Stop,           // SIGSTOP reçu
    Continue,       // SIGCONT reçu
    ExitToZombie,   // do_exit() → attente waitpid()
    ZombieToDead,   // Récolte par le reaper
}
```

#### `transition(pcb, tr) -> Result<ProcessState, TransitionError>`

```rust
pub fn transition(
    pcb: &ProcessControlBlock,
    tr:  StateTransition,
) -> Result<ProcessState, TransitionError>;
```

Applique la transition si valide, stocke le nouvel état via `pcb.set_state()` (atome `Release`).  
Retourne `Err(TransitionError { from, transition })` si la transition est illégale.

Toute transition illégale indique un bug noyau (panic recommandé en mode debug).

---

### `wakeup.rs` — Intégration DMA (PROC-02)

**Inversion de dépendance** :
- `memory/dma/` définit le trait `DmaWakeupHandler`.
- `process/state/wakeup.rs` l'implémente (`ProcessWakeupHandler`).
- `process/mod.rs::init()` enregistre l'implémentation auprès de `memory/dma/`.

#### `DmaWakeupHandler` (défini dans `memory/`)

```rust
pub trait DmaWakeupHandler: Send + Sync {
    fn on_dma_complete(&self, tid: u64, txn_id: u64, result: i64);
    fn on_dma_error(   &self, tid: u64, txn_id: u64, err: DmaError);
}
```

#### `ProcessWakeupHandler` (implémenté dans `process/`)

```
  on_dma_complete(tid, txn_id, result) :
    1. store_completion(tid, txn_id, result) dans DMA_COMPLETIONS[].
    2. DMA_WAIT_QUEUE.notify_all() → réveille les threads en attente.

  on_dma_error(tid, txn_id, err) :
    1. store_completion(tid, txn_id, err as i64) (valeur négative = erreur).
    2. DMA_WAIT_QUEUE.notify_all().
```

#### `DMA_COMPLETIONS` — Tableau de slots (256 entrées)

```rust
struct DmaCompletionSlot {
    tid:    AtomicU64,   // TID cible (0 = libre)
    txn_id: AtomicU64,   // ID transaction
    result: AtomicI64,   // 0 = OK, < 0 = errno négatif
}
```

#### `consume_completion(tid, txn_id) -> Option<i64>`

Utilisé par le thread attendant la fin d'une transaction DMA.

```rust
// Usage typique dans un driver :
loop {
    if let Some(r) = consume_completion(tid, txn_id) {
        return if r >= 0 { Ok(r as u64) } else { Err(r) };
    }
    if tcb.has_signal_pending() { return Err(EINTR); }
    unsafe { DMA_WAIT_QUEUE.wait_interruptible(tcb as *mut _); }
}
```

---

## 2. `group/` — Sessions, groupes de processus et contrôle de job

### `session.rs` — Sessions POSIX

#### `SessionId`

```rust
pub struct SessionId(pub u32);
pub const KERNEL_SESSION: SessionId = SessionId(0);
```

#### `Session`

```rust
pub struct Session {
    pub sid:        SessionId,
    pub leader_pid: AtomicU32,   // PID du leader de session
    pub ctty:       AtomicU64,   // Terminal de contrôle (0 = aucun)
    pub refcount:   AtomicU32,
    pub valid:      AtomicU32,
}
```

#### `SessionTable` — Table statique (1024 entrées)

| Méthode | Description |
|---------|-------------|
| `create(leader: Pid) -> Option<SessionId>` | Crée une session (SID = PID du leader) |
| `find(sid) -> Option<&Session>` | Trouve par SID |
| `set_ctty(sid, tty_id)` | Associe un terminal de contrôle |
| `destroy(sid)` | Invalide la session (si refcount == 0) |

Les sessions sont créées par `setsid(2)` (syscall) et liées au PCB via `pcb.sid`.

---

### `pgrp.rs` — Groupes de processus

#### `ProcessGroup`

```rust
pub struct ProcessGroup {
    pub pgid:       Pid,         // PGID = PID du leader de groupe
    pub session_id: SessionId,   // Session parente
    pub refcount:   AtomicU32,
    pub valid:      AtomicU32,
    pub member_count: AtomicU32,
}
```

#### `GroupTable` — Table statique (4096 entrées)

| Méthode | Description |
|---------|-------------|
| `create(pgid, sid) -> Option<&ProcessGroup>` | Crée un groupe |
| `find(pgid) -> Option<&ProcessGroup>` | Lookup par PGID |
| `add_member(pgid)` / `remove_member(pgid)` | Gestion des membres |
| `destroy(pgid)` | Invalide si vide |

Lié au PCB via `pcb.pgid`. Modifié par `setpgid(2)`.

---

### `job_control.rs` — Contrôle de job POSIX

Implémente la logique POSIX de contrôle de job pour les terminaux (`tty/`).

#### Signaux impliqués

| Signal | Déclencheur |
|--------|-------------|
| `SIGTSTP` | L'utilisateur tape `Ctrl+Z` |
| `SIGCONT` | Le shell envoie le job au premier plan (`fg`) |
| `SIGTTIN` | Thread background tente de lire depuis le terminal |
| `SIGTTOU` | Thread background tente d'écrire sur le terminal |

#### API

```rust
/// Envoie SIGTSTP à tous les membres du groupe de processus.
pub fn job_stop(pgid: Pid);

/// Envoie SIGCONT à tous les membres du groupe.
pub fn job_continue(pgid: Pid);

/// Vérifie si le thread doit recevoir SIGTTIN/SIGTTOU.
/// Appelé par les drivers tty avant lecture/écriture.
pub fn check_tty_access(thread: &ProcessThread, pcb: &ProcessControlBlock, is_read: bool)
    -> Result<(), Signal>;
```

---

## 3. `namespace/` — Espaces de noms

### `pid_ns.rs` — Namespace PID

Chaque namespace PID possède son propre allocateur de PIDs (bitmap identique à `PidAllocator`).

```rust
pub struct PidNamespace {
    pub id:         u32,           // Index unique
    pub level:      u32,           // Profondeur (0 = racine)
    pub init_pid:   AtomicU32,     // PID 1 dans ce namespace
    pub pop:        AtomicU32,     // Nombre de processus vivants
    pub refcount:   AtomicU32,
    pub valid:      AtomicU32,
    pub bitmap:     NsPidBitmap,   // Allocateur local
    pub alloc_lock: SpinLock<()>,
}
```

#### `NsPidBitmap`

Identique à `PidBitmap<512>` : 32 768 PIDs par namespace, allocation par trailing-zeros + CAS.

#### `ROOT_PID_NS`

```rust
pub static ROOT_PID_NS: PidNamespace;  // level=0, id=0
```

- Les processus sans `CLONE_NEWPID` restent dans le namespace racine (`pcb.pid_ns_idx = 0`).
- Un processus dans un namespace fils voit ses descendants avec des PIDs **locaux** différents des PIDs globaux.
- Maximum : `MAX_PID_NS = 64` namespaces simultanés.

---

### `uts_ns.rs` — Namespace UTS (hostname / domainname)

```rust
pub struct UtsNamespace {
    pub id:         u32,
    pub refcount:   AtomicU32,
    pub valid:      AtomicU32,
    // UnsafeCell pour mutation via &self (accès sous spinlock)
    pub hostname:   UnsafeCell<[u8; 64]>,
    pub domainname: UnsafeCell<[u8; 64]>,
    pub lock:       SpinLock<()>,
}
```

Méthodes : `get_hostname() -> &str`, `set_hostname(s)`, `get_domainname()`, `set_domainname(s)`.  
Lié aux syscalls `gethostname(2)` / `sethostname(2)` / `getdomainname(2)` / `setdomainname(2)`.

---

### `user_ns.rs` — Namespace utilisateur

```rust
pub struct UserNamespace {
    pub id:         u32,
    pub parent_id:  u32,
    pub owner_uid:  u32,         // UID du créateur
    pub owner_gid:  u32,
    pub refcount:   AtomicU32,
    pub valid:      AtomicU32,
    // Tables de mapping UID/GID (max 5 entrées chacune)
    uid_map:        [UidGidMapEntry; 5],
    gid_map:        [UidGidMapEntry; 5],
    uid_map_count:  AtomicU32,
    gid_map_count:  AtomicU32,
    pub lock:       SpinLock<()>,
}

pub struct UidGidMapEntry {
    pub ns_id:  u32,    // UID/GID dans le namespace
    pub host_id: u32,   // UID/GID sur l'hôte
    pub count:  u32,    // Longueur de la plage
}
```

Méthodes : `map_uid_to_host(uid) -> Option<u32>`, `map_host_to_ns(uid) -> Option<u32>`.

---

### `mount_ns.rs` et `net_ns.rs` — Stubs légers

Ces modules définissent les structures minimales (`MountNamespace`, `NetNamespace`) avec leur compteur de références et leur table globale. L'implémentation complète est déléguée à `fs/vfs/` et `net/` respectivement.

```rust
pub struct MountNamespace { pub id: u32, pub refcount: AtomicU32, pub valid: AtomicU32 }
pub struct NetNamespace    { pub id: u32, pub refcount: AtomicU32, pub valid: AtomicU32 }
```

---

## 4. `resource/` — Limites et métriques

### `rlimit.rs` — Limites POSIX (`getrlimit` / `setrlimit`)

#### `RLimitKind`

```rust
#[repr(u8)]
pub enum RLimitKind {
    CPU      =  0,  FSIZE    =  1,  DATA     =  2,  STACK    =  3,
    CORE     =  4,  RSS      =  5,  NPROC    =  6,  NOFILE   =  7,
    MEMLOCK  =  8,  AS       =  9,  LOCKS    = 10,  SIGPENDING = 11,
    MSGQUEUE = 12,  NICE     = 13,  RTPRIO   = 14,  RTTIME   = 15,
}
pub const RLIM_INFINITY: u64 = u64::MAX;
```

#### `RLimit` — Paire soft/hard

```rust
pub struct RLimit { pub soft: u64, pub hard: u64 }

impl RLimit {
    pub const UNLIMITED:       Self = Self { soft: RLIM_INFINITY, hard: RLIM_INFINITY };
    pub const DEFAULT_NOFILE:  Self = Self { soft: 1024,          hard: 4096 };
    pub const DEFAULT_STACK:   Self = Self { soft: 8 * 1024*1024, hard: RLIM_INFINITY };
    pub const DEFAULT_NPROC:   Self = Self { soft: 32768,         hard: 32768 };
    pub const DEFAULT_AS:      Self = Self { soft: RLIM_INFINITY, hard: RLIM_INFINITY };
}
```

#### `RLimitTable` — Table des limites d'un processus

```rust
pub struct RLimitTable { limits: [RLimit; RLimitKind::COUNT] }
```

| Méthode | Description |
|---------|-------------|
| `new_default() -> Self` | Valeurs POSIX par défaut |
| `get(kind) -> RLimit` | Lecture |
| `set(kind, new, is_root) -> Result` | Modification avec vérification POSIX |

Règle POSIX pour `setrlimit()` :
- `new.soft <= new.hard` toujours.
- Un non-root ne peut qu'abaisser `hard`.
- Root peut augmenter `hard`.

---

### `usage.rs` — Métriques de ressources

```rust
pub struct ProcessUsage {
    pub utime_ns:    AtomicU64,  // Temps CPU userspace (ns)
    pub stime_ns:    AtomicU64,  // Temps CPU kernel (ns)
    pub maxrss:      AtomicU64,  // Résidence mémoire max (pages)
    pub page_faults: AtomicU64,  // Défauts de page
    pub io_read:     AtomicU64,  // Octets lus
    pub io_write:    AtomicU64,  // Octets écrits
    pub ctx_sw_vol:  AtomicU64,  // Context switches volontaires
    pub ctx_sw_inv:  AtomicU64,  // Context switches involontaires
    pub signals:     AtomicU64,  // Signaux reçus
}
```

Accessible via `getrusage(2)`. Mis à jour par :
- le scheduler (temps CPU),
- `memory/fault.rs` (page faults),
- les drivers I/O (octets io),
- `lifecycle/exit.rs` (accumulation des stats des threads morts).

---

### `cgroup.rs` — Control Groups (v1)

Implémentation minimale des cgroups v1 pour la gestion des quotas.

```rust
pub struct CgroupEntry {
    pub id:          u32,
    pub valid:       AtomicU32,
    pub name:        [u8; 64],    // Nom ASCII
    pub cpu_shares:  AtomicU32,   // Poids CPU relatif (défaut 1024)
    pub memory_limit: AtomicU64,  // Limite mémoire en octets (0 = illimité)
    pub pids_max:    AtomicU32,   // Max processus dans ce cgroup (0 = illimité)
    pub pid_count:   AtomicU32,   // Nombre actuel de processus
    pub refcount:    AtomicU32,
}

pub struct CgroupTable {
    entries: [CgroupEntry; MAX_CGROUPS],  // MAX_CGROUPS = 256
    lock:    SpinLock<()>,
    count:   AtomicU32,
}

pub static CGROUP_TABLE: CgroupTable;
```

| Méthode | Description |
|---------|-------------|
| `create(name) -> Option<u32>` | Crée un cgroup, retourne son ID |
| `find_by_id(id) -> Option<&CgroupEntry>` | Lookup |
| `find_by_name(name) -> Option<&CgroupEntry>` | Lookup par nom |
| `attach_process(cgroup_id, pid)` | Ajoute un processus |
| `detach_process(cgroup_id, pid)` | Retire un processus |
| `set_memory_limit(id, bytes)` | Modifie la limite mémoire |
| `set_cpu_shares(id, shares)` | Modifie le poids CPU |
| `check_pids_limit(id) -> bool` | Vérifie si la limite de PIDs est atteinte |
