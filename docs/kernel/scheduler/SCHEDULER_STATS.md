# Scheduler Stats — CpuStats, LatencyHist

> **Sources** : `kernel/src/scheduler/stats/`

---

## Table des matières

1. [per_cpu.rs — CpuStats](#1-per_cpurs--cpustats)
2. [latency.rs — LatencyHist](#2-latencyrs--latencyhist)

---

## 1. per_cpu.rs — CpuStats

### Structure CpuStats

```rust
pub struct CpuStats {
    // Changements de contexte
    voluntary_switches:   AtomicU64,   // Thread bloqué volontairement
    involuntary_switches: AtomicU64,   // Préemption forcée

    // Comptabilité temps
    run_time_ns:   AtomicU64,    // Total temps CPU utilisateur+noyau
    idle_time_ns:  AtomicU64,    // Total temps en idle/HLT

    // Activité ticks
    ticks:         AtomicU64,    // Ticks totaux depuis le boot

    // Migrations SMP
    migrations_sent:   AtomicU64,   // Threads envoyés sur autre CPU
    migrations_rcvd:   AtomicU64,   // Threads reçus d'autres CPUs
}
```

### Tableau statique per-CPU

```rust
static CPU_STATS: [CpuStats; MAX_CPUS]  // MaxCPUs = 64
```

Chaque CPU a sa propre instance dans ce tableau, évitant le false-sharing grâce à `#[repr(C, align(64))]`.

### API

```rust
// Retourne la référence aux stats du CPU, ou None si cpu >= nr_cpus
pub fn stats(cpu: usize) -> Option<&'static CpuStats>

// Incrémente le compteur de context switches
pub fn inc_context_switches(cpu: usize, voluntary: bool)

// Ajoute du temps d'exécution (appelé depuis scheduler_tick)
pub fn add_run_time(cpu: usize, ns: u64)

// Ajoute du temps idle (appelé depuis idle_iteration)
pub fn add_idle_time(cpu: usize, ns: u64)

// Incrémente le compteur de ticks
pub fn inc_ticks(cpu: usize)

// Migrations
pub fn inc_migrations_sent(cpu: usize)
pub fn inc_migrations_rcvd(cpu: usize)
```

### Lecture des stats

```rust
let s = stats::per_cpu::stats(0).unwrap();
let total_switches = s.voluntary_switches.load(Relaxed)
                   + s.involuntary_switches.load(Relaxed);
let cpu_util_pct = s.run_time_ns.load(Relaxed) * 100
                 / (s.run_time_ns.load(Relaxed) + s.idle_time_ns.load(Relaxed));
```

### Utilisation dans le scheduler

| Appelant | Fonction | Moment |
|----------|----------|--------|
| `scheduler_tick()` | `inc_ticks()`, `add_run_time()` | Chaque tick |
| `context_switch()` | `inc_context_switches()` | À chaque switch |
| `idle_iteration()` | `add_idle_time()` | Sortie d'idle |
| `migration::request_migration()` | `inc_migrations_sent()` | Migration déclenchée |
| `tick::drain_pending_migrations()` | `inc_migrations_rcvd()` | Migration reçue |

---

## 2. latency.rs — LatencyHist

### LatencyHist — Histogramme de latence

```rust
pub struct LatencyHist {
    // 32 buckets logarithmiques :
    // [0]=0-1ns, [1]=1-2ns, [2]=2-4ns, ..., [31]=2^31+ ns
    buckets: [AtomicU64; 32],

    total:  AtomicU64,   // Nombre d'échantillons
    sum_ns: AtomicU64,   // Somme pour calcul de moyenne
    max_ns: AtomicU64,   // Maximum observé
}
```

### Enregistrement

```rust
impl LatencyHist {
    pub fn record(&self, ns: u64) {
        // Bucket = log2(ns) clampé à [0, 31]
        let bucket = (64 - ns.leading_zeros() as usize).min(31);
        self.buckets[bucket].fetch_add(1, Relaxed);
        self.sum_ns.fetch_add(ns, Relaxed);
        self.total.fetch_add(1, Relaxed);
        // Mise à jour max_ns (CAS loop)
    }
}
```

### Percentiles

```rust
impl LatencyHist {
    // Calcule le Nième percentile (N = 0..100)
    pub fn percentile_pct(&self, p: u64) -> u64 {
        let target = self.total.load(Relaxed) * p / 100;
        let mut cumul = 0u64;
        for (i, bucket) in self.buckets.iter().enumerate() {
            cumul += bucket.load(Relaxed);
            if cumul >= target {
                return 1u64 << i;  // Borne supérieure du bucket
            }
        }
        u64::MAX
    }

    pub fn p50(&self)  -> u64 { self.percentile_pct(50) }   // Médiane
    pub fn p99(&self)  -> u64 { self.percentile_pct(99) }   // 99e centile
    pub fn p999(&self) -> u64 { ... }                        // 99.9e centile

    pub fn total(&self)  -> u64
    pub fn max_ns(&self) -> u64
    pub fn avg_ns(&self) -> u64 {
        let t = self.total.load(Relaxed);
        if t == 0 { 0 } else { self.sum_ns.load(Relaxed) / t }
    }

    pub fn reset(&self)   // Remet tous les buckets et compteurs à zéro
}
```

### Instances globales

```rust
pub static SWITCH_LATENCY:   LatencyHist  // Latence de context_switch() (ns)
pub static WAKEUP_LATENCY:   LatencyHist  // Latence réveil (blocked→running) (ns)
pub static PICKNEXT_LATENCY: LatencyHist  // Latence pick_next_task() (ns)
pub static IPI_LATENCY:      LatencyHist  // Latence IPI reschedule (ns)
```

### Initialisation

```rust
pub unsafe fn init()
```
Remet à zéro toutes les instances (appelé depuis `scheduler::init()`).

### Exemple de lecture

```
SWITCH_LATENCY après 1h de charge :
  total  = 3,600,000  (1000 switches/s × 3600s)
  avg    = 850 ns
  p50    = 512 ns
  p99    = 4096 ns   (~4 µs)
  p99.9  = 32768 ns  (~33 µs — cache miss ou préemption longue)
  max    = 131072 ns (~131 µs — rare, lock contention)
```

### Granularité des buckets

| Bucket | Plage | Description |
|--------|-------|-------------|
| 0 | 0–1 ns | Impossible en pratique (< 1 cycle @3GHz) |
| 1 | 1–2 ns | Quelques cycles |
| 9 | 512 ns–1 µs | Typique context switch (cache chaud) |
| 10 | 1–2 µs | Cache miss L1/L2 |
| 13 | 8–16 µs | Cache miss LLC |
| 17 | 128–256 µs | Stall mémoire principale |
| 20 | 1–2 ms | Contention lock sévère |
| 31 | >2^31 ns | Anomalie système |
