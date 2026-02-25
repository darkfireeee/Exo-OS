# Primitives de Synchronisation IPC

Le module `ipc/sync/` fournit les primitives de synchronisation utilisées par les canaux, endpoints et mécanismes de rendezvous.

## Vue d'ensemble

```
sync/
├── futex.rs        — Shim IPC → memory::utils::futex_table (IPC-02)
├── wait_queue.rs   — IpcWaitQueue — wait/wake avec timeout et politiques
├── event.rs        — IpcEvent — notification one-shot
├── barrier.rs      — IpcBarrier — N participants
└── rendezvous.rs   — Point de rendez-vous symétrique
```

---

## Futex IPC — `sync/futex.rs`

### Principe fondamental (IPC-02)

`ipc/sync/futex.rs` est un **shim pur**. Il ne contient aucune logique futex. Toutes les opérations sont déléguées à `memory::utils::futex_table`.

**Rationale** : La table futex globale est partagée entre le kernel, les threads, et l'IPC. Toute implémentation locale crée deux tables désynchronisées → deadlocks impossibles à diagnostiquer.

### Types

```rust
/// Clé futex = adresse virtuelle (physmap) d'un AtomicU32 partagé.
pub struct FutexKey(u64);

impl FutexKey {
    /// Crée une clé depuis une référence atomique.
    pub fn from_addr(addr: &AtomicU32) -> Self {
        Self(addr as *const _ as u64)
    }
}

/// Résultat d'une attente futex.
pub enum WaiterState {
    Woken         = 0,  // réveillé normalement
    ValueMismatch = 1,  // valeur de l'AtomicU32 différente de expected
    Cancelled     = 2,  // annulation explicite
}

/// Statistiques IPC des futex (snapshot des compteurs memory).
pub struct FutexIpcStats {
    pub waits_total:      u64,
    pub wakes_total:      u64,
    pub timeouts_total:   u64,
    pub value_mismatches: u64,
}
```

### API

```rust
/// Attend que addr != expected.
/// Alloue FutexWaiter sur la pile, délègue à mem_futex_wait().
/// Spin-poll sur waiter.woken après retour de mem_futex_wait.
///
/// # Safety
/// Doit être appelé depuis un contexte thread valide avec un thread_id réel.
pub unsafe fn futex_wait(
    addr:      &AtomicU32,
    key:       FutexKey,
    expected:  u32,
    thread_id: u32,
    spin_max:  u32,
    wake_fn:   WakeFn,
) -> Result<WaiterState, IpcError>

/// Réveille au plus n threads en attente sur key.
pub unsafe fn futex_wake(key: FutexKey, n: u32) -> u32

/// Réveille TOUS les threads en attente sur key.
pub unsafe fn futex_wake_all(key: FutexKey) -> u32

/// Annule explicitement l'attente d'un waiter.
pub unsafe fn futex_cancel(waiter: *mut FutexWaiter)

/// Déplace max_requeue waiters de src vers dst (optimisation mutex biais).
pub unsafe fn futex_requeue(
    src:         FutexKey,
    dst:         FutexKey,
    max_wake:    u32,
    max_requeue: u32,
)

/// Retourne un snapshot des statistiques futex depuis memory::utils::FUTEX_STATS.
pub fn futex_stats() -> FutexIpcStats
```

### Modèle de délégation

```
ipc/sync/futex.rs                    memory/utils/futex_table.rs
─────────────────────────────────────────────────────────────────
futex_wait(addr, key, expected, ...)
  └── mem_futex_wait(key.0, expected, ...) ──►  FUTEX_TABLE
                                                  trouve bucket(key)
                                                  ajoute FutexWaiter
                                                  bloque le thread
  ◄── retourne
  spin-poll waiter.woken (AtomicBool)

futex_wake(key, n)
  └── mem_futex_wake(key.0, n, 0) ──────────►  FUTEX_TABLE
                                                  wake n waiters
                                                  waiter.woken = true
```

---

## File d'attente IPC — `sync/wait_queue.rs`

### Description

File d'attente de threads IPC avec timeout, politiques de réveil et statistiques intégrées. Utilisée par les canaux sync, les endpoints, et les barrières.

### Structures

```rust
/// Un seul waiter (aligné 64 octets pour éviter false sharing).
#[repr(align(64))]
pub struct IpcWaiter {
    pub thread_id:   u32,
    pub active:      AtomicBool,   // vrai si en attente
    pub woken:       AtomicBool,   // vrai si réveillé
    pub reason:      WakeReason,   // motif du réveil
    pub seq:         u64,          // numéro de séquence
    pub enqueued_at: u64,          // timestamp (ns)
    pub timeout_ns:  u64,          // 0 = pas de timeout
}

/// File d'attente (alignée 64 octets, capacité 64 waiters).
#[repr(align(64))]
pub struct IpcWaitQueue {
    pub channel_id: ChannelId,
    pub waiters:    [IpcWaiter; 64],
    pub count:      AtomicUsize,
    pub policy:     WakePolicy,
    pub stats:      WaitQueueStats,
}
```

### Politiques de réveil

```rust
pub enum WakePolicy {
    One,           // réveille un seul waiter (FIFO)
    All,           // réveille tous les waiters
    UpToN(u32),    // réveille au plus N waiters
}

pub enum WakeReason {
    Signaled,      // signal normal
    Timeout,       // timeout expiré
    Closed,        // canal fermé
    Interrupted,   // interruption externe
}
```

### API

```rust
impl IpcWaitQueue {
    /// Enregistre le thread courant comme waiter.
    /// Retourne l'index du slot alloué, ou None si file pleine.
    pub fn enqueue(&self, thread_id: u32, timeout_ns: u64, now_ns: u64) -> Option<usize>

    /// Réveille un waiter (politique One).
    pub fn wake_one(&self) -> bool

    /// Réveille tous les waiters (politique All).
    pub fn wake_all(&self) -> usize

    /// Réveille au plus n waiters.
    pub fn wake_n(&self, n: u32) -> usize

    /// Expire les waiters dont le timeout est dépassé.
    pub fn expire_timeouts(&self, now_ns: u64) -> usize
}
```

---

## Événement IPC — `sync/event.rs`

### Description

Notification one-shot : un émetteur signale un récepteur. Réinitialisable pour un usage répété.

### Modèle

```rust
pub struct IpcEvent {
    state:    AtomicU32,     // 0 = non signalé, 1 = signalé
    wait_key: FutexKey,
}

impl IpcEvent {
    pub fn new() -> Self
    pub fn signal(&self)                         // positionne state=1, wake
    pub fn wait(&self) -> Result<(), IpcError>   // bloque jusqu'à signal
    pub fn try_wait(&self) -> bool               // non bloquant
    pub fn reset(&self)                          // state=0
    pub fn is_set(&self) -> bool
}
```

### Exemple d'usage

```rust
// Thread producteur
event.signal();

// Thread consommateur
event.wait()?;         // bloque jusqu'au signal
event.reset();         // prêt pour le prochain cycle
```

---

## Barrière IPC — `sync/barrier.rs`

### Description

Synchronise N participants : tous doivent appeler `wait()` avant qu'aucun ne continue.

### Modèle

```rust
pub struct IpcBarrier {
    n_parties:  u32,
    count:      AtomicU32,    // arrivées actuelles
    generation: AtomicU32,    // génération (évite spurious wake)
    gate_key:   FutexKey,
}

impl IpcBarrier {
    pub fn new(n_parties: u32) -> Self

    /// Attend que tous les participants aient appelé wait().
    /// Retourne true pour le dernier arrivé (leader de la génération).
    pub fn wait(&self, timeout_ns: u64) -> Result<bool, IpcError>
}
```

### Modèle de génération

Chaque fois que tous les participants atteignent la barrière, la génération est incrémentée. Cela évite qu'un thread avancé d'un cycle ne réveille prématurément des threads du cycle courant (spurious wakeup).

---

## Rendezvous — `sync/rendezvous.rs`

### Description

Synchronisation symétrique : exactement deux participants se rencontrent. Ni l'un ni l'autre ne peut avancer tant que les deux ne sont pas arrivés.

### Modèle

```rust
pub struct IpcRendezvous {
    party_a:   AtomicBool,
    party_b:   AtomicBool,
    key_a:     FutexKey,
    key_b:     FutexKey,
}

impl IpcRendezvous {
    pub fn new() -> Self
    
    /// Participant A attend B.
    pub fn meet_a(&self, timeout_ns: u64) -> Result<(), IpcError>

    /// Participant B attend A.
    pub fn meet_b(&self, timeout_ns: u64) -> Result<(), IpcError>
}
```

### Différence avec la barrière

- **IpcBarrier** : N participants quelconques, roles identiques
- **IpcRendezvous** : exactement 2 participants, rôles distincts (A et B)

Le rendezvous est utilisé par le canal synchrone (`channel/sync.rs`) pour le handshake émetteur/récepteur.

---

## Tableau récapitulatif

| Primitive | Participants | Bloquant | Usage |
|---|---|---|---|
| `futex_wait/wake` | 1 waiter, 1 waker | Oui | Base de toutes les primitives |
| `IpcWaitQueue` | N waiters, 1 waker | Oui | Canaux, endpoints |
| `IpcEvent` | 1 waiter, 1 signaleur | Oui | Notification simple |
| `IpcBarrier` | N (symétrique) | Oui | Synchronisation de phase |
| `IpcRendezvous` | 2 (A et B) | Oui | Handshake rendezvous |
