# ⏰ Real-Time Scheduling

## Vue d'ensemble

Le module real-time fournit des garanties de latence pour les threads critiques.

## Classes de Priorité

```rust
pub enum RealtimePriority {
    /// Priorité maximale - interruptions désactivées
    Critical = 0,
    
    /// Haute priorité temps réel
    High = 1,
    
    /// Priorité temps réel normale
    Normal = 2,
    
    /// Soft real-time
    Soft = 3,
}
```

## Deadline Scheduling

### Structure Deadline

```rust
pub struct DeadlineTask {
    /// ID du thread
    pub thread_id: ThreadId,
    
    /// Période (en nanosecondes)
    pub period_ns: u64,
    
    /// Deadline relative (en nanosecondes)
    pub deadline_ns: u64,
    
    /// Temps CPU requis par période
    pub runtime_ns: u64,
    
    /// Prochaine deadline absolue
    pub next_deadline: u64,
}
```

### Algorithme EDF (Earliest Deadline First)

```rust
fn pick_next_realtime(&self) -> Option<ThreadId> {
    // Trier par deadline la plus proche
    self.deadline_tasks
        .iter()
        .filter(|t| t.is_ready())
        .min_by_key(|t| t.next_deadline)
        .map(|t| t.thread_id)
}
```

## Garanties de Latence

### Latency Bounds

```rust
pub struct LatencyConfig {
    /// Latence maximale acceptable (cycles)
    pub max_latency_cycles: u64,
    
    /// Action si dépassement
    pub on_miss: DeadlineMissAction,
}

pub enum DeadlineMissAction {
    /// Logger et continuer
    Log,
    /// Augmenter la priorité
    Boost,
    /// Panic (debug)
    Panic,
}
```

### Monitoring

```rust
pub struct RealtimeStats {
    /// Deadlines respectées
    pub deadlines_met: u64,
    
    /// Deadlines manquées
    pub deadlines_missed: u64,
    
    /// Latence maximale observée
    pub max_latency_ns: u64,
    
    /// Latence moyenne
    pub avg_latency_ns: u64,
}
```

## API

```rust
// Créer une tâche temps réel
let task = DeadlineTask {
    thread_id,
    period_ns: 10_000_000,      // 10ms
    deadline_ns: 8_000_000,     // 8ms
    runtime_ns: 2_000_000,      // 2ms CPU
    next_deadline: now() + 10_000_000,
};

scheduler.register_realtime(task)?;

// Configurer les garanties de latence
scheduler.set_latency_config(LatencyConfig {
    max_latency_cycles: 1000,
    on_miss: DeadlineMissAction::Boost,
});

// Obtenir les stats temps réel
let stats = scheduler.realtime_stats();
```

## Admission Control

Avant d'accepter une nouvelle tâche RT, vérifier la faisabilité:

```rust
fn admission_test(&self, new_task: &DeadlineTask) -> bool {
    let mut total_utilization = 0.0;
    
    for task in &self.deadline_tasks {
        total_utilization += task.runtime_ns as f64 / task.period_ns as f64;
    }
    
    total_utilization += new_task.runtime_ns as f64 / new_task.period_ns as f64;
    
    // Condition de Liu & Layland (EDF)
    total_utilization <= 1.0
}
```

## Priority Inheritance

Pour éviter l'inversion de priorité:

```rust
// Quand un thread RT bloque sur un mutex tenu par un thread normal
fn handle_priority_inversion(blocker: ThreadId, blocked_rt: ThreadId) {
    let rt_priority = scheduler.get_priority(blocked_rt);
    
    // Élever temporairement la priorité du bloqueur
    scheduler.boost_priority(blocker, rt_priority);
    
    // Quand le mutex est libéré, restaurer
    scheduler.restore_priority(blocker);
}
```
