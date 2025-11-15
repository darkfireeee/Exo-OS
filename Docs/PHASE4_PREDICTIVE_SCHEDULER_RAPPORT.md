# Phase 4 - Predictive Scheduler - Rapport Complet

**Date**: 12 janvier 2025  
**Status**: ✅ **CODE COMPLET** - Tests en cours  
**Fichiers**: 2 (predictive_scheduler.rs: 550 lignes | bench_predictive.rs: 280 lignes)

---

## 🎯 Objectifs

Créer un ordonnanceur prédictif qui optimise le scheduling basé sur l'historique d'exécution:
- **Latence scheduling**: -30 à -50% pour threads courts
- **Cache hits L1**: +20 à +40% grâce à affinity
- **Réactivité**: 2-5× amélioration pour workloads interactifs

---

## 📐 Architecture Implémentée

```
┌──────────────────────────────────────────────────────┐
│           PREDICTIVE SCHEDULER                        │
├──────────────────────────────────────────────────────┤
│                                                       │
│  1. EMA Tracking (α = 0.25)                          │
│  ┌─────────────────────────────────────┐            │
│  │ new_ema = α × new_time + (1-α) × old │            │
│  │ Réactivité modérée aux changements   │            │
│  │ RDTSC début/fin pour mesure précise  │            │
│  └─────────────────────────────────────┘            │
│                                                       │
│  2. Classification 3 Queues                          │
│  ┌─────────────────────────────────────┐            │
│  │ Hot:    EMA < 10ms    (Prio 3)      │            │
│  │ Normal: 10ms ≤ EMA < 100ms (Prio 2) │            │
│  │ Cold:   EMA ≥ 100ms   (Prio 1)      │            │
│  └─────────────────────────────────────┘            │
│           ↓ schedule_next()                          │
│  ┌─────────────────────────────────────┐            │
│  │ 1. Check Hot queue                   │            │
│  │ 2. Check Normal queue                │            │
│  │ 3. Check Cold queue                  │            │
│  │ 4. Return None (idle)                │            │
│  └─────────────────────────────────────┘            │
│                                                       │
│  3. Cache Affinity                                   │
│  ┌─────────────────────────────────────┐            │
│  │ Score = 100 si même CPU + <50ms      │            │
│  │ Décroissance linéaire après seuil    │            │
│  │ Score = 10 si autre CPU              │            │
│  └─────────────────────────────────────┘            │
│                                                       │
└──────────────────────────────────────────────────────┘
```

---

## 📊 Structures de Données

### ThreadPrediction
```rust
pub struct ThreadPrediction {
    thread_id: ThreadId,
    ema_execution_us: u64,           // EMA temps exécution
    total_executions: u64,            // Nombre total runs
    last_start_tsc: u64,              // TSC début dernière exec
    last_switch_out_tsc: u64,         // TSC fin dernière exec
    last_cpu_id: usize,               // Dernier CPU utilisé
    class: ThreadClass,               // Hot/Normal/Cold
    cache_affinity_score: u64,        // Score 0-100
}
```

**Méthodes clés**:
- `update_ema(execution_time_us)`:
  ```
  if first_execution:
      ema = execution_time
  else:
      ema = 0.25 × new_time + 0.75 × old_ema
  ```
  
- `mark_execution_start(cpu_id)`:
  - Capture `rdtsc()`
  - Stocke `cpu_id`

- `mark_execution_end()`:
  - Capture `rdtsc()`
  - Calcule `elapsed = end_tsc - start_tsc`
  - Convertit en microsecondes: `elapsed_us = elapsed_cycles / tsc_freq_mhz`
  - Appelle `update_ema(elapsed_us)`
  - Reclassifie thread selon nouveau EMA

- `calculate_cache_affinity(target_cpu, current_tsc)`:
  ```
  if target_cpu == last_cpu:
      time_since_last = (current_tsc - last_switch_out_tsc) / tsc_freq
      if time_since_last < 50ms:
          score = 100
      else:
          decay = (time_since_last - 50ms) / 1ms
          score = max(0, 100 - decay)
  else:
      score = 10
  ```

### ThreadClass
```rust
pub enum ThreadClass {
    Hot,      // < 10ms   (priorité 3)
    Normal,   // 10-100ms (priorité 2)
    Cold,     // > 100ms  (priorité 1)
}
```

**Classification automatique**:
```rust
fn from_ema_us(ema: u64) -> ThreadClass {
    if ema < 10_000 { Hot }
    else if ema < 100_000 { Normal }
    else { Cold }
}
```

### ThreadQueue
```rust
struct ThreadQueue {
    queue: Mutex<VecDeque<ThreadId>>,  // File FIFO
    size: AtomicUsize,                  // Taille cache
}
```

**Opérations O(1)**:
- `push(thread_id)`: Ajoute à la fin
- `pop() -> Option<ThreadId>`: Retire du début
- `len() -> usize`: Lecture atomique de `size`

### PredictiveScheduler
```rust
pub struct PredictiveScheduler {
    hot_queue: ThreadQueue,
    normal_queue: ThreadQueue,
    cold_queue: ThreadQueue,
    predictions: Mutex<BTreeMap<ThreadId, ThreadPrediction>>,
    tsc_frequency_mhz: AtomicU64,
    stats: SchedulerStats,
}
```

---

## 🔧 Algorithmes Principaux

### 1. Enregistrement Thread
```rust
register_thread(thread_id):
    prediction = ThreadPrediction::new(thread_id)
    predictions.insert(thread_id, prediction)
    normal_queue.push(thread_id)  // Commence en Normal
```

### 2. Exécution Thread
```rust
mark_execution_start(thread_id, cpu_id):
    prediction = predictions[thread_id]
    prediction.last_start_tsc = rdtsc()
    prediction.last_cpu_id = cpu_id

mark_execution_end(thread_id):
    prediction = predictions[thread_id]
    end_tsc = rdtsc()
    elapsed_cycles = end_tsc - prediction.last_start_tsc
    elapsed_us = elapsed_cycles / tsc_frequency_mhz
    
    old_class = prediction.class
    prediction.update_ema(elapsed_us)
    new_class = prediction.class
    
    if old_class != new_class:
        stats.reclassifications++
    
    # Réinsérer dans queue appropriée
    match new_class:
        Hot => hot_queue.push(thread_id)
        Normal => normal_queue.push(thread_id)
        Cold => cold_queue.push(thread_id)
```

### 3. Scheduling (Sélection Prochain Thread)
```rust
schedule_next(current_cpu_id) -> Option<ThreadId>:
    # Priorité 1: Hot
    if !hot_queue.is_empty():
        thread_id = select_with_affinity(hot_queue, current_cpu_id)
        stats.hot_scheduled++
        return Some(thread_id)
    
    # Priorité 2: Normal
    if !normal_queue.is_empty():
        thread_id = select_with_affinity(normal_queue, current_cpu_id)
        stats.normal_scheduled++
        return Some(thread_id)
    
    # Priorité 3: Cold
    if !cold_queue.is_empty():
        thread_id = cold_queue.pop()
        stats.cold_scheduled++
        return Some(thread_id)
    
    return None  # Idle
```

### 4. Sélection avec Affinité
```rust
select_with_affinity(queue, cpu_id) -> Option<ThreadId>:
    # Version simple: pop premier
    # TODO: Scanner N premiers pour meilleur affinity
    
    thread_id = queue.pop()
    prediction = predictions[thread_id]
    
    current_tsc = rdtsc()
    affinity = prediction.calculate_cache_affinity(cpu_id, current_tsc)
    
    if affinity > 80:
        stats.cache_affinity_hits++
    
    return thread_id
```

---

## 🧪 Tests Implémentés

### Tests Unitaires (8 tests)

1. **test_thread_class_from_ema**:
   - 5ms → Hot ✅
   - 50ms → Normal ✅
   - 150ms → Cold ✅

2. **test_thread_class_priority**:
   - Hot.priority() > Normal.priority() ✅
   - Normal.priority() > Cold.priority() ✅

3. **test_thread_prediction_new**:
   - thread_id correct
   - ema_execution_us == 0
   - class == Normal (défaut)

4. **test_ema_update**:
   - 1ère exec: ema = 10ms
   - 2ème exec (20ms): ema = 0.25×20 + 0.75×10 = 12.5ms ✅

5. **test_thread_reclassification**:
   - Normal → Hot (execs courtes)
   - Hot → Cold (execs longues)

6. **test_scheduler_register_thread**:
   - Enregistrement threads
   - Présence dans predictions

7. **test_scheduler_schedule_priority**:
   - Hot sort en premier
   - Puis Normal
   - Puis Cold

8. **test_stats_snapshot**:
   - cache_affinity_rate()
   - class_distribution()

### Benchmarks (6 benchmarks)

1. **bench_schedule_next_latency** (10k iter):
   - Mesure latence `schedule_next()`
   - Validation <300 cycles

2. **bench_ema_update** (100k iter):
   - Mesure latence `update_ema()`
   - Validation <100 cycles

3. **bench_cache_affinity_calculation** (50k iter):
   - Mesure `calculate_cache_affinity()`
   - Validation <150 cycles

4. **bench_thread_classification_workflow**:
   - Workflow complet 50 threads × 1000 iter
   - Mesure temps total
   - Affiche distribution finale

5. **bench_scheduling_fairness**:
   - 100 threads, 1000 schedules
   - Validation ratio max/min < 10:1
   - Tous threads schedulés ≥1 fois

6. **bench_cache_affinity_effectiveness**:
   - 20 threads, 4 CPUs, 500 iter
   - Mesure taux affinity hits
   - Attendu: 20-40%

---

## 📊 Résultats Attendus vs Réels

| Métrique | Attendu | Réel | Status |
|----------|---------|------|--------|
| **Latence schedule_next()** | 50-200 cycles | 🔄 À mesurer | Pending |
| **Latence update_ema()** | 10-50 cycles | 🔄 À mesurer | Pending |
| **Cache affinity rate** | 20-40% | 🔄 À mesurer | Pending |
| **Fairness ratio max/min** | <10:1 | 🔄 À mesurer | Pending |
| **Hot thread %** | 30-50% | 🔄 À mesurer | Pending |
| **Reclassifications** | 5-10% | 🔄 À mesurer | Pending |

---

## 🔧 Paramètres de Tuning

### Constantes Réglables

```rust
// EMA alpha (0.0-1.0)
// Plus élevé = plus réactif, mais instable
// Plus bas = plus stable, mais lent à s'adapter
const EMA_ALPHA: f64 = 0.25;

// Seuils classification (en microsecondes)
const HOT_THRESHOLD_US: u64 = 10_000;      // 10ms
const NORMAL_THRESHOLD_US: u64 = 100_000;  // 100ms

// Seuil cache affinity
const CACHE_AFFINITY_THRESHOLD_US: u64 = 50_000; // 50ms
```

**Recommandations**:
- **Workload interactif** (GUI, serveur web):
  - `HOT_THRESHOLD = 5ms`
  - `EMA_ALPHA = 0.3` (plus réactif)
  
- **Workload batch** (calculs, builds):
  - `HOT_THRESHOLD = 20ms`
  - `EMA_ALPHA = 0.15` (plus stable)

- **Workload mixte**:
  - Paramètres actuels (10ms, 0.25)

---

## 🚀 Optimisations Futures

### 1. Affinity Lookahead
Actuellement: `select_with_affinity()` pop le premier thread

**Optimisation**: Scanner les N premiers (ex: 5)
```rust
select_with_affinity(queue, cpu_id):
    candidates = queue.peek_n(5)
    best = candidates.max_by(|t| affinity_score(t, cpu_id))
    queue.remove(best)
    return best
```

Gain attendu: +10-20% affinity hits

### 2. Per-CPU Queues
Actuellement: 3 queues globales (Hot/Normal/Cold)

**Optimisation**: 3 × N_CPUS queues
```
CPU 0: [Hot_0, Normal_0, Cold_0]
CPU 1: [Hot_1, Normal_1, Cold_1]
...
```

Gain attendu: -50% contention sur locks

### 3. Lock-Free Queues
Actuellement: `Mutex<VecDeque>`

**Optimisation**: Utiliser `crossbeam::queue::SegQueue` ou `fusion_rings`

Gain attendu: -30% latence schedule_next()

### 4. Adaptive Thresholds
Actuellement: seuils fixes (10ms, 100ms)

**Optimisation**: Ajuster selon distribution réelle
```rust
hot_threshold = percentile_25(all_ema_times)
normal_threshold = percentile_75(all_ema_times)
```

Gain attendu: Meilleure classification dynamique

---

## 🔗 Intégration Kernel

### Cargo.toml
```toml
[features]
predictive_scheduler = []
```

### kernel/src/scheduler/mod.rs
```rust
#[cfg(feature = "predictive_scheduler")]
pub mod predictive_scheduler;

#[cfg(all(test, feature = "predictive_scheduler"))]
pub mod bench_predictive;
```

### Utilisation (exemple)
```rust
use scheduler::predictive_scheduler::PredictiveScheduler;

let scheduler = PredictiveScheduler::new();
scheduler.init_tsc_frequency(2000); // 2GHz

// Enregistrer threads
scheduler.register_thread(1);
scheduler.register_thread(2);

// Boucle scheduling
loop {
    if let Some(thread_id) = scheduler.schedule_next(current_cpu()) {
        scheduler.mark_execution_start(thread_id, current_cpu());
        
        // Exécuter thread
        run_thread(thread_id);
        
        scheduler.mark_execution_end(thread_id);
    } else {
        // Idle
        halt();
    }
}

// Statistiques
let stats = scheduler.stats();
println!("Hot: {}%, Normal: {}%, Cold: {}%",
    stats.class_distribution());
```

---

## ⚠️ Limitations Connues

1. **Pas de preemption tracking**: 
   - Mesure seulement temps volontaire
   - Solution: Tracker aussi interruptions timer

2. **Affinity simpliste**:
   - Pop premier thread sans lookahead
   - Solution: Scanner N premiers candidats

3. **Queues globales**:
   - Contention sur 3 locks (Hot/Normal/Cold)
   - Solution: Per-CPU queues

4. **Pas de deadline support**:
   - Scheduler purement prioritaire
   - Solution: Ajouter earliest-deadline-first pour threads temps-réel

5. **TSC frequency fixe**:
   - Assume fréquence constante
   - Solution: Recalibrer avec HPET/ACPI PM timer

---

## 📚 Références

**Code**:
- `kernel/src/scheduler/predictive_scheduler.rs` (550 lignes)
- `kernel/src/scheduler/bench_predictive.rs` (280 lignes)

**Algorithmes**:
- **EMA**: Exponential Moving Average (lissage exponentiel)
- **Priority Scheduling**: Multi-level feedback queue
- **Cache Affinity**: CPU pinning pour localité cache

**Inspirations**:
- **CFS** (Linux Completely Fair Scheduler): Virtual runtime tracking
- **Windows Thread Dispatcher**: Multi-level ready queues
- **FreeBSD ULE**: Load balancing avec affinity

---

**Dernière mise à jour**: 12 janvier 2025, 17:00 UTC  
**Auteur**: Exo-OS Team  
**Status**: ✅ Code complet, benchmarks en cours
