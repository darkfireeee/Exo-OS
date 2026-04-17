# 📋 DOC 3 — MODULE SCHEDULER/ : CONCEPTION COMPLÈTE
> Exo-OS · Couche 1 · Dépend uniquement de memory/
> Règles anti-crash · anti-deadlock · anti-latence · anti-race

---

## POSITION DANS L'ARCHITECTURE

```
┌─────────────────────────────────────────────────────────┐
│  scheduler/  ← COUCHE 1                                  │
│                                                         │
│  DÉPEND DE : memory/ uniquement                         │
│  EST APPELÉ PAR : ipc/, fs/, process/, arch/            │
│  signal/ : ABSENT — déplacé dans process/ (voir DOC1)   │
│  futex.rs : ABSENT — dans memory/utils/futex_table.rs   │
└─────────────────────────────────────────────────────────┘
```

**OBJECTIFS DE PERFORMANCE :**

| Métrique | Cible | Linux ref |
|---|---|---|
| Context switch | 500–800 cycles | ~2134 cycles |
| pick_next_task() | 100–150 cycles | ~200 cycles |
| Wakeup thread | <2µs end-to-end | — |
| IPI latence SMP | <10µs | ~20–50µs |

---

## ARBORESCENCE COMPLÈTE

```
kernel/src/scheduler/
├── mod.rs                          # API publique
│
├── core/
│   ├── mod.rs
│   ├── task.rs                     # Task struct (TCB — cache-aligned 64B)
│   ├── runqueue.rs                 # Run queue per-CPU (3 files RT/Normal/Idle)
│   ├── pick_next.rs                # pick_next_task() O(1) — 100-150 cycles
│   ├── switch.rs                   # Dispatch → switch_asm.s
│   └── preempt.rs                  # PreemptGuard RAII (jamais direct disable/enable)
│
├── asm/
│   ├── switch_asm.s                # Context switch ASM COMPLET
│   │                               # Callee-saved ABI: rbx,rbp,r12-r15,rsp
│   │                               # + MXCSR + x87 FCW (hors état XSAVE)
│   │                               # CR3 switché ICI pour KPTI (avant restauration regs)
│   │                               # ✅ GARANTIE FORMELLE r15: push AVANT tout appel C
│   │                               # (ext4plus/inode/ops.rs utilise r15 — garanti préservé)
│   └── fast_path.s                 # Fast path scheduler (pas d'appel C)
│
├── policies/
│   ├── mod.rs
│   ├── cfs.rs                      # CFS (SCHED_NORMAL/BATCH)
│   ├── realtime.rs                 # RT (SCHED_FIFO/SCHED_RR)
│   ├── deadline.rs                 # EDF (SCHED_DEADLINE)
│   ├── idle.rs                     # IDLE policy
│   └── ai_guided.rs                # [IA] Politique guidée par hints NUMA statiques
│                                   # Fallback immédiat vers cfs.rs si hints absents
│
├── smp/
│   ├── mod.rs
│   ├── load_balance.rs             # Load balancing inter-CPU
│   ├── migration.rs                # Thread migration push/pull
│   ├── affinity.rs                 # cpumask affinity
│   └── topology.rs                 # Topologie SMP NUMA-aware
│
├── sync/
│   ├── mod.rs
│   ├── wait_queue.rs               # WaitQueue — utilise EmergencyPool UNIQUEMENT
│   ├── mutex.rs                    # Kernel mutex (priority inheritance)
│   ├── rwlock.rs                   # RW lock
│   ├── spinlock.rs                 # Spinlock (irq-safe)
│   ├── condvar.rs                  # Condition variable
│   └── barrier.rs                  # Barrière SMP
│   # NOTE: futex.rs ABSENT — dans memory/utils/futex_table.rs (RÈGLE SCHED-03)
│
├── timer/
│   ├── mod.rs
│   ├── hrtimer.rs                  # High-resolution timers (nanoseconde)
│   ├── tick.rs                     # Scheduler tick (HZ=1000)   ← ✅ CORRIGÉ: HZ précisé
│   ├── clock.rs                    # Monotonic/realtime clock
│   └── deadline_timer.rs           # Timer SCHED_DEADLINE
│
├── fpu/                            # Logique d'état FPU — NE PAS confondre avec arch/cpu/fpu.rs
│   ├── mod.rs                      # arch/cpu/fpu.rs = instructions ASM brutes (XSAVE/XRSTOR)
│   │                               # scheduler/fpu/ = logique état (flag lazy_fpu_used dans TCB)
│   ├── lazy.rs                     # Lazy FPU save (CR0.TS, #NM exception)
│   ├── save_restore.rs             # XSAVE/XRSTOR — appelle arch::cpu::fpu (instructions brutes)
│   └── state.rs                    # ✅ AJOUT: FpuState par thread (512B aligné)
│                                   # Absent du DOC3 original, nécessaire pour save_restore.rs
│
├── energy/
│   ├── mod.rs
│   ├── c_states.rs                 # C-state governor (contrainte par threads RT)
│   ├── frequency.rs                # Budget RT corrigé par fréquence CPU
│   └── power_profile.rs            # Profile perf/éco
│
└── stats/
    ├── mod.rs
    ├── per_cpu.rs                  # Stats par CPU (cycles, switches, migrations)
    └── latency.rs                  # Histogramme latences p50/p99/p999
```

---

## RÈGLES PAR SOUS-MODULE

---

### 📌 scheduler/asm/switch_asm.s

**RÈGLE SWITCH-01 : Sauvegarder TOUS les registres callee-saved + MXCSR + x87 FCW**

```asm
# kernel/src/scheduler/asm/switch_asm.s
#
# Callee-saved System V ABI (Rust suppose qu'ils sont préservés) :
#   rbx, rbp, r12, r13, r14, r15, rsp
#
# MXCSR et x87 FCW :
#   Sauvegardés EXPLICITEMENT dans switch_asm (pas via XSAVE)
#   XSAVE est géré séparément par scheduler/fpu/lazy.rs (si FPU active)
#   Sans cette sauvegarde explicite, du code Rust compilé avec des
#   optimisations SSE pourrait corrompre MXCSR entre deux threads.
#
# CR3 (KPTI) :
#   Switché ICI atomiquement, AVANT la restauration des registres du nouveau thread

.section .text
.global context_switch_asm
.type context_switch_asm, @function

# Signature (System V ABI):
#   context_switch_asm(old_rsp: *mut u64, new_rsp: u64, new_cr3: u64)
#                      rdi               rsi            rdx

context_switch_asm:
    # Sauvegarder les registres callee-saved du thread SORTANT
    push    %rbx
    push    %rbp
    push    %r12
    push    %r13
    push    %r14
    push    %r15            # ← OBLIGATOIRE (utilisé par ext4plus/inode/ops.rs)

    # Sauvegarder MXCSR (SSE control register — 32 bits)
    sub     $4, %rsp
    stmxcsr (%rsp)

    # Sauvegarder x87 FCW (control word — 16 bits, aligné sur 4)
    sub     $4, %rsp
    fstcw   (%rsp)

    # Sauvegarder rsp du thread sortant
    mov     %rsp, (%rdi)    # *old_rsp = rsp_courant

    # === POINT DE NON-RETOUR ===
    # CR3 switché ICI — AVANT de charger le nouveau rsp
    # Raison : atomique entre les deux threads, évite fenêtre avec mauvais CR3
    cmp     %rdx, %cr3
    je      .skip_cr3_switch
    mov     %rdx, %cr3      # Invalide TLB user automatiquement (PCID non utilisé ici)
.skip_cr3_switch:

    # Charger rsp du nouveau thread
    mov     %rsi, %rsp

    # Restaurer dans l'ordre INVERSE de la sauvegarde
    fldcw   (%rsp)          # x87 FCW
    add     $4, %rsp
    ldmxcsr (%rsp)          # MXCSR
    add     $4, %rsp

    pop     %r15
    pop     %r14
    pop     %r13
    pop     %r12
    pop     %rbp
    pop     %rbx

    ret     # Continue l'exécution du nouveau thread

.size context_switch_asm, . - context_switch_asm
```

**RÈGLE SWITCH-02 : Lazy FPU AVANT le switch, mark APRÈS**

```rust
// kernel/src/scheduler/core/switch.rs

pub fn context_switch(prev: &mut Task, next: &mut Task) {
    // 1. Lazy FPU : sauvegarder état FPU du thread SORTANT si utilisé
    //    DOIT se faire AVANT switch_asm (prev n'est plus courant après)
    fpu::lazy::save_if_active(prev);

    // 2. Changer le thread courant per-CPU
    percpu::set_current_task(next.as_ptr());

    // 3. CR3 pour KPTI
    let new_cr3 = next.address_space.page_table.cr3_value();

    // 4. Context switch ASM
    unsafe {
        context_switch_asm(
            &mut prev.saved_rsp,
            next.saved_rsp,
            new_cr3,
        );
    }
    // Ici : on exécute dans le contexte de 'next'

    // 5. Lazy FPU : marquer FPU comme "non chargée" pour le nouveau thread
    //    Prochain usage FPU → #NM → chargement état FPU de 'next'
    fpu::lazy::mark_fpu_not_loaded();
}
```

---

### 📌 scheduler/core/preempt.rs

**RÈGLE PREEMPT-01 : RAII obligatoire, jamais de disable/enable directs**

```rust
// kernel/src/scheduler/core/preempt.rs

/// Compteur de préemption per-CPU (dans le TCB ou per-CPU data)
/// 0 = préemptible, >0 = non préemptible
///
/// RÈGLE : Ne JAMAIS appeler preempt_disable/enable directement.
/// Toujours passer par PreemptGuard.
/// Raison : exception entre disable et enable = deadlock + compteur corrompu.

pub struct PreemptGuard {
    _phantom: PhantomData<*mut ()>,  // Non-Send pour éviter transfert entre threads
}

impl PreemptGuard {
    #[inline(always)]
    pub fn new() -> Self {
        // Incrémenter le compteur de préemption
        let count = percpu::preempt_count().fetch_add(1, Ordering::Relaxed);
        debug_assert!(count < 64, "PreemptGuard imbriqué trop profondément (count={})", count);
        Self { _phantom: PhantomData }
    }
}

impl Drop for PreemptGuard {
    #[inline(always)]
    fn drop(&mut self) {
        let count = percpu::preempt_count().fetch_sub(1, Ordering::Relaxed);
        debug_assert!(count > 0, "PreemptGuard::drop avec compteur déjà à 0 — double-drop");

        // Si on revient à 0 et qu'une préemption est pending → scheduler
        if count == 1 && percpu::preempt_pending() {
            schedule();
        }
    }
}

// Usage correct :
// {
//     let _guard = PreemptGuard::new();  // préemption désactivée
//     // ... section critique ...
// }  // Drop automatique → préemption réactivée
```

---

### 📌 scheduler/sync/wait_queue.rs

**RÈGLE WAITQ-01 : WaitNode depuis EmergencyPool UNIQUEMENT**

```rust
// kernel/src/scheduler/sync/wait_queue.rs
//
// RÈGLE ABSOLUE : WaitNode alloué depuis EmergencyPool (jamais depuis heap)
// Raison : wait_queue peut être appelé depuis un contexte de reclaim mémoire.
// Si on alloue depuis heap pendant reclaim → deadlock récursif.

pub struct WaitQueue {
    /// Liste des waiters — utilise des WaitNodes du EmergencyPool
    waiters: SpinLock<LinkedList<WaitNode>>,
}

impl WaitQueue {
    pub fn wait(&self, condition: impl Fn() -> bool, timeout: Option<Duration>) -> WaitResult {
        // Allouer depuis EmergencyPool — jamais depuis heap
        let node = memory::physical::frame::emergency_pool::alloc_wait_node()
            .expect("EmergencyPool épuisé — augmenter EMERGENCY_POOL_SIZE");

        node.thread_id = current_thread_id();
        node.woken = AtomicBool::new(false);

        {
            let mut waiters = self.waiters.lock();
            if condition() {
                // Condition déjà vraie → libérer et retourner immédiatement
                memory::physical::frame::emergency_pool::free_wait_node(node);
                return WaitResult::ConditionMet;
            }
            waiters.push(node);
        }

        // Se bloquer — le scheduler reprendra à condition d'être woken
        let result = scheduler::block_current(timeout);

        memory::physical::frame::emergency_pool::free_wait_node(node);
        result
    }

    pub fn wake_one(&self) {
        let mut waiters = self.waiters.lock();
        if let Some(node) = waiters.pop() {
            node.woken.store(true, Ordering::Release);
            scheduler::wake_thread(node.thread_id);
        }
    }
}
```

---

### 📌 scheduler/fpu/save_restore.rs

**RÈGLE FPU-01 : Séparation arch (instructions) / scheduler (logique état)**

```rust
// kernel/src/scheduler/fpu/save_restore.rs
//
// Ce module gère LA LOGIQUE d'état FPU (quand sauvegarder, pour quel thread).
// Les INSTRUCTIONS ASM brutes (xsave/xrstor) sont dans arch/x86_64/cpu/fpu.rs.
//
// NE PAS duplicquer les instructions ASM ici.

static XSAVE_AREA_SIZE: AtomicUsize = AtomicUsize::new(512);  // FXSAVE par défaut

pub fn detect_xsave_size() {
    let size = if cpu_has_avx512() {
        2112   // AVX-512 state
    } else if cpu_has_avx() {
        832    // AVX state
    } else if cpu_has_xsave() {
        576    // SSE + basic XSAVE
    } else {
        512    // FXSAVE fallback
    };
    XSAVE_AREA_SIZE.store(size, Ordering::Release);
}

/// Sauvegarder l'état FPU complet du thread courant
pub fn xsave(state: &mut FpuState) {
    let size = XSAVE_AREA_SIZE.load(Ordering::Relaxed);
    debug_assert!(state.buffer.len() >= size,
        "Buffer FPU trop petit: {} < {}", state.buffer.len(), size);

    unsafe {
        // Délégue à arch/ pour l'instruction ASM brute
        arch::cpu::fpu::xsave(state.buffer.as_mut_ptr(), !0u64);
    }
}

/// Restaurer l'état FPU d'un thread
pub fn xrstor(state: &FpuState) {
    unsafe {
        arch::cpu::fpu::xrstor(state.buffer.as_ptr(), !0u64);
    }
}
```

---

### 📌 scheduler/smp/load_balance.rs

**RÈGLE LB-01 : Lock ordering CPU ID croissant (anti-deadlock)**

```rust
// kernel/src/scheduler/smp/load_balance.rs

pub fn do_load_balance(this_cpu: CpuId) {
    let this_rq = percpu::run_queue(this_cpu);
    let this_load = this_rq.nr_running.load(Ordering::Relaxed);

    let busiest = find_busiest_cpu_in_domain(this_cpu);

    if let Some(busiest_cpu) = busiest {
        let busiest_load = percpu::run_queue(busiest_cpu)
            .nr_running.load(Ordering::Relaxed);

        // Seuil: déséquilibre de plus de 25%
        if busiest_load > this_load + (this_load / 4) {
            migrate_tasks(busiest_cpu, this_cpu, (busiest_load - this_load) / 2);
        }
    }
}

fn migrate_tasks(from_cpu: CpuId, to_cpu: CpuId, count: u32) {
    // RÈGLE : Acquérir les locks dans l'ordre CROISSANT des CPU IDs
    // Raison : éviter deadlock A→B / B→A si deux CPUs font le LB simultanément
    let (first_cpu, second_cpu) = if from_cpu < to_cpu {
        (from_cpu, to_cpu)
    } else {
        (to_cpu, from_cpu)
    };

    let _guard1 = percpu::run_queue(first_cpu).lock();
    let _guard2 = percpu::run_queue(second_cpu).lock();

    // Migrer les threads NORMAL uniquement
    // Raison : RT a des affinités strictes (cpumask fixe, latence garantie)
    let mut migrated = 0;
    while migrated < count {
        let task = match percpu::run_queue(from_cpu).normal_queue.pop_tail() {
            Some(t) => t,
            None => break,
        };

        if task.cpumask.contains(to_cpu) {
            percpu::run_queue(to_cpu).enqueue(task, task.policy);
            migrated += 1;
        } else {
            percpu::run_queue(from_cpu).normal_queue.push_tail(task);
            break;
        }
    }
}
```

---

### 📌 scheduler/energy/c_states.rs

**RÈGLE CSTATE-01 : fetch_min pour contrainte RT, recalcul à la sortie**

```rust
// kernel/src/scheduler/energy/c_states.rs

pub struct CpuIdleGovernor {
    max_c_state: AtomicU32,
    max_exit_latency_us: AtomicU32,
}

impl CpuIdleGovernor {
    /// Admission d'un thread RT → contraindre le C-state
    pub fn constrain_for_rt_thread(&self, rt_thread_latency_us: u32) {
        // fetch_min : la contrainte la plus stricte gagne
        self.max_exit_latency_us.fetch_min(rt_thread_latency_us, Ordering::AcqRel);

        let latency = self.max_exit_latency_us.load(Ordering::Acquire);
        let max_c_state = self.latency_to_c_state(latency);
        self.max_c_state.fetch_min(max_c_state, Ordering::AcqRel);

        ACPI_IDLE.constrain_c_state(self.max_c_state.load(Ordering::Acquire));
    }

    /// Fin d'un thread RT → recalculer depuis tous les RT restants
    pub fn relax_rt_constraint(&self) {
        let min_latency = percpu::rt_threads()
            .map(|t| t.rt_latency_us)
            .min()
            .unwrap_or(u32::MAX);  // Pas de RT → contrainte maximale relâchée

        self.max_exit_latency_us.store(min_latency, Ordering::Release);
        let max_c_state = self.latency_to_c_state(min_latency);
        self.max_c_state.store(max_c_state, Ordering::Release);
    }

    fn latency_to_c_state(&self, latency_us: u32) -> u32 {
        // Calibré pour x86_64 typique
        // C0: 0µs, C1: 1µs, C1E: 2µs, C2: 50µs, C3: 200µs, C6: 400µs
        match latency_us {
            0..=1    => 0,  // C0 uniquement
            2..=49   => 1,  // C1 max
            50..=199 => 2,  // C2 max
            200..=399 => 3, // C3 max
            _ => 6,         // Pas de contrainte → C6 autorisé
        }
    }
}
```

---

## ORDRE D'INITIALISATION SCHEDULER/

```
SÉQUENCE OBLIGATOIRE :

1. scheduler::core::preempt::init()         ← Init compteurs préemption per-CPU
2. scheduler::core::runqueue::init_percpu() ← Init run queues per-CPU
3. scheduler::fpu::save_restore::detect_xsave_size() ← Détecter taille XSAVE
4. scheduler::fpu::lazy::init()             ← Init lazy FPU (CR0.TS=1)
5. scheduler::timer::tick::init(HZ=1000)   ← Init ticker scheduler
6. scheduler::timer::hrtimer::init()        ← Init high-res timers
7. scheduler::sync::wait_queue::init()      ← (vérifie EmergencyPool initialisé)
8. scheduler::energy::c_states::init()      ← Init C-state governors
9. scheduler::smp::init_ap_queues()         ← Init queues pour APs (après SMP)
```

---

## TABLEAU DES RÈGLES SCHEDULER/ (référence rapide)

```
┌────────────────────────────────────────────────────────────────┐
│ RÈGLES ABSOLUES — scheduler/ (couche 1)                        │
├────────────────────────────────────────────────────────────────┤
│ SCHED-01 │ Dépend UNIQUEMENT de memory/ (jamais ipc/, fs/)     │
│ SCHED-02 │ signal/ ABSENT — dans process/ (DOC4)               │
│ SCHED-03 │ futex.rs ABSENT — dans memory/utils/futex_table.rs  │
│ SCHED-04 │ PreemptGuard RAII obligatoire (jamais direct)        │
│ SCHED-05 │ WaitNode depuis EmergencyPool uniquement             │
│ SCHED-06 │ switch_asm.s : sauvegarder r15 OBLIGATOIRE          │
│           │ (utilisé par ext4plus/inode/ops.rs — callee-saved)  │
│ SCHED-07 │ MXCSR + x87 FCW sauvegardés explicitement dans      │
│           │ switch_asm.s (indépendamment de XSAVE)              │
│ SCHED-08 │ CR3 switché DANS switch_asm AVANT restauration regs  │
│ SCHED-09 │ Lazy FPU : save AVANT switch_asm, mark APRÈS        │
│ SCHED-10 │ Lock ordering SMP : CPU ID croissant TOUJOURS        │
│ SCHED-11 │ RT admission → fetch_min(c_state_latency)           │
│ SCHED-12 │ Budget SCHED_DEADLINE corrigé par fréquence CPU     │
│ SCHED-13 │ pick_next_task() : zéro allocation, zéro sleep      │
│ SCHED-14 │ Migration : threads RT non migrés sans vérif         │
│           │ cpumask stricte                                      │
│ SCHED-15 │ signal_pending = AtomicBool dans TCB — scheduler    │
│           │ LIT seulement, process/signal/ ÉCRIT               │
│ SCHED-16 │ fpu/save_restore.rs délègue à arch/cpu/fpu.rs       │
│           │ pour les instructions ASM brutes (séparation claire) │
├────────────────────────────────────────────────────────────────┤
│ INTERDITS ABSOLUS                                              │
├────────────────────────────────────────────────────────────────┤
│ ✗  use crate::ipc dans scheduler/                              │
│ ✗  use crate::fs dans scheduler/                               │
│ ✗  use crate::process::signal dans scheduler/                  │
│ ✗  Appeler l'allocateur heap depuis pick_next_task()           │
│ ✗  preempt_disable() / enable() sans RAII                      │
│ ✗  Table futex dans scheduler/ (une seule dans memory/)        │
│ ✗  Oublier r15 dans le context switch ASM                      │
│ ✗  Switcher CR3 APRÈS restauration des registres               │
│ ✗  Dupliquer instructions XSAVE dans save_restore.rs           │
│    (déléguer à arch/cpu/fpu.rs uniquement)                     │
└────────────────────────────────────────────────────────────────┘
```

---

## 📋 CORRECTIONS APPORTÉES À DOC3

| # | Localisation | Erreur / Manque | Correction |
|---|---|---|---|
| 1 | `timer/tick.rs` | Commentaire `HZ configurable` vague | Précisé `HZ=1000` (cohérent avec DOC2 et séquence boot) |
| 2 | `fpu/` arborescence | `state.rs` absent | Ajouté — requis par `save_restore.rs` pour `FpuState` |
| 3 | `fpu/` commentaire | Séparation arch/scheduler non documentée | Ajouté commentaire explicatif dans `fpu/mod.rs` |
| 4 | `switch_asm.s` commentaires | Rôle de MXCSR/x87 FCW pas expliqué | Clarifié : sauvegardés explicitement, indépendamment de XSAVE |
| 5 | `wait_queue.rs` code | Chemin EmergencyPool raccourci | Chemin complet `memory::physical::frame::emergency_pool` |
| 6 | Tableau règles | SCHED-16 absent | Ajouté : `fpu/save_restore.rs` délègue instructions à `arch/` |
| 7 | Header position | `futex.rs ABSENT` non mentionné dans le header | Ajouté dans le bloc position |

---

*DOC 3 — Module Scheduler — Exo-OS — v5 corrigé*
*Prochains : DOC 4 (Process/Signal) · DOC 5 (IPC) · DOC 6 (FS) · DOC 7 (Security/Capability) · DOC 8 (DMA) · DOC 9 (Shield)*
