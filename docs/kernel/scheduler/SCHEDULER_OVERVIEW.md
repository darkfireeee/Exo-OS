# Scheduler Exo-OS — Vue d'ensemble

> **Couche** : 1 (SCHED-01)  
> **Répertoire source** : `kernel/src/scheduler/`  
> **Compilation validée** : `cargo build` → 0 erreur, 0 avertissement  
> **Règles de référence** : `docs/refonte/DOC3_MODULE_SCHEDULER_FIXED.md`

---

## Table des matières

1. [Positionnement dans l'architecture](#1-positionnement-dans-larchitecture)
2. [Arborescence des modules](#2-arborescence-des-modules)
3. [Règles SCHED (DOC3) — récapitulatif](#3-règles-sched-doc3--récapitulatif)
4. [Séquence d'initialisation](#4-séquence-dinitialisation)
5. [Flux de scheduling normal](#5-flux-de-scheduling-normal)
6. [Points d'entrée publics (mod.rs)](#6-points-dentrée-publics-modrs)
7. [Dépendances inter-modules](#7-dépendances-inter-modules)
8. [Ponts FFI (vue synthétique)](#8-ponts-ffi-vue-synthétique)
9. [Fichiers de documentation détaillée](#9-fichiers-de-documentation-détaillée)

---

## 1. Positionnement dans l'architecture

```
┌─────────────────────────────────────────────────────────┐
│ process/   ipc/   fs/   security/   syscall/            │  Couches supérieures
├─────────────────────────────────────────────────────────┤
│               scheduler/   (Couche 1)                   │  ← CE MODULE
├─────────────────────────────────────────────────────────┤
│  memory/  (allocateur, emergency_pool, frame)           │  Couche 0
├─────────────────────────────────────────────────────────┤
│  arch/x86_64/  (CPU, FPU, GDT, IDT, SMP, TSC)          │  Matériel
└─────────────────────────────────────────────────────────┘
```

Le scheduler est en **Couche 1** :
- Il dépend **exclusive­ment** de `memory/` (SCHED-01).
- `process/`, `ipc/`, `fs/` **ne sont jamais importés** dans `scheduler/`.
- `arch/` est accessible uniquement via des **ponts FFI C ABI**.

---

## 2. Arborescence des modules

```
kernel/src/scheduler/
├── mod.rs                  ← init(), init_ap(), re-exports publics
│
├── core/
│   ├── task.rs             ← ThreadControlBlock (128 B, 2 cache lines)
│   ├── switch.rs           ← context_switch(), schedule_yield()
│   ├── preempt.rs          ← PreemptGuard, IrqGuard (RAII)
│   ├── runqueue.rs         ← PerCpuRunQueue (RT-bitmap + CFS-heap + Idle)
│   └── pick_next.rs        ← pick_next_task() O(1)
│
├── asm/
│   ├── switch_asm.s        ← Sauvegarde registres, MXCSR, CR3 (AT&T)
│   └── fast_path.s         ← Retour rapide depuis interrupt
│
├── fpu/
│   ├── lazy.rs             ← CR0.TS, handle_nm_exception
│   ├── save_restore.rs     ← XSAVE/XRSTOR via FFI arch/
│   ├── state.rs            ← FpuState (2688 B, align 64)
│   └── mod.rs
│
├── policies/
│   ├── cfs.rs              ← Completely Fair Scheduler (vruntime)
│   ├── realtime.rs         ← SCHED_FIFO / SCHED_RR
│   ├── deadline.rs         ← SCHED_DEADLINE (EDF)
│   ├── idle.rs             ← Idle thread + HLT loop
│   ├── ai_guided.rs        ← Tables lookup EMA (pas d'inférence ML)
│   └── mod.rs
│
├── smp/
│   ├── load_balance.rs     ← Équilibrage toutes les 4 ticks
│   ├── migration.rs        ← IPI reschedule via FFI arch/
│   ├── affinity.rs         ← CpuMask (u64 bitmap, 64 CPUs max)
│   ├── topology.rs         ← Nœuds NUMA, distances
│   └── mod.rs
│
├── sync/
│   ├── wait_queue.rs       ← WaitQueue + WaitNode (EmergencyPool)
│   ├── mutex.rs            ← KMutex<T> (blocking)
│   ├── rwlock.rs           ← KRwLock<T>
│   ├── spinlock.rs         ← SpinLock<T>, IrqSpinLock<T>
│   ├── condvar.rs          ← CondVar
│   ├── barrier.rs          ← KBarrier
│   └── mod.rs
│
├── timer/
│   ├── tick.rs             ← scheduler_tick() @HZ=1000
│   ├── hrtimer.rs          ← Timers haute résolution (ns)
│   ├── clock.rs            ← TSC, monotonic_ns(), realtime_ns()
│   ├── deadline_timer.rs   ← Min-heap EDF par CPU
│   └── mod.rs
│
├── energy/
│   ├── c_states.rs         ← C0/C1/C2/C3, fetch_min (CSTATE-01)
│   ├── frequency.rs        ← P-states, MSR 0x199
│   ├── power_profile.rs    ← Profils Performance/Balanced/PowerSave
│   └── mod.rs
│
└── stats/
    ├── per_cpu.rs          ← CpuStats (context switches, run time…)
    ├── latency.rs          ← LatencyHist (p50/p99/p99.9)
    └── mod.rs
```

---

## 3. Règles SCHED (DOC3) — récapitulatif

| Règle | Description | Statut |
|-------|-------------|--------|
| SCHED-01 | Couche 1 : dépend uniquement de `memory/` | ✅ |
| SCHED-02 | Tous les types partagés définis dans `core/task.rs` | ✅ |
| SCHED-03 | `ThreadControlBlock` = 128 B exactement (2×64 B cache lines) | ✅ |
| SCHED-04 | `SchedPolicy` couvre FIFO/RR/CFS/DEADLINE/IDLE | ✅ |
| SCHED-05 | `WaitNode` alloué uniquement depuis `SchedNodePool` / `EmergencyPool` | ✅ |
| WAITQ-01 | `emergency_pool_alloc_wait_node` / `emergency_pool_free_wait_node` via FFI | ✅ |
| SCHED-06 | `switch_asm.s` : r15 sauvegardé EN PREMIER | ✅ |
| SCHED-07 | MXCSR + x87 FCW sauvegardés explicitement dans la pile | ✅ |
| SCHED-08 | Changement CR3 AVANT restauration des registres | ✅ |
| SCHED-09 | FPU lazy : `xsave_current(prev)` AVANT switch, `set_fpu_loaded(false)` APRÈS | ✅ |
| SCHED-10 | Lock ordering : locks scheduler < locks memory | ✅ |
| SCHED-11 | `PreemptGuard` / `IrqGuard` : RAII, pas d'unlock manuel | ✅ |
| SCHED-12 | `pick_next_task()` : O(1) via RT-bitmap + CFS-heap | ✅ |
| SCHED-13 | Init sequence en 11 étapes dans `mod.rs::init()` | ✅ |
| SCHED-14 | `AI_HINTS_ENABLED` atomique, fallback systématique vers CFS | ✅ |
| SCHED-15 | `signal_pending: AtomicBool` — scheduler READ-ONLY | ✅ |
| SCHED-16 | Instructions FPU uniquement dans `arch/x86_64/cpu/fpu.rs` | ✅ |
| CSTATE-01 | `max_allowed_cstate()` utilise `fetch_min` atomique | ✅ |

---

## 4. Séquence d'initialisation

```
scheduler::init(params)                          (mod.rs:70)
  │
  ├── 1. preempt::init(nr_cpus)                  Compteurs préemption per-CPU
  ├── 2. topology::init(nr_cpus, nr_nodes)        Carte CPU/NUMA
  ├── 3. runqueue::init_percpu(nr_cpus)           Alloc files per-CPU (statique)
  ├── 4. tick::init(nr_cpus)                      Compteurs ticks
  ├── 5. hrtimer::init(nr_cpus)                   Tableaux hrTimer per-CPU
  ├── 6. deadline_timer::init(nr_cpus)            Min-heap EDF per-CPU
  ├── 7. c_states::init(nr_cpus)                  Contraintes C-state per-CPU
  ├── 8. latency::init()                          LatencyHist globaux
  ├── 9. wait_queue::init()                       Pool WaitNodes
  ├── 10. fpu::save_restore::init()               Détection XSAVE/AVX/AVX-512
  └── 11. fpu::lazy::init()                       Set CR0.TS=1 sur ce CPU

scheduler::init_ap(cpu_id)                       (mod.rs:115)
  └── Réplique les étapes CPU-locales sur chaque CPU secondaire (AP)
```

---

## 5. Flux de scheduling normal

```
Timer IRQ (HZ=1000)
  └──► scheduler_tick(cpu_id, current_tcb)        [tick.rs — export C ABI]
         ├── inc_context_switches() / inc_ticks()
         ├── advance_vruntime(delta_ns, weight)
         ├── cfs::tick_check_preempt()  → set NEED_RESCHED si dépassement
         ├── realtime::rr_tick()        → set NEED_RESCHED si quantum expiré
         ├── deadline::deadline_tick()  → detection miss
         ├── fire_expired(hrtimers)
         └── balance_cpu() si tick % 4 == 0

Retour vers le thread courant
  └──► check signal_pending (read-only)
         │
         └── si NEED_RESCHED → schedule_yield() ou context_switch()

context_switch(prev, next)                       [switch.rs]
  ├── 1. xsave_current(prev)  si FPU loaded
  ├── 2. prev.set_state(Runnable)
  ├── 3. context_switch_asm(&prev.kernel_rsp, next.kernel_rsp, next.cr3)
  │         ┌── switch_asm.s ──────────────────────────────────────────┐
  │         │  push r15, r14, r13, r12, rbp, rbx                      │
  │         │  sub $16, %rsp                                           │
  │         │  stmxcsr 0(%rsp)   # MXCSR → pile                       │
  │         │  fstcw  8(%rsp)   # FCW → pile                          │
  │         │  mov %rsp, (%rdi) # sauve RSP prev                      │
  │         │  mov next.cr3, %rax                                      │
  │         │  mov %rax, %cr3   # switch KPTI/PCID                    │
  │         │  mov (%rsi), %rsp # charge RSP next                     │
  │         │  fldcw  8(%rsp)                                          │
  │         │  ldmxcsr 0(%rsp)                                         │
  │         │  add $16, %rsp                                           │
  │         │  pop rbx, rbp, r12, r13, r14, r15                       │
  │         │  ret                                                     │
  │         └──────────────────────────────────────────────────────────┘
  ├── 4. next.set_state(Running)
  └── 5. next.set_fpu_loaded(false)
```

**Lazy FPU** : si le thread `next` tente une instruction FPU, CR0.TS=1 provoque une exception #NM → `sched_fpu_handle_nm(tcb_ptr)` → `handle_nm_exception()` → `clts + xrstor_for(next)` → CR0.TS=0.

---

## 6. Points d'entrée publics (mod.rs)

```rust
// Initialisation
pub unsafe fn init(params: &SchedInitParams);
pub unsafe fn init_ap(cpu_id: u32);

// Scheduling
pub use core::switch::{context_switch, schedule_yield};
pub use core::pick_next::pick_next_task;
pub use core::runqueue::run_queue;

// Types de tâche
pub use core::task::{
    ThreadControlBlock, ThreadId, ProcessId, CpuId,
    TaskState, SchedPolicy, Priority,
};

// Primitives de protection
pub use core::preempt::{PreemptGuard, IrqGuard};

// Temps
pub use timer::clock::monotonic_ns;

// Callback tick (export C ABI pour arch/)
pub use timer::tick::{scheduler_tick, HZ};

// Hints AI
pub use policies::ai_guided::AI_HINTS_ENABLED;
```

---

## 7. Dépendances inter-modules

```
scheduler/
  ├── → memory/ (FFI)
  │     ├── emergency_pool_alloc_wait_node()
  │     ├── emergency_pool_free_wait_node()
  │     └── __rust_alloc / __rust_dealloc (FPU state buffer)
  │
  ├── → arch/ (FFI)
  │     ├── arch_xsave64 / arch_xrstor64
  │     ├── arch_fxsave64 / arch_fxrstor64
  │     ├── arch_has_xsave / arch_has_avx
  │     ├── arch_current_cpu()
  │     ├── arch_send_reschedule_ipi(cpu)
  │     └── arch_set_cpu_pstate(cpu, pstate)
  │
  └── ← arch/ (export C ABI vers arch/)
        ├── sched_fpu_handle_nm(tcb_ptr)    [fpu/lazy.rs]
        └── sched_ipi_reschedule(tcb_ptr)   [timer/tick.rs]
```

**Règle stricte** : scheduler/ n'importe **jamais** `process::`, `ipc::`, `fs::`, `security::`, `syscall::`.

---

## 8. Ponts FFI (vue synthétique)

| Direction | Symbole C | Fichier Rust | Description |
|-----------|-----------|--------------|-------------|
| sched→arch | `arch_xsave64(ptr, mask)` | `fpu/save_restore.rs` | Sauvegarde état FPU |
| sched→arch | `arch_xrstor64(ptr, mask)` | `fpu/save_restore.rs` | Restauration état FPU |
| sched→arch | `arch_has_xsave() → bool` | `fpu/save_restore.rs` | Détection CPUID |
| sched→arch | `arch_has_avx() → bool` | `fpu/save_restore.rs` | Détection AVX |
| sched→arch | `arch_current_cpu() → u32` | `smp/migration.rs` | CPU courant |
| sched→arch | `arch_send_reschedule_ipi(u32)` | `smp/migration.rs` | Envoyer IPI |
| sched→arch | `arch_set_cpu_pstate(u32, u32)` | `energy/frequency.rs` | MSR 0x199 |
| sched→mem  | `emergency_pool_alloc_wait_node() → *mut WaitNode` | `sync/wait_queue.rs` | Allouer nœud |
| sched→mem  | `emergency_pool_free_wait_node(*mut WaitNode)` | `sync/wait_queue.rs` | Libérer nœud |
| arch→sched | `sched_fpu_handle_nm(*mut u8)` | `fpu/lazy.rs` | Handler #NM |
| arch→sched | `sched_ipi_reschedule(*mut u8)` | `timer/tick.rs` | Handler IPI |

---

## 9. Fichiers de documentation détaillée

| Fichier | Contenu |
|---------|---------|
| [SCHEDULER_CORE.md](SCHEDULER_CORE.md) | TCB, context_switch, PreemptGuard, RunQueue, pick_next |
| [SCHEDULER_ASM.md](SCHEDULER_ASM.md) | switch_asm.s, fast_path.s — encodage registres |
| [SCHEDULER_FPU.md](SCHEDULER_FPU.md) | Lazy FPU, XSAVE/XRSTOR, FpuState layout |
| [SCHEDULER_POLICIES.md](SCHEDULER_POLICIES.md) | CFS, RT FIFO/RR, EDF, Idle, AI-guided |
| [SCHEDULER_SMP.md](SCHEDULER_SMP.md) | Load balancing, migration IPI, affinité, topologie NUMA |
| [SCHEDULER_SYNC.md](SCHEDULER_SYNC.md) | WaitQueue, KMutex, KRwLock, SpinLock, CondVar, KBarrier |
| [SCHEDULER_TIMER.md](SCHEDULER_TIMER.md) | scheduler_tick, hrtimer, clock TSC, deadline_timer |
| [SCHEDULER_ENERGY.md](SCHEDULER_ENERGY.md) | C-states, P-states, profils d'alimentation |
| [SCHEDULER_STATS.md](SCHEDULER_STATS.md) | CpuStats, LatencyHist (p50/p99/p99.9) |
| [SCHEDULER_FFI_BRIDGES.md](SCHEDULER_FFI_BRIDGES.md) | Carte complète des ponts FFI arch↔sched↔memory |
