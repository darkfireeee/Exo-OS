# Scheduler Core — TCB, Context Switch, RunQueue, Pick-Next

> **Sources** : `kernel/src/scheduler/core/`  
> **Règles** : SCHED-02, SCHED-03, SCHED-04, SCHED-06 à SCHED-12

---

## Table des matières

1. [task.rs — ThreadControlBlock](#1-taskrs--threadcontrolblock)
2. [switch.rs — Context Switch](#2-switchrs--context-switch)
3. [preempt.rs — Guards RAII](#3-preemptrs--guards-raii)
4. [runqueue.rs — PerCpuRunQueue](#4-runqueuers--percpurunqueue)
5. [pick_next.rs — Algorithme O(1)](#5-pick_nextrs--algorithme-o1)

---

## 1. task.rs — ThreadControlBlock

### Identifiants

```rust
pub struct ThreadId(pub u32);     // identifiant unique de thread
pub struct ProcessId(pub u32);    // identifiant de processus parent
pub struct CpuId(pub u32);        // indice du CPU (0..MAX_CPUS-1)
```

### Priorité

```rust
pub struct Priority(pub u8);

impl Priority {
    pub const RT_MAX:        Self = Self(0);    // Priorité RT la plus haute
    pub const RT_MIN:        Self = Self(99);   // Priorité RT la plus basse
    pub const NORMAL_MAX:    Self = Self(100);  // Normal le plus prioritaire
    pub const NORMAL_DEFAULT:Self = Self(120);  // Nice 0 → priorité 120
    pub const NORMAL_MIN:    Self = Self(139);  // Nice +19 → priorité 139
    pub const IDLE:          Self = Self(140);  // Thread idle uniquement
}

// Correspondance nice → priorité
// nice ∈ [-20, +19] → priority ∈ [100, 139]
// priority = 120 + nice
```

### Politique d'ordonnancement

```rust
pub enum SchedPolicy {
    Fifo,       // RT temps-réel : pas de quantum, préempté seulement par RT+
    RoundRobin, // RT temps-réel : quantum 10 ms (RR_TIMESLICE_NS)
    Normal,     // CFS (Completely Fair Scheduler)
    Deadline,   // EDF (SCHED_DEADLINE) — admission control
    Idle,       // Thread idle exclusivement
}
```

**Poids CFS** (table conforme Linux) :

```
nice -20 → poids 88761   (×9×  le poids nice 0)
nice   0 → poids 1024    (poids de référence)
nice +19 → poids 15      (×1/68 le poids nice 0)
```

### État du thread

```rust
pub enum TaskState {
    Running,   // Actuellement sur un CPU
    Runnable,  // Prêt, dans une run queue
    Blocked,   // En attente d'un événement (wait_queue)
    Zombie,    // Terminé, en attente de join/reap
    Dead,      // Totalement terminé
}
```

**Transitions atomiques** via `task.try_transition(from, to)` (compare-and-swap).

### ThreadAiState — Profilage EMA inline (8 octets)

```rust
pub struct ThreadAiState {
    avg_burst_cycles: u32,  // EMA 1/8 des cycles CPU mesurés
    avg_sleep_us:     u32,  // EMA 1/8 du temps de sommeil (µs)
}
```

- `is_cpu_bound()` : vrai si `avg_burst_cycles > avg_sleep_us * 1000`
- Pas d'inférence ML : classification déterministe par seuil.

### DeadlineParams

```rust
pub struct DeadlineParams {
    runtime_ns:  u64,  // Budget d'exécution par période
    deadline_ns: u64,  // Délai relatif à l'activation
    period_ns:   u64,  // Période de renouvellement
    // Champs internes (gérés par deadline.rs) :
    abs_deadline: u64,
    remaining_budget: u64,
    cpu_fraction: u64,  // × 2^32, pour admission control
}
```

### ThreadControlBlock — Layout mémoire

`#[repr(C, align(64))]` — exactement **128 octets** (SCHED-03) :

```
Offset  Size  Field
  0       4   tid               ThreadId
  4       4   pid               ProcessId
  8       4   cpu               AtomicU32 (CpuId courant)
  12      1   priority          Priority
  13      1   policy            SchedPolicy (tag enum)
  14      1   state             AtomicU8 (TaskState)
  15      1   [pad]
  16      8   vruntime          AtomicU64 (ns normalisé par poids)
  24      8   kernel_rsp        u64 (RSP sauvegardé lors du switch)
  32      8   cr3               u64 (adresse physique PML4)
  40      8   fpu_state_ptr     *mut u8 (pointe FpuState, null si non init)
  48      8   deadline          DeadlineParams (packed : runtime,deadline,period)
  56      8   ai_state          ThreadAiState (avg_burst + avg_sleep)
  64      8   flags             AtomicU32 (+ 4B pad)
  72      8   affinity          u64 (bitmask 64 CPUs)
  80      1   signal_pending    AtomicBool
  81      1   fpu_loaded        AtomicBool (redondant avec FPU_LOADED flag)
  82      6   [pad]
  88     40   stats             TaskStats (interne, compteurs)
 128      ←   fin (2 × cache line de 64 B)
```

### Flags TCB

| Bit | Constante | Description |
|-----|-----------|-------------|
| 0 | `KTHREAD` | Thread noyau (pas de mapping utilisateur) |
| 1 | `FPU_LOADED` | L'état FPU est chargé dans les registres |
| 2 | `EXITING` | Thread en cours de terminaison |
| 3 | `WAKEUP_SPURIOUS` | Réveil spurieux autorisé |
| 4 | `NEED_RESCHED` | Préemption demandée |
| 5 | `IN_RECLAIM` | En cours de récupération mémoire |
| 6 | `MIGRATED` | Vient d'être migré vers ce CPU |
| 7 | `PTRACE` | Sous surveillance ptrace |
| 8 | `IN_WAIT_QUEUE` | Inséré dans une WaitQueue |
| 9 | `IS_IDLE` | Thread idle du CPU |

### Signal (SCHED-15)

```rust
// Dans TCB :
signal_pending: AtomicBool,
signal_mask:    AtomicU64,

// API scheduler (lecture uniquement) :
pub fn has_signal_pending(&self) -> bool
pub fn set_signal_pending(&self)   // Appelé par process::signal:: uniquement
pub fn clear_signal_pending(&self)
```

**Le scheduler ne modifie jamais `signal_mask`** ni ne délivre les signaux. Il lit uniquement `signal_pending` pour savoir s'il faut retransmettre le contrôle à `process::signal::`.

---

## 2. switch.rs — Context Switch

### check_signal_pending

```rust
pub fn check_signal_pending(tcb: &ThreadControlBlock) -> bool {
    tcb.has_signal_pending()  // lecture AtomicBool, Relaxed
}
```

Lecture seule — conforme SCHED-15.

### context_switch

```rust
pub unsafe fn context_switch(
    prev: &mut ThreadControlBlock,
    next: &mut ThreadControlBlock,
)
```

**Séquence exacte** (SCHED-09) :

1. Si `prev.fpu_loaded()` → `fpu::save_restore::xsave_current(prev)`
2. `prev.set_state(Runnable)`
3. `context_switch_asm(&mut prev.kernel_rsp, next.kernel_rsp, next.cr3)` ← bascule RSP
4. `next.set_state(Running)`
5. `next.set_fpu_loaded(false)` → CR0.TS sera mis à 1 lors du retour lazy

> Le `context_switch_asm` est implémenté dans `asm/switch_asm.s` (voir SCHEDULER_ASM.md).

### schedule_yield

```rust
pub unsafe fn schedule_yield(
    current: &mut ThreadControlBlock,
    rq: &mut PerCpuRunQueue,
)
```

1. Ré-enfile `current` dans la run queue si `Runnable`
2. Appelle `pick_next_task(rq, current)`
3. Si un autre thread est sélectionné → `context_switch(current, next)`
4. Sinon → retour immédiat (seul thread prêt)

---

## 3. preempt.rs — Guards RAII

### Constante

```rust
pub const MAX_CPUS: usize = 64;
```

### Compteur de préemption

Chaque CPU possède un compteur `preempt_count: i32` dans un tableau statique aligné. La préemption est **désactivée** quand `preempt_count > 0`.

### PreemptGuard

```rust
pub struct PreemptGuard { _priv: () }

impl PreemptGuard {
    pub fn new() -> Self          // preempt_count += 1
    pub fn is_preempted_disabled() -> bool
    pub fn depth() -> i32
}

impl Drop for PreemptGuard {
    fn drop(&mut self) {          // preempt_count -= 1
        // si count == 0 et NEED_RESCHED → schedule
    }
}
```

**Utilisation typique** :

```rust
let _guard = PreemptGuard::new();  // Désactive la préemption
// ... section critique ...
// drop(_guard) à la sortie du scope → réactive
```

### IrqGuard

```rust
pub struct IrqGuard {
    saved_rflags: u64,
    _priv: (),
}

impl IrqGuard {
    pub fn new() -> Self               // cli + sauvegarde RFLAGS
    pub fn irqs_were_enabled(&self) -> bool
}

impl Drop for IrqGuard {
    fn drop(&mut self) {               // Restaure RFLAGS (sti si nécessaire)
    }
}
```

### Assertions

```rust
pub fn assert_preempt_disabled()   // panic! si préemption activée
pub fn assert_preempt_enabled()    // panic! si préemption désactivée
pub fn total_preempt_disable_count() -> i32
```

---

## 4. runqueue.rs — PerCpuRunQueue

### Constantes

```rust
pub const MAX_TASKS_PER_CPU:    usize = 512;   // Slots CFS max
pub const RT_LEVELS:            usize = 100;   // Priorités RT 0..99
pub const RR_TIMESLICE_MS:      u64   = 10;    // Quantum Round-Robin (ms)
pub const CFS_MIN_GRANULARITY_US: u64 = 750;   // Tranche CFS minimale (µs)
pub const CFS_TARGET_LATENCY_MS:  u64 = 6;     // Latence cible CFS (ms)
```

### Structure de la file

```
PerCpuRunQueue
├── rt_queue : RtRunQueue
│     ├── bitmap : RtBitmap (128 bits), O(1) find_first_set
│     ├── heads[100] : u16 (indice tête de chaque niveau)
│     └── entries[256] : RtEntry { tcb, next, prev }
│
├── cfs_heap : [Option<NonNull<TCB>>; 512]  (min-heap par vruntime)
│     └── nr_cfs: usize
│
├── idle_thread : Option<NonNull<TCB>>
│
├── stats : RunQueueStats
│     ├── nr_running: AtomicU32
│     ├── total_switches: AtomicU64
│     └── last_balance_tick: AtomicU64
│
└── cpu : CpuId
```

### API principale

```rust
impl PerCpuRunQueue {
    pub fn new(cpu: CpuId) -> Self
    pub fn set_idle_thread(&mut self, idle: NonNull<ThreadControlBlock>)

    // Enfile selon la politique du TCB (RT → rt_queue, Normal → cfs_heap, etc.)
    pub fn enqueue(&mut self, tcb: NonNull<ThreadControlBlock>)

    // Retire de la file, retourne false si non trouvé
    pub fn remove(&mut self, tcb: NonNull<ThreadControlBlock>) -> bool

    // Sélectionne le prochain thread O(1) : RT > CFS > Idle
    pub fn pick_next(&mut self) -> Option<NonNull<ThreadControlBlock>>

    pub fn nr_running(&self) -> u32
    pub fn timeslice_for(&self, tcb: NonNull<ThreadControlBlock>) -> u64

    // Avance l'horloge interne CFS
    pub fn advance_clock(&mut self, delta_ns: u64)

    // Migration : retire le thread CFS le moins prioritaire
    pub fn cfs_dequeue_for_migration(&mut self, dst_cpu: CpuId)
        -> Option<NonNull<ThreadControlBlock>>
}
```

### Algorithme `enqueue`

```
Si policy == Fifo || RoundRobin → rt_queue.insert(prio, tcb)
Si policy == Normal             → cfs_heap.push(tcb) par vruntime
Si policy == Deadline           → deadline_timer::dl_enqueue()
Si policy == Idle               → idle_thread = Some(tcb)
nr_running += 1
```

### Algorithme `pick_next` (O(1))

```
1. Si rt_queue.bitmap non vide     → dequeue_highest_rt()
2. Sinon si dl_pick_next() ≠ None  → thread EDF arrivant à échéance
3. Sinon si cfs_heap non vide      → cfs_heap.pop() (min vruntime)
4. Sinon                           → idle_thread
```

### Accès global

```rust
// Unsafe : accès direct au tableau statique
pub unsafe fn run_queue(cpu: CpuId) -> &'static mut PerCpuRunQueue

// Initialise le tableau per-CPU (appelé par scheduler::init)
pub fn init_percpu(nr_cpus: usize)
```

---

## 5. pick_next.rs — Algorithme O(1)

### Compteurs globaux

```rust
pub static PICK_NEXT_TOTAL:      AtomicU64 = AtomicU64::new(0);
pub static PICK_SAME_CURRENT:    AtomicU64 = AtomicU64::new(0);
pub static PICK_RT_RT:           AtomicU64 = AtomicU64::new(0);
pub static PICK_SKIP_INELIGIBLE: AtomicU64 = AtomicU64::new(0);
```

### PickResult

```rust
pub enum PickResult {
    Switch(NonNull<ThreadControlBlock>),  // Basculer vers ce thread
    KeepCurrent,                           // Continuer avec le thread courant
    Idle,                                  // Passer au thread idle
}
```

### pick_next_task

```rust
pub unsafe fn pick_next_task(
    rq: &mut PerCpuRunQueue,
    current: &ThreadControlBlock,
) -> PickResult
```

Logique :
1. Incrémente `PICK_NEXT_TOTAL`.
2. Appelle `ai_guided::maybe_prefer(rq, candidate)` si `AI_HINTS_ENABLED`.
3. Si RT courant et RT candidat de même priorité → `KeepCurrent` (`PICK_RT_RT`).
4. Si candidat inéligible (état != Runnable, affinité) → `PICK_SKIP_INELIGIBLE`.
5. Si aucun meilleur candidat → `KeepCurrent` (`PICK_SAME_CURRENT`).
6. Sinon → `Switch(next)`.

### account_time

```rust
pub unsafe fn account_time(tcb: &ThreadControlBlock, delta_ns: u64)
```

Avance `vruntime` du thread courant en fin de tranche (appelé par `tick.rs`).
