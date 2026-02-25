# Compteurs et Statistiques IPC

Le sous-module `ipc/stats/` maintient des compteurs globaux de performance en temps réel, accessibles sans verrou.

## Vue d'ensemble

```
stats/
└── counters.rs   — IPC_STATS global, StatEvent, IpcStatsSnapshot
```

---

## Compteur global — `stats/counters.rs`

### Singleton

```rust
pub static IPC_STATS: IpcStatsCounter = IpcStatsCounter::new();
```

Initialisé à zéro au démarrage (BSS). Réinitialisable via `IPC_STATS.reset_all()`.

### Structure

```rust
pub struct IpcStatsCounter {
    pub msgs_sent:     AtomicU64,   // messages envoyés avec succès
    pub msgs_received: AtomicU64,   // messages reçus avec succès
    pub msgs_dropped:  AtomicU64,   // messages non livrés (file pleine, etc.)
    pub shm_allocs:    AtomicU64,   // allocations SHM
    pub shm_frees:     AtomicU64,   // libérations SHM
    pub ep_creates:    AtomicU64,   // endpoints créés
    pub ep_destroys:   AtomicU64,   // endpoints détruits
    pub cap_checks:    AtomicU64,   // vérifications capability
    pub cap_denials:   AtomicU64,   // refus capability
}
```

Tous les champs sont des `AtomicU64` avec `Ordering::Relaxed` pour les incréments. La précision absolue n'est pas garantie (les compteurs peuvent être légèrement décalés sur SMP), mais aucune valeur n'est perdue sur une durée longue.

---

## Événements mesurables

```rust
pub enum StatEvent {
    MsgSent,
    MsgReceived,
    MsgDropped,
    ShmAlloc,
    ShmFree,
    EndpointCreate,
    EndpointDestroy,
    CapCheck,
    CapDenial,
}

impl IpcStatsCounter {
    /// Incrémente un compteur d'une unité.
    #[inline(always)]
    pub fn inc(&self, event: StatEvent) {
        match event {
            StatEvent::MsgSent        => self.msgs_sent.fetch_add(1, Relaxed),
            StatEvent::MsgReceived    => self.msgs_received.fetch_add(1, Relaxed),
            StatEvent::MsgDropped     => self.msgs_dropped.fetch_add(1, Relaxed),
            StatEvent::ShmAlloc       => self.shm_allocs.fetch_add(1, Relaxed),
            StatEvent::ShmFree        => self.shm_frees.fetch_add(1, Relaxed),
            StatEvent::EndpointCreate => self.ep_creates.fetch_add(1, Relaxed),
            StatEvent::EndpointDestroy=> self.ep_destroys.fetch_add(1, Relaxed),
            StatEvent::CapCheck       => self.cap_checks.fetch_add(1, Relaxed),
            StatEvent::CapDenial      => self.cap_denials.fetch_add(1, Relaxed),
        };
    }
}
```

---

## Snapshot

```rust
pub struct IpcStatsSnapshot {
    pub msgs_sent:     u64,
    pub msgs_received: u64,
    pub msgs_dropped:  u64,
    pub shm_allocs:    u64,
    pub shm_frees:     u64,
    pub ep_creates:    u64,
    pub ep_destroys:   u64,
    pub cap_checks:    u64,
    pub cap_denials:   u64,
}

impl IpcStatsCounter {
    /// Prend un snapshot momentané de tous les compteurs.
    /// Non atomique entre les champs — convient au monitoring, pas à l'audit.
    pub fn snapshot(&self) -> IpcStatsSnapshot {
        IpcStatsSnapshot {
            msgs_sent:     self.msgs_sent.load(Relaxed),
            msgs_received: self.msgs_received.load(Relaxed),
            msgs_dropped:  self.msgs_dropped.load(Relaxed),
            shm_allocs:    self.shm_allocs.load(Relaxed),
            shm_frees:     self.shm_frees.load(Relaxed),
            ep_creates:    self.ep_creates.load(Relaxed),
            ep_destroys:   self.ep_destroys.load(Relaxed),
            cap_checks:    self.cap_checks.load(Relaxed),
            cap_denials:   self.cap_denials.load(Relaxed),
        }
    }

    /// Remet tous les compteurs à zéro (maintenance / warm restart).
    pub fn reset_all(&self) {
        self.msgs_sent.store(0, Relaxed);
        // ... tous les champs
    }
}
```

---

## Statistiques futex

Les statistiques futex IPC sont maintenues par `memory::utils::FUTEX_STATS` et exposées via `ipc/sync/futex.rs` :

```rust
pub fn futex_stats() -> FutexIpcStats {
    FutexIpcStats {
        waits_total:      FUTEX_STATS.wait_calls.load(Relaxed),
        wakes_total:      FUTEX_STATS.wake_calls.load(Relaxed),
        timeouts_total:   FUTEX_STATS.timeouts.load(Relaxed),
        value_mismatches: FUTEX_STATS.value_mismatches.load(Relaxed),
    }
}
```

Ces compteurs sont séparés de `IPC_STATS` car ils apppartiennent à la couche mémoire, pas à la couche IPC.

---

## Utilisation pratique

### Monitoring en temps réel

```rust
// Depuis un thread de monitoring / debugger kernel
loop {
    let snap = IPC_STATS.snapshot();
    kprintln!(
        "[IPC] sent={} recv={} dropped={} shm_allocs={} cap_denials={}",
        snap.msgs_sent, snap.msgs_received, snap.msgs_dropped,
        snap.shm_allocs, snap.cap_denials
    );
    scheduler::sleep_ns(1_000_000_000);  // toutes les secondes
}
```

### Calcul de débit

```rust
let before = IPC_STATS.snapshot();
let t0 = rdtsc_ns();

// ... opérations mesurées ...

let after  = IPC_STATS.snapshot();
let t1     = rdtsc_ns();
let msgs   = after.msgs_sent - before.msgs_sent;
let dt_ms  = (t1 - t0) / 1_000_000;
let mps    = msgs * 1000 / dt_ms.max(1);  // msgs/s
kprintln!("[IPC] débit = {} msgs/s", mps);
```

### Détection d'anomalies

```rust
let snap = IPC_STATS.snapshot();

if snap.msgs_dropped > 0 {
    // Files pleine → envisager d'augmenter RING_SIZE ou d'ajouter des consommateurs
    kprintln!("[IPC] AVERTISSEMENT : {} messages perdus", snap.msgs_dropped);
}

if snap.cap_denials > 100 {
    // Tentatives répétées sans droits → potentiel problème de configuration capability
    kprintln!("[IPC] ALERTE : {} refus capability", snap.cap_denials);
}
```

---

## Emplacement des incréments dans le code

| Compteur | Incrémenté dans |
|---|---|
| `msgs_sent` | `channel/sync.rs` — `sync_channel_send()` succès |
| `msgs_received` | `channel/sync.rs` — `sync_channel_recv()` succès |
| `msgs_dropped` | `channel/broadcast.rs` — abonné file pleine |
| `shm_allocs` | `shared_memory/allocator.rs` — `shm_alloc()` succès |
| `shm_frees` | `shared_memory/allocator.rs` — `shm_free()` succès |
| `ep_creates` | `endpoint/lifecycle.rs` — `endpoint_create()` succès |
| `ep_destroys` | `endpoint/lifecycle.rs` — `endpoint_destroy()` succès |
| `cap_checks` | `capability_bridge/check.rs` — `verify_ipc_access()` appelé |
| `cap_denials` | `capability_bridge/check.rs` — `verify_ipc_access()` → Err |
