# 📊 Prédiction EMA

## Moyenne Mobile Exponentielle

L'EMA (Exponential Moving Average) prédit le temps d'exécution futur d'un thread basé sur son historique.

## Formule

```
new_ema = α × actual_runtime + (1 - α) × old_ema
```

Où:
- `α` (alpha) = facteur de lissage (défaut: 0.125 = 1/8)
- `actual_runtime` = temps réel de la dernière exécution
- `old_ema` = prédiction précédente

## Implémentation Optimisée

```rust
// Évite la multiplication flottante
const EMA_ALPHA_SHIFT: u32 = 3;  // α = 1/8

fn update_ema(old_ema: u64, actual: u64) -> u64 {
    // new = actual/8 + old*7/8
    // new = actual/8 + old - old/8
    // new = old + (actual - old)/8
    let diff = actual as i64 - old_ema as i64;
    (old_ema as i64 + (diff >> EMA_ALPHA_SHIFT)) as u64
}
```

## Choix de Alpha

| Alpha | Réactivité | Stabilité | Usage |
|-------|------------|-----------|-------|
| 0.5 | Très haute | Basse | Workloads très variables |
| 0.25 | Haute | Moyenne | Défaut agressif |
| **0.125** | Moyenne | **Haute** | **Défaut Exo-OS** |
| 0.0625 | Basse | Très haute | Workloads stables |

## Structure EmaPredictor

```rust
pub struct EmaPredictor {
    /// EMA courante en nanosecondes
    ema_ns: u64,
    
    /// Nombre d'échantillons
    samples: u64,
    
    /// Variance (pour détection de changement)
    variance: u64,
}

impl EmaPredictor {
    pub fn update(&mut self, runtime_ns: u64) {
        if self.samples == 0 {
            // Premier échantillon: initialiser directement
            self.ema_ns = runtime_ns;
        } else {
            // Mise à jour EMA
            let diff = runtime_ns as i64 - self.ema_ns as i64;
            self.ema_ns = (self.ema_ns as i64 + (diff >> EMA_ALPHA_SHIFT)) as u64;
            
            // Mise à jour variance
            let var_diff = (diff.abs() as u64) as i64 - self.variance as i64;
            self.variance = (self.variance as i64 + (var_diff >> EMA_ALPHA_SHIFT)) as u64;
        }
        self.samples += 1;
    }
    
    pub fn predict(&self) -> u64 {
        self.ema_ns
    }
    
    pub fn confidence(&self) -> f32 {
        // Plus d'échantillons = plus de confiance
        1.0 - (1.0 / (self.samples as f32 + 1.0))
    }
}
```

## Exemple d'Adaptation

```
Thread "interactive_ui":
  Run 1: 500µs  → EMA = 500µs         → HOT
  Run 2: 800µs  → EMA = 537µs         → HOT
  Run 3: 600µs  → EMA = 545µs         → HOT
  Run 4: 2ms    → EMA = 727µs         → HOT
  Run 5: 15ms   → EMA = 2.5ms         → NORMAL (migration!)
  Run 6: 800µs  → EMA = 2.3ms         → NORMAL
  Run 7: 500µs  → EMA = 2.0ms         → NORMAL
  ...
  Run 20: 600µs → EMA = 800µs         → HOT (migration back!)
```

## Heuristiques Additionnelles

```rust
pub struct PredictionHeuristics {
    /// Détecte les patterns périodiques
    pub periodic_detector: PeriodicDetector,
    
    /// Détecte les pics de charge
    pub burst_detector: BurstDetector,
    
    /// Historique récent
    pub history: ExecutionHistory,
}
```

### Détection de Période

```rust
// Si un thread a un pattern (ex: timer 10ms)
if periodic_detector.detect_period(&history) {
    // Pré-réveiller le thread avant le deadline
    scheduler.pre_wake(thread_id, predicted_wake_time);
}
```

### Détection de Burst

```rust
// Si un thread alterne burst/idle
if burst_detector.is_bursting(&history) {
    // Augmenter temporairement la priorité
    scheduler.boost_priority(thread_id);
}
```
