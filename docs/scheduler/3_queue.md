# 🔥 Système 3-Queue

## Concept

Le scheduler utilise 3 queues classées par **temps d'exécution prédit** via EMA:

```
┌─────────────────────────────────────────────────────────────┐
│                    Scheduler 3-Queue                         │
├─────────────────────────────────────────────────────────────┤
│  HOT Queue    │ EMA < 1ms   │ Priorité 1 │ Interactif      │
│  NORMAL Queue │ 1ms-10ms    │ Priorité 2 │ Standard        │
│  COLD Queue   │ EMA > 10ms  │ Priorité 3 │ Batch/Compute   │
└─────────────────────────────────────────────────────────────┘
```

## Classification

```rust
fn classify_queue(ema_ns: u64) -> QueueType {
    if ema_ns < 1_000_000 {        // < 1ms
        QueueType::Hot
    } else if ema_ns < 10_000_000 { // < 10ms
        QueueType::Normal
    } else {
        QueueType::Cold
    }
}
```

## Ordre de Service

```rust
fn dequeue(&mut self) -> Option<Box<Thread>> {
    // Hot first (interactif)
    if let Some(t) = self.hot.pop_front() { return Some(t); }
    // Then Normal
    if let Some(t) = self.normal.pop_front() { return Some(t); }
    // Then Cold (batch)
    self.cold.pop_front()
}
```

## Avantages

### 1. Latence Interactive Minimale

Les threads interactifs (courts) sont toujours servis en premier.

### 2. Équité pour les Batch

Les threads longs ne sont pas affamés - ils obtiennent du CPU quand les queues hot/normal sont vides.

### 3. Prédiction Adaptative

L'EMA s'adapte au comportement réel du thread:
- Thread qui devient interactif → migre vers HOT
- Thread qui devient CPU-bound → migre vers COLD

## Statistiques

```rust
pub struct SchedulerStats {
    pub hot_queue_len: usize,
    pub normal_queue_len: usize,
    pub cold_queue_len: usize,
    pub total_switches: u64,
    pub total_spawns: u64,
    pub avg_switch_time_ns: u64,
}
```

## API

```rust
// Spawn un nouveau thread
let id = SCHEDULER.spawn(entry_fn, stack_size)?;

// Yield volontaire
yield_now();

// Bloquer le thread courant
block_current();

// Débloquer un thread
unblock(thread_id);

// Obtenir les stats
let stats = SCHEDULER.stats();
```
