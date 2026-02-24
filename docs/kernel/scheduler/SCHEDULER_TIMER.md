# Scheduler Timer — Tick, HrTimer, Clock TSC, Deadline Timer

> **Sources** : `kernel/src/scheduler/timer/`  
> **Fréquence** : HZ = 1000 (1 tick = 1 ms)

---

## Table des matières

1. [tick.rs — scheduler_tick](#1-tickrs--scheduler_tick)
2. [clock.rs — TSC et temps monotone](#2-clockrs--tsc-et-temps-monotone)
3. [hrtimer.rs — Timers haute résolution](#3-hrtimers--timers-haute-résolution)
4. [deadline_timer.rs — Min-heap EDF](#4-deadline_timerrs--min-heap-edf)

---

## 1. tick.rs — scheduler_tick

### Constantes

```rust
pub const HZ:      u64 = 1000;                        // Fréquence des ticks (ticks/s)
pub const TICK_NS: u64 = 1_000_000_000 / HZ;           // Durée d'un tick = 1 ms en ns
```

### Compteurs globaux

```rust
pub static TICK_COUNT:       AtomicU64  // Nombre total de ticks depuis le boot
pub static TICK_PREEMPTIONS: AtomicU64  // Préemptions déclenchées par tick
```

### scheduler_tick — export C ABI

```rust
#[no_mangle]
pub unsafe extern "C" fn scheduler_tick(
    cpu_id: u32,
    current: *mut ThreadControlBlock,
)
```

Appelé depuis le handler du timer périodique dans `arch/x86_64/interrupts/` à chaque tick.

**Séquence complète** :

```
1. TICK_COUNT.fetch_add(1)
2. stats::per_cpu::inc_ticks(cpu_id)
3. drain_pending_migrations(cpu_id, rq)       ← traite IPI entrants

4. delta_ns = monotonic_ns() - last_schedule_time
   tcb.advance_vruntime(delta_ns, policy.cfs_weight())
   stats::per_cpu::add_run_time(cpu_id, delta_ns)

5. Selon la politique :
   a. CFS   : cfs::tick_check_preempt(tcb, rq, elapsed)
              → si préemption nécessaire : tcb.request_preemption()
   b. RR    : realtime::rr_tick(tcb, elapsed)
              → si quantum expiré : ré-enfile en fin de file, NEED_RESCHED
   c. EDF   : deadline::deadline_tick(tcb, elapsed)
              → si budget épuisé : NEED_RESCHED
              → si miss : DEADLINE_MISSES++

6. hrtimer::fire_expired(cpu_id)               ← callbacks timers expirés

7. Si TICK_COUNT % BALANCE_INTERVAL_TICKS == 0 :
   smp::load_balance::balance_cpu(cpu_id)

8. energy::power_profile::maybe_update_pstate() si profile = Balanced
```

### sched_ipi_reschedule — export C ABI

```rust
#[no_mangle]
pub unsafe extern "C" fn sched_ipi_reschedule(
    tcb_ptr: *mut u8,
)
```

Appelé depuis le handler IPI (vecteur dédié dans IDT) sur le CPU cible :
1. `tcb.flags |= NEED_RESCHED` (AtomicU32 fetch_or).
2. Utilisé pour la migration (signale au CPU de re-sélectionner sa tâche).

### init

```rust
pub unsafe fn init(_nr_cpus: usize)
```
Remet TICK_COUNT et TICK_PREEMPTIONS à zéro. Simple initialisation de compteurs.

---

## 2. clock.rs — TSC et temps monotone

### Initialisation

```rust
pub unsafe fn init(tsc_hz: u64)
```
- Stocke `TSC_HZ` (fréquence du TSC en Hz, mesurée ou fournie par BIOS/ACPI).
- Calcule `NS_PER_TSC_SHIFT` pour la conversion TSC → ns sans division.

### Lecture TSC

```rust
// RDTSC — non-sérialisé (peut être réordonné)
pub fn rdtsc() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

// RDTSCP — sérialisé (garantit l'ordre, lit aussi cpu_id dans ECX)
pub fn rdtscp() -> u64 {
    let mut aux: u32 = 0;
    unsafe { core::arch::x86_64::__rdtscp(&mut aux) }
}
```

### Conversion TSC → ns

```rust
pub fn tsc_to_ns(tsc_delta: u64) -> u64
```

Formule sans division flottante :
```
ns = tsc_delta × NS_SCALE >> 32
```
où `NS_SCALE = (10^9 × 2^32) / TSC_HZ` est précalculé à l'init.

### Temps monotone

```rust
// Nanosecondes depuis le boot (monotoniq, TSC-based)
pub fn monotonic_ns() -> u64 {
    tsc_to_ns(rdtsc() - BOOT_TSC)
}

// Microsecondes depuis le boot
pub fn monotonic_us() -> u64 {
    monotonic_ns() / 1000
}
```

**Propriétés** : monotone, sans drift, sans NTP. Utilisé pour vruntime, latency tracking, timeouts.

### Temps réel

```rust
// Fixe l'offset d'epoch (fourni par RTC ou NTP via syscall)
pub fn set_realtime_offset(epoch_ns: u64)

// ns depuis epoch UNIX (non monotone si set_realtime_offset appelé)
pub fn realtime_ns() -> u64 {
    monotonic_ns() + REALTIME_OFFSET.load(SeqCst)
}
```

---

## 3. hrtimer.rs — Timers haute résolution

### Type callback

```rust
type HrTimerCallback = unsafe fn(cpu: usize, timer_id: u32, data: u64);
```

### Compteurs

```rust
pub static HRTIMER_FIRED:     AtomicU64
pub static HRTIMER_CANCELLED: AtomicU64
```

### Initialisation

```rust
pub unsafe fn init(nr_cpus: usize)
```
Alloue les tableaux de slots de timers per-CPU (tableau statique).

### Gestion des timers

```rust
// Arme un timer : déclenché dans delay_ns nanosecondes
// Retourne un ID pour annulation ultérieure
pub unsafe fn arm(
    cpu: usize,
    delay_ns: u64,
    data: u64,
    cb: HrTimerCallback,
) -> u32

// Annule un timer par ID
// Retourne true si le timer existait et a été annulé
pub unsafe fn cancel(cpu: usize, id: u32) -> bool

// Exécute les callbacks des timers expirés (appelé depuis scheduler_tick)
// Retourne le nombre de timers déclenchés
pub unsafe fn fire_expired(cpu: usize) -> usize
```

### Fonctionnement

Chaque CPU possède un tableau de `HrTimerSlot` :
```rust
struct HrTimerSlot {
    deadline_ns: u64,           // Déclenchement absolu en ns (monotonic_ns)
    data:        u64,
    callback:    HrTimerCallback,
    id:          u32,
    active:      bool,
}
```

`fire_expired` itère le tableau et appelle les callbacks des slots dont `deadline_ns ≤ monotonic_ns()`.

**Précision** : limitée par `TICK_NS = 1 ms`. Les timers sub-ms sont arrondis au prochain tick. Pour des timers plus précis, un HPET ou APIC timer en one-shot serait nécessaire.

---

## 4. deadline_timer.rs — Min-heap EDF

### Compteurs

```rust
pub static DL_ENQUEUES:    AtomicU64  // Threads EDF enfilés
pub static DL_DEQUEUES:    AtomicU64  // Threads EDF défilés
pub static DL_MISS_EVENTS: AtomicU64  // Deadline misses détectés
```

### Initialisation

```rust
pub unsafe fn init(nr_cpus: usize)
```
Alloue les min-heaps per-CPU (tableau statique de `DL_TIMER_CAPACITY = 64` slots).

### API

```rust
// Enfile un thread EDF dans le min-heap (trié par abs_deadline)
pub unsafe fn dl_enqueue(
    cpu: usize,
    tcb_ptr: NonNull<ThreadControlBlock>,
)

// Retourne le thread dont l'échéance absolue est la plus proche
// (sans le retirer du heap)
pub unsafe fn dl_pick_next(cpu: usize)
    -> Option<NonNull<ThreadControlBlock>>

// Tick EDF : décrémente budgets, détecte misses, refresh échéances
pub unsafe fn dl_tick(cpu: usize)
```

### dl_tick

```
Pour chaque TCB EDF dans le heap :
  1. deadline::deadline_tick(tcb, TICK_NS)
     → si budget = 0 : tcb.request_preemption()
  2. Si now > abs_deadline : DL_MISS_EVENTS++, deadline::check_deadline_miss()
  3. Si période terminée : deadline::refresh_deadline(tcb)
```

### Min-heap EDF

Le heap est trié par `tcb.deadline.abs_deadline` croissant.
`dl_pick_next()` retourne le sommet (thread avec deadline absolue la plus proche).

```
         [d=10ms]
        /         \
   [d=15ms]    [d=20ms]
   /    \
[d=30ms] [d=50ms]
```

L'EDF est optimal (minimise les deadline misses) pour des tâches à utilisation ≤ 1.
