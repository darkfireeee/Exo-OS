# Phase 5 : Adaptive Drivers - Rapport Technique

**Date** : 12 janvier 2025  
**Statut** : ✅ COMPLET  
**Gains attendus** : -40 à -60% latence (polling) | -80 à -95% CPU (interrupt)

---

## 1. Vue d'Ensemble

### 1.1. Problématique

Les drivers traditionnels utilisent soit :
- **Interrupts** : Faible CPU (~1-5%) mais latence élevée (~10-50µs)
- **Polling** : Latence faible (~1-5µs) mais CPU élevé (~90-100%)

**Solution** : Adaptive Drivers qui switchent automatiquement entre modes selon la charge.

### 1.2. Architecture

```
┌─────────────────────────────────────────────────┐
│          AdaptiveDriver Trait                   │
│  - wait_interrupt()  : Mode Interrupt           │
│  - poll_status()     : Mode Polling             │
│  - hybrid_wait()     : Mode Hybrid              │
│  - batch_operation() : Mode Batch               │
└─────────────────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────┐
│        AdaptiveController                       │
│  - SlidingWindow (1 sec)                        │
│  - Auto-switch logic                            │
│  - DriverStats tracking                         │
└─────────────────────────────────────────────────┘
                      │
        ┌─────────────┼─────────────┐
        ▼             ▼             ▼
  ┌─────────┐   ┌─────────┐   ┌─────────┐
  │  Block  │   │ Network │   │  Other  │
  │ Driver  │   │ Driver  │   │ Drivers │
  └─────────┘   └─────────┘   └─────────┘
```

---

## 2. Modes de Fonctionnement

### 2.1. DriverMode Enum

```rust
pub enum DriverMode {
    Interrupt,  // Faible charge
    Polling,    // Charge élevée
    Hybrid,     // Charge moyenne
    Batch,      // Coalescence
}
```

#### Caractéristiques par mode

| Mode      | Latence (µs) | CPU (%)  | Throughput | Use Case               |
|-----------|--------------|----------|------------|------------------------|
| Interrupt | 10-50        | 1-5      | Faible     | Idle, faible activité  |
| Polling   | 1-5          | 90-100   | Élevé      | Charge intensive       |
| Hybrid    | 5-15         | 20-60    | Moyen      | Charge variable        |
| Batch     | 100-1000     | Variable | Très élevé | Disk I/O, coalescence  |

### 2.2. Latency Priority

```rust
impl DriverMode {
    pub fn latency_priority(&self) -> u8 {
        match self {
            DriverMode::Polling   => 0,  // Plus faible latence
            DriverMode::Hybrid    => 1,
            DriverMode::Interrupt => 2,
            DriverMode::Batch     => 3,  // Latence acceptable pour throughput
        }
    }
}
```

---

## 3. AdaptiveController - Auto-Switch Logic

### 3.1. Seuils de Switch

```rust
const HIGH_THROUGHPUT_THRESHOLD: u64 = 10_000;  // ops/sec
const LOW_THROUGHPUT_THRESHOLD: u64 = 1_000;    // ops/sec
```

#### Logique de décision

```
Throughput > 10K ops/sec  → Mode POLLING
  (Charge élevée : favoriser latence faible)

1K < Throughput < 10K     → Mode HYBRID
  (Charge moyenne : compromis CPU/latence)

Throughput < 1K ops/sec   → Mode INTERRUPT
  (Charge faible : économiser CPU)
```

### 3.2. Sliding Window (1 seconde)

```rust
pub struct SlidingWindow {
    timestamps: VecDeque<u64>,  // TSC timestamps
}
```

**Fonctionnement** :
1. Enregistre timestamp TSC à chaque opération
2. Nettoie timestamps > 1 seconde
3. Calcule `throughput = count / elapsed_sec`

**Complexité** :
- `record_timestamp()`: O(1) amortized
- `current_throughput()`: O(n) où n = ops dans la fenêtre

### 3.3. DriverStats - Métriques

```rust
pub struct DriverStats {
    total_operations: u64,
    total_cycles: u64,
    mode_switches: u64,
    
    // Temps par mode (µs)
    time_interrupt_us: u64,
    time_polling_us: u64,
    time_hybrid_us: u64,
    time_batch_us: u64,
}
```

**Métriques calculées** :
- `avg_throughput()`: ops/sec global
- `avg_cycles_per_op()`: Efficacité
- `mode_distribution()`: % temps par mode

---

## 4. AdaptiveBlockDriver - Implémentation Disque

### 4.1. Architecture

```rust
pub struct AdaptiveBlockDriver {
    controller: Mutex<AdaptiveController>,
    request_queue: Mutex<VecDeque<BlockRequest>>,
    hardware_ready: AtomicBool,
    stats: DriverStats,
}
```

### 4.2. Soumission de Requête

```rust
pub fn submit_request(&mut self, request: BlockRequest) 
    -> Result<(), &'static str> 
{
    let start_tsc = rdtsc();
    
    // 1. Enregistrer opération → obtenir mode optimal
    let mode = self.controller.lock().record_operation();
    
    // 2. Traiter selon mode
    let result = match mode {
        DriverMode::Interrupt => self.submit_interrupt_mode(request),
        DriverMode::Polling   => self.submit_polling_mode(request),
        DriverMode::Hybrid    => self.submit_hybrid_mode(request),
        DriverMode::Batch     => self.submit_batch_mode(request),
    };
    
    // 3. Enregistrer cycles
    let cycles = rdtsc() - start_tsc;
    self.controller.lock().record_cycles(cycles);
    
    result
}
```

### 4.3. Mode Batch - Coalescence

**Avantages** :
- Réordonne requêtes par numéro de bloc → accès séquentiel
- Réduit nombre d'interrupts
- Améliore utilisation DMA

**Flush automatique** :
- Quand queue atteint `MAX_BATCH_SIZE` (32 requêtes)
- Ou timeout (non implémenté dans cette phase)

```rust
fn flush_batch(&mut self) -> Result<(), &'static str> {
    let mut batch = queue.drain(..).collect();
    
    // Coalescence: tri par block_number
    batch.sort_by_key(|req| req.block_number);
    
    // Soumission batch
    for request in batch.iter() {
        self.send_to_hardware(request)?;
    }
    
    // Attente completion
    for _ in 0..batch_size {
        self.wait_interrupt()?;
    }
}
```

---

## 5. Hybrid Mode - Best of Both Worlds

### 5.1. Principe

```rust
const MAX_POLL_CYCLES: u64 = 10_000;  // ~5µs @ 2GHz

fn hybrid_wait(&mut self) -> Result<(), &'static str> {
    let start = rdtsc();
    
    // Phase 1: Polling court
    while rdtsc() - start < MAX_POLL_CYCLES {
        if self.poll_status()? {
            return Ok(());  // Completion rapide
        }
    }
    
    // Phase 2: Fallback interrupt
    self.wait_interrupt()
}
```

**Gains** :
- Si hardware répond vite (<5µs) : latence polling
- Sinon : évite gaspillage CPU avec interrupt

### 5.2. Tuning MAX_POLL_CYCLES

```
Valeur trop faible  → Passe trop vite en interrupt (perd gains polling)
Valeur trop élevée  → Gaspille CPU si hardware lent
```

**Valeur optimale** : Dépend de la latence moyenne hardware
- SSD NVMe : 5-10µs → `MAX_POLL_CYCLES = 10K-20K`
- HDD mécanique : 5-10ms → `MAX_POLL_CYCLES = 1K`

---

## 6. Benchmarks RDTSC

### 6.1. Métriques Collectées

```rust
pub struct BenchStats {
    samples: Vec<u64>,
    mean: u64,
    min: u64,
    max: u64,
    std_dev: u64,
    p50: u64,   // Médiane
    p95: u64,
    p99: u64,
}
```

### 6.2. Benchmark 1 : Mode Switch Latency

**Mesure** : Temps pour changer de mode
```rust
bench_mode_switch(1000 iterations)
```

**Attendu** : <500 cycles (~250ns @ 2GHz)

### 6.3. Benchmark 2 : Record Operation Overhead

**Mesure** : Overhead de `record_operation()`
```rust
bench_record_operation(10000 iterations)
```

**Attendu** : <200 cycles (~100ns)

### 6.4. Benchmark 3 : Throughput Calculation

**Mesure** : Temps de calcul `current_throughput()`
```rust
bench_throughput_calculation(10000 iterations)
```

**Attendu** : <1000 cycles (~500ns)

### 6.5. Benchmark 4 : Submit Request (Polling)

**Mesure** : Latence soumission en mode polling
```rust
bench_submit_polling(1000 iterations)
```

**Attendu** : 2K-10K cycles (1-5µs) selon simulation hardware

### 6.6. Benchmark 5 : Submit Request (Batch)

**Mesure** : Latence par requête en batch de 32
```rust
bench_submit_batch(batch_size=32)
```

**Attendu** : 
- Latence individuelle : Élevée (attente batch)
- Throughput global : 2-3× supérieur

### 6.7. Benchmark 6 : Auto-Switch (Variable Load)

**Scénario** :
1. **Phase 1** : 100 req/sec (faible charge) → Mode Interrupt
2. **Phase 2** : 5K req/sec (charge moyenne) → Mode Hybrid
3. **Phase 3** : 50K req/sec (charge élevée) → Mode Polling

**Mesures** :
- Distribution temps par mode
- Nombre de switches
- Latence moyenne par phase

---

## 7. Résultats Attendus

### 7.1. Gains Latence

| Mode      | Latence vs Interrupt | Latence vs Polling |
|-----------|----------------------|--------------------|
| Interrupt | Baseline (100%)      | +800% à +2500%     |
| Polling   | -80% à -95%          | Baseline (100%)    |
| Hybrid    | -50% à -70%          | +200% à +500%      |

**Example @ 2GHz** :
- Interrupt : 20µs = 40K cycles
- Polling : 2µs = 4K cycles (-90%)
- Hybrid : 8µs = 16K cycles (-60%)

### 7.2. Gains CPU

| Mode      | CPU Usage | Économie vs Polling |
|-----------|-----------|---------------------|
| Interrupt | 1-5%      | -85% à -95%         |
| Polling   | 90-100%   | Baseline            |
| Hybrid    | 20-60%    | -40% à -80%         |

### 7.3. Throughput (Batch Mode)

**Sans coalescence** :
- Requêtes aléatoires : Seek time élevé
- Throughput : ~100 IOPS

**Avec coalescence** :
- Requêtes triées : Accès séquentiel
- Throughput : ~250-300 IOPS (+150% à +200%)

---

## 8. Stratégies d'Optimisation

### 8.1. Polling Adaptatif

**Idée** : Ajuster `MAX_POLL_CYCLES` dynamiquement
```rust
if avg_completion_time < 5µs {
    MAX_POLL_CYCLES = 20_000;  // Augmenter polling
} else {
    MAX_POLL_CYCLES = 5_000;   // Réduire gaspillage
}
```

### 8.2. Batch Timeout

**Problème actuel** : Batch flush seulement si queue pleine
**Solution** : Timeout de 1ms
```rust
if queue.len() > 0 && elapsed_since_first > 1ms {
    flush_batch();
}
```

### 8.3. Prédiction de Charge

**Idée** : EMA sur throughput pour anticiper switches
```rust
predicted_throughput = α * current + (1-α) * previous
if predicted_throughput > threshold {
    preemptive_switch_to_polling();
}
```

### 8.4. Cache-Aware Batch Ordering

**Amélioration** : Trier batch par proximité cache
```rust
batch.sort_by_key(|req| {
    (req.block_number / CACHE_LINE_SIZE, req.block_number)
});
```

---

## 9. Extensions Futures

### 9.1. Network Driver

**Modes adaptés** :
- **Interrupt** : Faible trafic (<1K packets/sec)
- **NAPI** (Hybrid) : Trafic moyen (1K-100K pps)
- **Polling** : Trafic élevé (>100K pps)

### 9.2. GPU Driver

**Spécificité** : Soumission batch de commandes
- Mode Batch par défaut
- Polling pour sync rapide

### 9.3. USB Driver

**Contrainte** : Latence stricte pour devices temps réel (audio)
- Force Polling pour audio/video
- Interrupt pour stockage

---

## 10. Code Clé

### 10.1. AdaptiveDriver Trait

```rust
pub trait AdaptiveDriver {
    fn name(&self) -> &str;
    fn wait_interrupt(&mut self) -> Result<(), &'static str>;
    fn poll_status(&mut self) -> Result<bool, &'static str>;
    fn batch_operation(&mut self, batch_size: usize) 
        -> Result<usize, &'static str>;
    fn current_mode(&self) -> DriverMode;
    fn set_mode(&mut self, mode: DriverMode);
    fn stats(&self) -> &DriverStats;
}
```

### 10.2. Auto-Switch Implementation

```rust
pub fn record_operation(&mut self) -> DriverMode {
    self.sliding_window.record_timestamp(rdtsc());
    self.stats.total_operations += 1;
    
    let throughput = self.sliding_window
        .current_throughput(self.tsc_frequency_mhz);
    
    let optimal_mode = if throughput > HIGH_THROUGHPUT_THRESHOLD {
        DriverMode::Polling
    } else if throughput < LOW_THROUGHPUT_THRESHOLD {
        DriverMode::Interrupt
    } else {
        DriverMode::Hybrid
    };
    
    if optimal_mode != self.current_mode {
        self.switch_mode(optimal_mode);
    }
    
    self.current_mode
}
```

---

## 11. Tests Unitaires

### 11.1. Coverage

**adaptive_driver.rs** : 10 tests
- Mode name/priority
- Stats calculations (throughput, cycles/op, distribution)
- Controller init/force_mode
- SlidingWindow throughput
- Auto-switch logic

**adaptive_block.rs** : 5 tests
- BlockRequest creation
- Driver initialization
- Polling mode submission
- Batch accumulation
- Batch flush on full

**bench_adaptive.rs** : 3 tests
- BenchStats calculation
- Mode switch bench
- Record operation bench

**Total** : 18 tests

### 11.2. Test Example

```rust
#[test]
fn test_auto_switch_high_throughput() {
    let mut controller = AdaptiveController::new(2000);
    
    // Simuler 20K ops/sec
    for _ in 0..20_000 {
        let mode = controller.record_operation();
        controller.record_cycles(1000);
    }
    
    // Devrait switcher en Polling
    assert_eq!(controller.current_mode(), DriverMode::Polling);
}
```

---

## 12. Intégration Kernel

### 12.1. Fichiers Créés

```
kernel/src/drivers/
├── adaptive_driver.rs      (450 lignes)  ✅
├── adaptive_block.rs       (400 lignes)  ✅
└── bench_adaptive.rs       (400 lignes)  ✅
```

### 12.2. Modifications

```
kernel/src/drivers/mod.rs
+ pub mod adaptive_driver;
+ pub mod adaptive_block;
+ #[cfg(test)]
+ pub mod bench_adaptive;
```

---

## 13. Conclusion

### 13.1. Achievements

✅ **Trait AdaptiveDriver** : Interface générique pour tous drivers  
✅ **Auto-switch logic** : Adaptation automatique selon charge  
✅ **SlidingWindow** : Mesure précise throughput (1 sec)  
✅ **Block Driver** : Implémentation complète avec batch  
✅ **Benchmarks** : 6 benchmarks RDTSC complets  
✅ **Tests** : 18 unit tests avec validation  

### 13.2. Gains Projetés

| Métrique      | Interrupt (baseline) | Adaptive (optimal) | Gain     |
|---------------|----------------------|--------------------|----------|
| Latence       | 20µs                 | 2-8µs              | -60% à -90% |
| CPU Usage     | 5% (idle)            | 5-90% (adaptatif)  | Optimal  |
| Throughput    | 100 IOPS             | 250-300 IOPS       | +150% à +200% |

### 13.3. Prochaines Étapes

1. ✅ **Phase 5 complète** : Trait + Block + Benchmarks
2. ⏳ **Rapport technique** : Ce document
3. ⏳ **Framework benchmark** : Orchestration globale
4. ⏳ **Validation finale** : Exécution + comparaisons

---

**Statut** : ✅ Phase 5 - Adaptive Drivers TERMINÉE  
**Prochaine Phase** : Framework de Benchmarking Unifié (Task 19)

