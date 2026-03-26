# Audit approfondi du module `scheduler` (Exo-OS)

Date: 2026-03-22
Périmètre: `kernel/src/scheduler/**`
Objectif: base de refonte/correction ordonnancement noyau

---

## 1) Résumé exécutif

Le module `scheduler` est la couche 1.
Il orchestre l’exécution des threads noyau/processus.
Il s’appuie principalement sur `memory` et interfaces `arch`.
Il intègre politiques CFS/RT/Deadline/Idle.
Il porte aussi sync primitives noyau (spinlock, mutex, rwlock, wait queues).
Il est donc critique pour latence, sûreté, et stabilité globale.

Forces:
- découpage clair par sous-domaines (`core`, `timer`, `sync`, `fpu`, `smp`, `policies`)
- réexports propres dans `scheduler/mod.rs`
- attention aux invariants de preemption
- hot path identifié (`pick_next`, `runqueue`, `switch`)

Points de risque:
- forte sensibilité asm context switch
- dépendance stricte au layout TCB
- TODO SMP dans `core/switch.rs`
- interactions fines avec signaux/process et time arch

---

## 2) Positionnement et invariants

`scheduler` ne doit pas importer les couches hautes métier.
`scheduler` dépend de `memory` pour allocations et structures wait.
`scheduler` consomme des hooks `arch` pour FPU/context-switch/interruptions.
`scheduler` doit éviter allocations dans hot path.
`scheduler` doit respecter lock ordering global.
`scheduler` doit garantir fairness/perf des politiques.
`scheduler` doit conserver exactitude compteurs temps.
`scheduler` doit rester sûr en SMP.

Invariants structurants:
- init ordonnée des sous-modules.
- runqueue per-CPU valide avant activation scheduling actif.
- guard RAII (`PreemptGuard`, `IrqGuard`) corrects.
- switch asm cohérent avec état FPU.
- pick_next déterministe avec fallback idle.

---

## 3) Cartographie exhaustive des fichiers

### 3.1 Racine
- `kernel/src/scheduler/mod.rs`

### 3.2 Dossier `asm`
- `kernel/src/scheduler/asm/fast_path.s`
- `kernel/src/scheduler/asm/switch_asm.s`

### 3.3 Dossier `core`
- `kernel/src/scheduler/core/mod.rs`
- `kernel/src/scheduler/core/pick_next.rs`
- `kernel/src/scheduler/core/preempt.rs`
- `kernel/src/scheduler/core/runqueue.rs`
- `kernel/src/scheduler/core/switch.rs`
- `kernel/src/scheduler/core/task.rs`

### 3.4 Dossier `energy`
- `kernel/src/scheduler/energy/mod.rs`
- `kernel/src/scheduler/energy/c_states.rs`
- `kernel/src/scheduler/energy/frequency.rs`
- `kernel/src/scheduler/energy/power_profile.rs`

### 3.5 Dossier `fpu`
- `kernel/src/scheduler/fpu/mod.rs`
- `kernel/src/scheduler/fpu/lazy.rs`
- `kernel/src/scheduler/fpu/save_restore.rs`
- `kernel/src/scheduler/fpu/state.rs`

### 3.6 Dossier `policies`
- `kernel/src/scheduler/policies/mod.rs`
- `kernel/src/scheduler/policies/cfs.rs`
- `kernel/src/scheduler/policies/deadline.rs`
- `kernel/src/scheduler/policies/idle.rs`
- `kernel/src/scheduler/policies/realtime.rs`

### 3.7 Dossier `smp`
- `kernel/src/scheduler/smp/mod.rs`
- `kernel/src/scheduler/smp/affinity.rs`
- `kernel/src/scheduler/smp/load_balance.rs`
- `kernel/src/scheduler/smp/migration.rs`
- `kernel/src/scheduler/smp/topology.rs`

### 3.8 Dossier `stats`
- `kernel/src/scheduler/stats/mod.rs`
- `kernel/src/scheduler/stats/latency.rs`
- `kernel/src/scheduler/stats/per_cpu.rs`

### 3.9 Dossier `sync`
- `kernel/src/scheduler/sync/mod.rs`
- `kernel/src/scheduler/sync/barrier.rs`
- `kernel/src/scheduler/sync/condvar.rs`
- `kernel/src/scheduler/sync/mutex.rs`
- `kernel/src/scheduler/sync/rwlock.rs`
- `kernel/src/scheduler/sync/seqlock.rs`
- `kernel/src/scheduler/sync/spinlock.rs`
- `kernel/src/scheduler/sync/wait_queue.rs`

### 3.10 Dossier `timer`
- `kernel/src/scheduler/timer/mod.rs`
- `kernel/src/scheduler/timer/clock.rs`
- `kernel/src/scheduler/timer/deadline_timer.rs`
- `kernel/src/scheduler/timer/hrtimer.rs`
- `kernel/src/scheduler/timer/tick.rs`

---

## 4) APIs publiques structurantes

Réexports principaux:
- `ThreadControlBlock`, `ThreadId`, `ProcessId`, `CpuId`, `TaskState`, `SchedPolicy`
- `PreemptGuard`, `IrqGuard`
- `run_queue`, `init_percpu`
- `pick_next_task`
- `context_switch`, `schedule_yield`, `schedule_block`, `wake_enqueue`
- `block_current_thread`, `current_thread_raw`
- `monotonic_ns`, `scheduler_tick`

Init publique:
- `unsafe fn init(params: &SchedInitParams)`
- `unsafe fn init_ap(cpu_id: u32)`

Paramètres init:
- `SchedInitParams { nr_cpus, nr_nodes }`

---

## 5) État fonctionnel par sous-module

`core/task.rs`:
- définit types de base scheduling.
- porte état thread (priority/policy/state/flags).
- zone très sensible aux changements layout.

`core/runqueue.rs`:
- per-CPU queues.
- support multi-politiques.
- cœur de la contention SMP.

`core/pick_next.rs`:
- sélection thread suivant.
- hot path majeur.
- doit rester O(1) ou proche selon policy.

`core/switch.rs`:
- orchestration context switch.
- intègre hooks FPU.
- contient TODO SMP pour CPU id réel.

`sync/*`:
- primitives de synchro kernel.
- fondation wait queues/futex interactions indirectes.

`timer/*`:
- tick scheduler.
- horloges monotones.
- deadline/hrtimer.

`fpu/*`:
- lazy FPU state.
- XSAVE/XRSTOR gating.

`smp/*`:
- load-balance/migration/affinity.
- topologie CPU.

`policies/*`:
- CFS, realtime, deadline, idle.

---

## 6) Concurrence, locks et atomiques

Patterns observés:
- `SpinLock` maison dans `sync/spinlock.rs`.
- `IrqSpinLock` pour zones IRQ-sensitive.
- `SeqLock` pour lectures fréquentes/écritures rares.
- `Atomic*` abondants dans `core`, `timer`, `stats`.

Points d’attention:
- lock ordering inter-couches.
- risques de false-sharing sur compteurs per-CPU.
- coût de contention runqueue en SMP élevé.

---

## 7) TODO/stub/placeholder relevés

- `kernel/src/scheduler/core/switch.rs` ligne TODO SMP lecture vrai CPU ID.
- dépendance à implémentations arch pour certains chemins timing/FPU.

Impacts:
- robustesse SMP potentiellement incomplète sur topologies complexes.
- instrumentation CPU courante à renforcer.

---

## 8) Usage `&str`, API texte, traces

`&str` est présent surtout:
- noms/logiques diagnostics.
- libellés politiques/perf.

Règle refonte:
- éviter strings dans hot loops.
- conserver traces debug ciblées hors chemin critique.

---

## 9) Crates/imports majeurs

Imports majeurs:
- `core::sync::atomic::*`
- `core::ptr::NonNull`
- `crate::scheduler::...`
- `crate::arch::x86_64::...` (ponts ciblés)
- `crate::memory::...` (dépendances couche0)

Dépendances à surveiller:
- couplage implicite avec process via signaux/états thread.
- couplage timer avec arch time.

---

## 10) Journal de contrôle détaillé (SCHED-CHK)

- SCHED-CHK-001 vérifier couche1 dépend uniquement memory.
- SCHED-CHK-002 vérifier absence import process/ipc/fs/security direct.
- SCHED-CHK-003 vérifier ordre init de `scheduler::init`.
- SCHED-CHK-004 vérifier clamp `nr_cpus` sur `MAX_CPUS`.
- SCHED-CHK-005 vérifier `nr_nodes` min 1.
- SCHED-CHK-006 vérifier `preempt::init` avant runqueues.
- SCHED-CHK-007 vérifier runqueues initialisées pour tous CPUs.
- SCHED-CHK-008 vérifier FPU detect/init en phase boot.
- SCHED-CHK-009 vérifier lazy FPU activé BSP et AP.
- SCHED-CHK-010 vérifier tick init cadence attendue.
- SCHED-CHK-011 vérifier hrtimer init per-CPU.
- SCHED-CHK-012 vérifier deadline_timer init per-CPU.
- SCHED-CHK-013 vérifier wait_queue init après emergency pool.
- SCHED-CHK-014 vérifier c_states init après time.
- SCHED-CHK-015 vérifier topology_init finalisée.
- SCHED-CHK-016 vérifier TCB fields atomiques cohérents.
- SCHED-CHK-017 vérifier transitions TaskState sûres.
- SCHED-CHK-018 vérifier priority bounds stricts.
- SCHED-CHK-019 vérifier policy enum stable ABI interne.
- SCHED-CHK-020 vérifier runqueue invariants non rompus.
- SCHED-CHK-021 vérifier enqueue/dequeue symétrie.
- SCHED-CHK-022 vérifier idle thread toujours présent.
- SCHED-CHK-023 vérifier fallback idle quand queues vides.
- SCHED-CHK-024 vérifier fairness CFS sur vruntime.
- SCHED-CHK-025 vérifier starvation RT/CFS évitée.
- SCHED-CHK-026 vérifier budget deadline correctement décrémenté.
- SCHED-CHK-027 vérifier replenishment deadline.
- SCHED-CHK-028 vérifier migration cross-CPU safe.
- SCHED-CHK-029 vérifier affinity masks respectées.
- SCHED-CHK-030 vérifier load_balance période configurée.
- SCHED-CHK-031 vérifier balancing pas trop agressif.
- SCHED-CHK-032 vérifier `pick_next_task` path minimal.
- SCHED-CHK-033 vérifier `pick_next` invariants O(1) attendus.
- SCHED-CHK-034 vérifier `account_time` exactitude.
- SCHED-CHK-035 vérifier timestamps monotones.
- SCHED-CHK-036 vérifier clock source cohérente arch.
- SCHED-CHK-037 vérifier drift/time correction non bloquante.
- SCHED-CHK-038 vérifier `scheduler_tick` side effects maîtrisés.
- SCHED-CHK-039 vérifier preempt count never underflow.
- SCHED-CHK-040 vérifier guard RAII drop systématique.
- SCHED-CHK-041 vérifier IrqGuard restaure flags initiaux.
- SCHED-CHK-042 vérifier nested guards correct.
- SCHED-CHK-043 vérifier assert_preempt_* utile debug.
- SCHED-CHK-044 vérifier spinlock lock/unlock ordering.
- SCHED-CHK-045 vérifier irq spinlock restore order.
- SCHED-CHK-046 vérifier mutex blocking path wakeups.
- SCHED-CHK-047 vérifier rwlock reader/writer fairness.
- SCHED-CHK-048 vérifier condvar wake semantics.
- SCHED-CHK-049 vérifier barrier participant counts.
- SCHED-CHK-050 vérifier wait_queue FIFO/LIFO intention claire.
- SCHED-CHK-051 vérifier wait_queue timeout handling.
- SCHED-CHK-052 vérifier wait node lifecycle complet.
- SCHED-CHK-053 vérifier no allocation in hot switch path.
- SCHED-CHK-054 vérifier context switch asm contract.
- SCHED-CHK-055 vérifier save/restore regs complet.
- SCHED-CHK-056 vérifier MXCSR/FCW sauvegarde cohérente.
- SCHED-CHK-057 vérifier CR3 switch timing correct.
- SCHED-CHK-058 vérifier FPU save avant switch.
- SCHED-CHK-059 vérifier FPU loaded reset after switch.
- SCHED-CHK-060 vérifier #NM lazy handler path.
- SCHED-CHK-061 vérifier fpu state buffer alignement.
- SCHED-CHK-062 vérifier XSAVE size detection.
- SCHED-CHK-063 vérifier AVX/XSAVE feature gating.
- SCHED-CHK-064 vérifier fallback FXSAVE path.
- SCHED-CHK-065 vérifier AP init path reproduit BSP essentials.
- SCHED-CHK-066 vérifier cpu_id mapping stable.
- SCHED-CHK-067 vérifier TODO SMP identifié et priorisé.
- SCHED-CHK-068 vérifier per_cpu stats increments atomiques.
- SCHED-CHK-069 vérifier latency hist buckets bornés.
- SCHED-CHK-070 vérifier overflow counters accepté/documenté.
- SCHED-CHK-071 vérifier power profile transitions sûres.
- SCHED-CHK-072 vérifier frequency scaling hooks valides.
- SCHED-CHK-073 vérifier c-state max policy respectée.
- SCHED-CHK-074 vérifier idle policy réellement basse priorité.
- SCHED-CHK-075 vérifier realtime quantum RR stable.
- SCHED-CHK-076 vérifier FIFO non préempté par normal.
- SCHED-CHK-077 vérifier deadline admission control.
- SCHED-CHK-078 vérifier timer wheel hrtimer cohérent.
- SCHED-CHK-079 vérifier deadline_timer ordering strict.
- SCHED-CHK-080 vérifier clock read path lock-free.
- SCHED-CHK-081 vérifier cross-CPU clock skew géré.
- SCHED-CHK-082 vérifier SMP topology distances cohérentes.
- SCHED-CHK-083 vérifier NUMA node affinity usable.
- SCHED-CHK-084 vérifier migrate source/destination locks ordre.
- SCHED-CHK-085 vérifier IPI reschedule trigger correct.
- SCHED-CHK-086 vérifier race wake vs block traitée.
- SCHED-CHK-087 vérifier wake_enqueue idempotence.
- SCHED-CHK-088 vérifier block_current_thread transitions.
- SCHED-CHK-089 vérifier `current_thread_raw` sécurité usage.
- SCHED-CHK-090 vérifier ptr NonNull invariants.
- SCHED-CHK-091 vérifier static mut minimisés.
- SCHED-CHK-092 vérifier unsafe blocs documentés.
- SCHED-CHK-093 vérifier atomics ordering appropriés.
- SCHED-CHK-094 vérifier Relaxed uniquement compteurs.
- SCHED-CHK-095 vérifier Acquire/Release sur flags de synchro.
- SCHED-CHK-096 vérifier SeqCst usage exceptionnel.
- SCHED-CHK-097 vérifier deadlock lock ordering global.
- SCHED-CHK-098 vérifier scheduler locks avant memory locks.
- SCHED-CHK-099 vérifier IPC lock interactions documentées.
- SCHED-CHK-100 vérifier no_std compile constraints.
- SCHED-CHK-101 vérifier target bare-metal builds.
- SCHED-CHK-102 vérifier tests unitaires core présents.
- SCHED-CHK-103 vérifier tests runqueue invariants.
- SCHED-CHK-104 vérifier tests pick_next fairness.
- SCHED-CHK-105 vérifier tests preempt guards.
- SCHED-CHK-106 vérifier tests wait_queue timeout.
- SCHED-CHK-107 vérifier tests spinlock correctness.
- SCHED-CHK-108 vérifier tests mutex/rwlock.
- SCHED-CHK-109 vérifier tests condvar/barrier.
- SCHED-CHK-110 vérifier tests timer tick.
- SCHED-CHK-111 vérifier tests hrtimer ordering.
- SCHED-CHK-112 vérifier tests deadline scheduling.
- SCHED-CHK-113 vérifier tests fpu lazy path.
- SCHED-CHK-114 vérifier tests context switch smoke.
- SCHED-CHK-115 vérifier tests smp balance.
- SCHED-CHK-116 vérifier tests migration affinity.
- SCHED-CHK-117 vérifier tests energy profiles.
- SCHED-CHK-118 vérifier bench context switch latency.
- SCHED-CHK-119 vérifier bench wakeup latency.
- SCHED-CHK-120 vérifier bench timer jitter.
- SCHED-CHK-121 vérifier bench lock contention.
- SCHED-CHK-122 vérifier bench runqueue scalability.
- SCHED-CHK-123 vérifier branch prediction hot paths.
- SCHED-CHK-124 vérifier cacheline alignment TCB.
- SCHED-CHK-125 vérifier false-sharing counters.
- SCHED-CHK-126 vérifier function inlining appropriée.
- SCHED-CHK-127 vérifier panic paths minimales.
- SCHED-CHK-128 vérifier debug logs hors hot path.
- SCHED-CHK-129 vérifier names `&str` hors loops critiques.
- SCHED-CHK-130 vérifier documentation sync avec code.
- SCHED-CHK-131 vérifier docs policies à jour.
- SCHED-CHK-132 vérifier docs ASM à jour.
- SCHED-CHK-133 vérifier docs FPU à jour.
- SCHED-CHK-134 vérifier docs timer à jour.
- SCHED-CHK-135 vérifier docs SMP à jour.
- SCHED-CHK-136 vérifier docs stats à jour.
- SCHED-CHK-137 vérifier exports publics justifiés.
- SCHED-CHK-138 vérifier symbol visibility minimale.
- SCHED-CHK-139 vérifier erreurs propagées correctement.
- SCHED-CHK-140 vérifier values par défaut raisonnables.
- SCHED-CHK-141 vérifier HZ constant align use-cases.
- SCHED-CHK-142 vérifier monotonic_ns monotonicité stricte.
- SCHED-CHK-143 vérifier scheduler_tick non réentrant.
- SCHED-CHK-144 vérifier timer interrupt nesting control.
- SCHED-CHK-145 vérifier irq disable windows courtes.
- SCHED-CHK-146 vérifier irq restore même en erreur.
- SCHED-CHK-147 vérifier signal_pending lecture safe.
- SCHED-CHK-148 vérifier separation process/scheduler signals.
- SCHED-CHK-149 vérifier no direct delivery scheduler.
- SCHED-CHK-150 vérifier data race `NEED_RESCHED`.
- SCHED-CHK-151 vérifier handshake avec syscall return.
- SCHED-CHK-152 vérifier scheduler init unique.
- SCHED-CHK-153 vérifier ap init multi-fois safe.
- SCHED-CHK-154 vérifier runqueue init idempotence.
- SCHED-CHK-155 vérifier cpu offline handling.
- SCHED-CHK-156 vérifier thread migration during offline.
- SCHED-CHK-157 vérifier idle thread replacement safe.
- SCHED-CHK-158 vérifier pointer lifetime runqueue entries.
- SCHED-CHK-159 vérifier queue corruption detection.
- SCHED-CHK-160 vérifier crash diagnostics enrichis.
- SCHED-CHK-161 vérifier perf counters reset interface.
- SCHED-CHK-162 vérifier frequency scaling fallback.
- SCHED-CHK-163 vérifier no-op on unsupported arch feature.
- SCHED-CHK-164 vérifier wakeup storms behavior.
- SCHED-CHK-165 vérifier starvation detector éventuel.
- SCHED-CHK-166 vérifier throttling RT si nécessaire.
- SCHED-CHK-167 vérifier DL overload handling.
- SCHED-CHK-168 vérifier load balancing window tuning.
- SCHED-CHK-169 vérifier migration cost model.
- SCHED-CHK-170 vérifier affinity overrides.
- SCHED-CHK-171 vérifier cpu mask width assumptions.
- SCHED-CHK-172 vérifier >64 CPU roadmap.
- SCHED-CHK-173 vérifier compile warnings zéro.
- SCHED-CHK-174 vérifier clippy hot spots.
- SCHED-CHK-175 vérifier bloat symboles limités.
- SCHED-CHK-176 vérifier asm comments précis.
- SCHED-CHK-177 vérifier preservation ABI C.
- SCHED-CHK-178 vérifier extern signatures alignées.
- SCHED-CHK-179 vérifier stack alignment context switch.
- SCHED-CHK-180 vérifier unwind assumptions none.
- SCHED-CHK-181 vérifier panic=abort compatibility.
- SCHED-CHK-182 vérifier test harness host target.
- SCHED-CHK-183 vérifier bare-metal target testability.
- SCHED-CHK-184 vérifier instrumentation toggles.
- SCHED-CHK-185 vérifier tracing hooks optional.
- SCHED-CHK-186 vérifier lockdep-style checks possiblement.
- SCHED-CHK-187 vérifier preemption disable depth limits.
- SCHED-CHK-188 vérifier recursion scheduling interdite.
- SCHED-CHK-189 vérifier scheduler from interrupt safe.
- SCHED-CHK-190 vérifier nested interrupt behavior.
- SCHED-CHK-191 vérifier tickless roadmap (si applicable).
- SCHED-CHK-192 vérifier power-save idle transitions.
- SCHED-CHK-193 vérifier wake latency from C-states.
- SCHED-CHK-194 vérifier frequency change overhead.
- SCHED-CHK-195 vérifier thermal hooks roadmap.
- SCHED-CHK-196 vérifier runtime stats overhead.
- SCHED-CHK-197 vérifier lock contention stats.
- SCHED-CHK-198 vérifier per-cpu normalization.
- SCHED-CHK-199 vérifier timezone/realtime separation.
- SCHED-CHK-200 vérifier scheduler clock vs wall clock.
- SCHED-CHK-201 vérifier deadline units consistency.
- SCHED-CHK-202 vérifier nanosecond arithmetic overflow.
- SCHED-CHK-203 vérifier saturating ops où requis.
- SCHED-CHK-204 vérifier multiplication/division costs.
- SCHED-CHK-205 vérifier compile-time const tuning.
- SCHED-CHK-206 vérifier runtime tunables governance.
- SCHED-CHK-207 vérifier ownership module claire.
- SCHED-CHK-208 vérifier code review unsafe obligatoire.
- SCHED-CHK-209 vérifier roadmap TODO SMP.
- SCHED-CHK-210 vérifier backlog defects priorisés.
- SCHED-CHK-211 vérifier scenarios stress multi-core.
- SCHED-CHK-212 vérifier scenarios I/O heavy.
- SCHED-CHK-213 vérifier scenarios CPU bound.
- SCHED-CHK-214 vérifier scenarios mixed workloads.
- SCHED-CHK-215 vérifier fairness across policies.
- SCHED-CHK-216 vérifier idle accounting correctness.
- SCHED-CHK-217 vérifier context switch count accuracy.
- SCHED-CHK-218 vérifier lost wakeups absence.
- SCHED-CHK-219 vérifier spurious wake policy.
- SCHED-CHK-220 vérifier blocking syscalls integration.
- SCHED-CHK-221 vérifier IPC hooks compatibility.
- SCHED-CHK-222 vérifier futex wake interplay.
- SCHED-CHK-223 vérifier memory pressure callbacks.
- SCHED-CHK-224 vérifier OOM pathways with blocked threads.
- SCHED-CHK-225 vérifier stop-world events handling.
- SCHED-CHK-226 vérifier suspend/resume roadmap.
- SCHED-CHK-227 vérifier debug symbols for profiling.
- SCHED-CHK-228 vérifier deterministic behavior debug/release.
- SCHED-CHK-229 vérifier regression tests pipeline.
- SCHED-CHK-230 vérifier CI coverage scheduler.
- SCHED-CHK-231 vérifier bench thresholds tracked.
- SCHED-CHK-232 vérifier golden traces maintained.
- SCHED-CHK-233 vérifier doc index updated.
- SCHED-CHK-234 vérifier coupling map maintained.
- SCHED-CHK-235 vérifier external API stability.
- SCHED-CHK-236 vérifier deprecations documented.
- SCHED-CHK-237 vérifier migration plan refonte.
- SCHED-CHK-238 vérifier rollback strategy.
- SCHED-CHK-239 vérifier on-call runbook scheduler.
- SCHED-CHK-240 vérifier panic triage checklist.
- SCHED-CHK-241 vérifier metrics to alert mapping.
- SCHED-CHK-242 vérifier threshold alarms.
- SCHED-CHK-243 vérifier long-run drift tests.
- SCHED-CHK-244 vérifier jitter budget documented.
- SCHED-CHK-245 vérifier RT latency SLA.
- SCHED-CHK-246 vérifier DL miss counters.
- SCHED-CHK-247 vérifier CFS vruntime sanity checks.
- SCHED-CHK-248 vérifier migration churn limits.
- SCHED-CHK-249 vérifier stability under fault injection.
- SCHED-CHK-250 vérifier stable release gates.
- SCHED-CHK-251 vérifier ABI hooks arch validés.
- SCHED-CHK-252 vérifier symbol versioning interne.
- SCHED-CHK-253 vérifier performance budgets.
- SCHED-CHK-254 vérifier safe refactor entry points.
- SCHED-CHK-255 vérifier module readiness refonte.
- SCHED-CHK-256 vérifier debt list consolidée.
- SCHED-CHK-257 vérifier action plan priorisé.
- SCHED-CHK-258 vérifier propriétaires techniques nommés.
- SCHED-CHK-259 vérifier critères de sortie refonte.
- SCHED-CHK-260 vérifier clôture audit et suivi.

---

## 11) Conclusion

Le scheduler est techniquement structuré et mûr sur le socle principal.
La refonte doit cibler d’abord les invariants `core`/`switch`/`runqueue`.
Le TODO SMP dans `switch.rs` doit être traité tôt.
Les primitives sync et timer demandent une validation de charge soutenue.
Ce document sert de base de correction incrémentale et de non-régression.

## 12) Addendum de validation Scheduler (complément 500+)

- SCHED-ADD-001 valider politique de priorisation incidents.
- SCHED-ADD-002 valider protocole postmortem scheduling.
- SCHED-ADD-003 valider protocole mesure latence standard.
- SCHED-ADD-004 valider protocole mesure jitter standard.
- SCHED-ADD-005 valider protocole mesure throughput standard.
- SCHED-ADD-006 valider protocole mesure contention standard.
- SCHED-ADD-007 valider matrice workloads CPU-bound.
- SCHED-ADD-008 valider matrice workloads IO-bound.
- SCHED-ADD-009 valider matrice workloads mixed.
- SCHED-ADD-010 valider matrice workloads RT.
- SCHED-ADD-011 valider matrice workloads deadline.
- SCHED-ADD-012 valider matrice workloads idle-heavy.
- SCHED-ADD-013 valider matrice workloads NUMA.
- SCHED-ADD-014 valider matrice workloads SMT on/off.
- SCHED-ADD-015 valider matrice workloads single-core.
- SCHED-ADD-016 valider matrice workloads many-core.
- SCHED-ADD-017 valider budget context-switch par policy.
- SCHED-ADD-018 valider budget wakeup latency par policy.
- SCHED-ADD-019 valider budget timer latency par policy.
- SCHED-ADD-020 valider budget migration coût par policy.
- SCHED-ADD-021 valider budget overhead stats par policy.
- SCHED-ADD-022 valider budget overhead guards par policy.
- SCHED-ADD-023 valider budget overhead locks par policy.
- SCHED-ADD-024 valider budget overhead FPU par policy.
- SCHED-ADD-025 valider budget overhead balancing.
- SCHED-ADD-026 valider comportement en timer storms.
- SCHED-ADD-027 valider comportement en wake storms.
- SCHED-ADD-028 valider comportement en migration storms.
- SCHED-ADD-029 valider comportement en lock storms.
- SCHED-ADD-030 valider comportement en IRQ storms.
- SCHED-ADD-031 valider comportement en mémoire pression.
- SCHED-ADD-032 valider comportement en saturation runqueue.
- SCHED-ADD-033 valider comportement en starvation potentielle.
- SCHED-ADD-034 valider comportement en inversion priorité.
- SCHED-ADD-035 valider comportement en CPU hotplug.
- SCHED-ADD-036 valider comportement en CPU offline.
- SCHED-ADD-037 valider comportement en AP init tardif.
- SCHED-ADD-038 valider comportement en perte horloge.
- SCHED-ADD-039 valider comportement en reprise horloge.
- SCHED-ADD-040 valider comportement en pause VM.
- SCHED-ADD-041 valider comportement en reprise VM.
- SCHED-ADD-042 valider comportement en panic partielle.
- SCHED-ADD-043 valider comportement en panic globale.
- SCHED-ADD-044 valider comportement en debug build.
- SCHED-ADD-045 valider comportement en release build.
- SCHED-ADD-046 valider cohérence des indicateurs exportés.
- SCHED-ADD-047 valider cohérence des compteurs reset.
- SCHED-ADD-048 valider cohérence des compteurs overflow.
- SCHED-ADD-049 valider cohérence des compteurs per-CPU.
- SCHED-ADD-050 valider cohérence des compteurs globaux.
- SCHED-ADD-051 valider plan de correction TODO SMP.
- SCHED-ADD-052 valider plan de correction FPU lazy.
- SCHED-ADD-053 valider plan de correction balancing.
- SCHED-ADD-054 valider plan de correction lock contention.
- SCHED-ADD-055 valider plan de correction timer jitter.
- SCHED-ADD-056 valider plan de correction preempt depth.
- SCHED-ADD-057 valider plan de correction wake queue.
- SCHED-ADD-058 valider plan de correction migration churn.
- SCHED-ADD-059 valider plan de correction fairness drift.
- SCHED-ADD-060 valider clôture audit scheduler avec KPI.
