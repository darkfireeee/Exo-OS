# Scheduler Policies — CFS, RT, EDF, Idle, AI-Guided

> **Sources** : `kernel/src/scheduler/policies/`  
> **Règles** : SCHED-04, SCHED-12, SCHED-14

---

## Table des matières

1. [cfs.rs — Completely Fair Scheduler](#1-cfsrs--completely-fair-scheduler)
2. [realtime.rs — SCHED_FIFO / SCHED_RR](#2-realtimers--sched_fifo--sched_rr)
3. [deadline.rs — SCHED_DEADLINE (EDF)](#3-deadliners--sched_deadline-edf)
4. [idle.rs — Thread idle](#4-idlers--thread-idle)
5. [ai_guided.rs — Hints IA](#5-ai_guidedrs--hints-ia)

---

## 1. cfs.rs — Completely Fair Scheduler

### Principe du vruntime

Le CFS ordonnance les threads normaux en maintenant un **vruntime** (virtual runtime) normalisé par le poids : chaque thread avance son vruntime à la vitesse inverse de son poids. Le thread avec le **plus petit vruntime** est sélectionné.

```
vruntime_delta = delta_ns × NICE_0_WEIGHT / task_weight
```

Un thread avec `nice = -20` (poids 88761) avance son vruntime ~87× plus lentement que `nice = +19` (poids 1024), donc il est sélectionné plus souvent.

### Constantes

```rust
pub const CFS_MIN_SLICE_NS:     u64 = 750_000;          // 750 µs minimum par tranche
pub const CFS_TARGET_PERIOD_NS: u64 = 6_000_000;        // 6 ms de latence cible
pub const CFS_WAKEUP_PREEMPT_NS:u64 = 1_000_000;        // 1 ms seuil préemption wakeup
```

### timeslice_for

```rust
pub fn timeslice_for(
    tcb: &ThreadControlBlock,
    nr_tasks: usize,
    total_weight: u64,
) -> u64
```

Formule :
```
slice = CFS_TARGET_PERIOD_NS × task_weight / total_weight
slice = max(slice, CFS_MIN_SLICE_NS)
```

Si `nr_tasks == 0` → retourne `CFS_TARGET_PERIOD_NS`.

### should_preempt_on_wakeup

```rust
pub fn should_preempt_on_wakeup(
    current: &ThreadControlBlock,
    woken: &ThreadControlBlock,
) -> bool
```

Retourne `true` si :
```
woken.vruntime + CFS_WAKEUP_PREEMPT_NS < current.vruntime
```
C'est-à-dire si le thread réveillé a accumulé suffisamment moins de temps CPU.

### normalize_vruntime_on_enqueue

```rust
pub fn normalize_vruntime_on_enqueue(
    tcb: &ThreadControlBlock,
    min_vruntime: u64,
    slice_ns: u64,
)
```

Empêche un thread qui vient d'être dormant longtemps d'accaparer le CPU :
```
new_vruntime = max(tcb.vruntime, min_vruntime - slice_ns)
```

### tick_check_preempt

```rust
pub fn tick_check_preempt(
    current: &ThreadControlBlock,
    rq: &PerCpuRunQueue,
    elapsed_ns: u64,
) -> bool
```

Appelé à chaque tick :
```
ideal_runtime = timeslice_for(current, nr_tasks, total_weight)
Si elapsed_ns > ideal_runtime → set NEED_RESCHED → true
```

### Compteurs

```rust
pub static CFS_PREEMPTIONS:        AtomicU64  // Préemptions CFS totales
pub static CFS_WAKEUP_PREMPT_COUNT:AtomicU64  // Préemptions au wakeup
```

---

## 2. realtime.rs — SCHED_FIFO / SCHED_RR

### Constante

```rust
pub const RR_TIMESLICE_NS: u64 = 10_000_000;  // 10 ms quantum Round-Robin
```

### fifo_should_preempt

```rust
pub fn fifo_should_preempt(
    rq: &PerCpuRunQueue,
    running: &ThreadControlBlock,
) -> bool
```

Retourne `true` si un thread RT de priorité **strictement supérieure** (valeur inférieure) est présent dans la run queue RT.

```
rt_highest_prio(rq) < running.priority.0
```

Les threads FIFO ne sont **jamais** préemptés par des threads de même priorité (contrairement au RR).

### rr_tick

```rust
pub fn rr_tick(
    tcb: &ThreadControlBlock,
    elapsed_since_schedule_ns: u64,
) -> bool
```

Retourne `true` si le quantum Round-Robin est expiré :
```
elapsed_since_schedule_ns >= RR_TIMESLICE_NS
```

Quand expiré : le thread est re-enfilé à la fin de sa file de priorité.

### rr_remaining_slice

```rust
pub fn rr_remaining_slice(elapsed_since_schedule_ns: u64) -> u64
```

Retourne le temps restant dans le quantum :
```
RR_TIMESLICE_NS.saturating_sub(elapsed_since_schedule_ns)
```

### Compteurs

```rust
pub static RT_PREEMPTIONS:       AtomicU64  // Préemptions RT totales
pub static RR_QUANTUM_EXPIRATIONS:AtomicU64 // Fins de quantum RR
```

---

## 3. deadline.rs — SCHED_DEADLINE (EDF)

### Admission control

Avant d'accepter un thread DEADLINE, le scheduler vérifie que la charge CPU totale ne dépasse pas 100 % :

```
Σ (runtime_i / period_i) ≤ 1.0
```

Représenté en entier avec le dénominateur 2^32 :
```
cpu_fraction = runtime_ns × 2^32 / period_ns
TOTAL_CPU_FRACTION (AtomicU64) += cpu_fraction
Si TOTAL_CPU_FRACTION > 2^32 → refus
```

### DeadlineError

```rust
pub enum DeadlineError {
    InsufficientBudget,  // fraction CPU insuffisante disponible
    InvalidParams,       // deadline > period, runtime > deadline, etc.
    Overflow,            // Calcul de fraction CPU déborderait
}
```

### admit_thread

```rust
pub fn admit_thread(p: &DeadlineParams) -> Result<u64, DeadlineError>
```

1. Valide `runtime ≤ deadline ≤ period`, toutes valeurs > 0.
2. Calcule `fraction = runtime × 2^32 / period`.
3. `fetch_add(fraction)` sur `TOTAL_CPU_FRACTION`.
4. Si dépassement → `fetch_sub` (rollback) → `Err(InsufficientBudget)`.
5. Retourne la fraction allouée (pour libération ultérieure).

### release_thread

```rust
pub fn release_thread(fraction: u64)
```

`TOTAL_CPU_FRACTION.fetch_sub(fraction, SeqCst)` — libère le budget.

### Gestion de l'échéance

```rust
// Renouvelle l'échéance absolue : abs_deadline += period_ns
pub fn refresh_deadline(tcb: &mut ThreadControlBlock)

// Vérifie si l'échéance est dépassée (now > abs_deadline)
pub fn check_deadline_miss(tcb: &ThreadControlBlock) -> bool

// Budget restant dans la période courante
pub fn remaining_budget(tcb: &ThreadControlBlock) -> u64

// Appelé à chaque tick : décrémente budget, retourne true si expiré
pub fn deadline_tick(tcb: &ThreadControlBlock, elapsed_ns: u64) -> bool
```

### Compteurs

```rust
pub static DEADLINE_THREADS_ADMITTED: AtomicU64
pub static DEADLINE_ADMISSION_DENIED: AtomicU64
pub static DEADLINE_MISSES:           AtomicU64
```

---

## 4. idle.rs — Thread idle

### Marquage

```rust
pub fn mark_idle_thread(tcb: &mut ThreadControlBlock)
    // Sets flag IS_IDLE + priority = IDLE (140)

pub fn is_idle_thread(tcb: &ThreadControlBlock) -> bool
    // → tcb.flags & IS_IDLE != 0
```

### idle_iteration

```rust
pub unsafe fn idle_iteration(nr_running: usize) -> bool
```

Appelé à chaque itération de la boucle idle :
1. Si `nr_running > 0` → `true` (sortir de idle, quelque chose à faire).
2. Sélectionne le C-state optimal via `energy::c_states::select_cstate()`.
3. `energy::c_states::enter_cstate(cs)` → HLT (C1) ou MWAIT (C2/C3).
4. Retourne `false` (continuer boucle idle le cycle suivant).

### idle_loop

```rust
pub unsafe fn idle_loop(get_nr_running: unsafe fn() -> usize) -> !
```

Boucle perpétuelle :
```
loop {
    while !idle_iteration(get_nr_running()) {}
    // schedule() implicite via NEED_RESCHED à la sortie d'interruption
}
```

### Compteurs

```rust
pub static IDLE_ENTRIES:   AtomicU64  // Entrées en mode idle
pub static IDLE_HLT_COUNT: AtomicU64  // Instructions HLT exécutées
pub static IDLE_WAKEUPS:   AtomicU64  // Sorties de idle
```

---

## 5. ai_guided.rs — Hints IA

### Principe (SCHED-14)

Pas d'inférence ML à l'exécution. Uniquement des **tables de lookup** précomputées basées sur la classification EMA du thread (`ThreadAiState`).

### Tables de lookup (`.rodata`)

```rust
// Pénalité de vruntime pour threads CPU-bound (ralentit leur progression)
static CPU_BOUND_PENALTY: [u32; 16] = [
    0, 512, 1024, 1536, 2048, 2560, 3072, 3584,
    4096, 4608, 5120, 5632, 6144, 6656, 7168, 7680,
];

// Bonus de vruntime pour threads I/O-bound (accélère leur sélection)
static IO_BOUND_BONUS: [u32; 16] = [
    0, 256, 512, 768, 1024, 1280, 1536, 1792,
    2048, 2304, 2560, 2816, 3072, 3328, 3584, 3840,
];
```

Index = niveau d'intensité CPU (0 = faible, 15 = très intense), dérivé de `ai_state.avg_burst_cycles`.

### maybe_prefer

```rust
pub unsafe fn maybe_prefer(
    rq: &PerCpuRunQueue,
    candidate: NonNull<ThreadControlBlock>,
) -> Option<NonNull<ThreadControlBlock>>
```

Logique :
1. Si `AI_HINTS_ENABLED == false` → `None` (fallback CFS normal).
2. Lit `candidate.ai_state.is_cpu_bound()`.
3. Si CPU-bound : applique `CPU_BOUND_PENALTY[intensity]` au vruntime virtuel.
4. Si I/O-bound : soustrait `IO_BOUND_BONUS[intensity]` du vruntime virtuel.
5. Compare vruntime virtuel modifié avec le thread current.
6. Retourne le meilleur candidat, ou `None` si CFS gagne.

**Fallback automatique** : si `AI_HINTS_ENABLED` est désactivé (runtime), toutes les décisions tombent sur CFS standard.

### Contrôle global

```rust
pub static AI_HINTS_ENABLED: AtomicBool = AtomicBool::new(true);
// Peut être désactivé via syscall ou paramètre de démarrage

pub static AI_GUIDED_PICKS:  AtomicU64  // Décisions guidées par AI
pub static AI_FALLBACK_PICKS: AtomicU64 // Décisions CFS fallback
```
