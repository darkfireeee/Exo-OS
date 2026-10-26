# Analyse Exhaustive Ligne par Ligne — `kernel/src/scheduler/`

## 1. Vue d'Ensemble de l'Ordonnanceur Multicœur

Le sous-système d'ordonnancement (`kernel/src/scheduler/`) est le cœur opérationnel d'Exo-OS. Il gère la répartition du temps processeur entre les threads noyau (kthreads) et processus utilisateurs (Ring 3), l'équilibrage de charge dynamique multicœur (SMP load balancing), la préemption fine, la gestion thermique et énergétique (C-states, P-states), ainsi que la conformité temps réel dur (SCHED_DEADLINE / Earliest Deadline First - EDF, SCHED_FIFO, SCHED_RR) et équitable (Completely Fair Scheduler - CFS).

Le sous-système se divise en 8 modules hautement spécialisés :
- `core/` : Primitives d'ordonnancement :
  - `task.rs` : Bloc de contrôle de thread (`ThreadControlBlock` - TCB), drapeaux d'état atomiques (`TaskState`), métriques d'exécution (`TaskStats`).
  - `runqueue.rs` : File d'exécution par CPU (`PerCpuRunQueue`) avec arbres équitables et listes chaînées prioritaires.
  - `pick_next.rs` : Algorithme de sélection de la prochaine tâche à exécuter (`pick_next_task`) en O(1) pour le temps réel.
  - `switch.rs` : Commutation de contexte bas niveau (`context_switch`), barrières anti-Spectre v2 (`IBPB`), shadow stack Intel CET, extension PKS et permutation des registres FPU/SIMD (`XSAVE`/`XRSTOR`).
  - `preempt.rs` : Compteur d'imbrication de préemption per-CPU (`PreemptGuard`, `IrqGuard`).
  - `boot_idle.rs` : Tâches d'inactivité (idle) d'amorce par cœur.
- `energy/` :
  - `c_states.rs` : Gestion des états de sommeil processeur (`C1`, `C2`, `C3`) et contraintes de latence imposées par les threads temps réel via `fetch_min`.
  - `frequency.rs` : Table des P-states et gouverneur de fréquence dynamique (scaling MHz).
  - `power_profile.rs` : Profils de consommation (`Performance`, `PowerSave`, `Balanced`).
- `fpu/` :
  - `state.rs`, `save_restore.rs`, `lazy.rs` : Sauvegarde et restauration non paresseuse (eager) des registres vectoriels SSE/AVX/AVX-512 au switch de contexte pour fiabiliser le code compilé Rust.
- `policies/` :
  - `cfs.rs` : Ordonnancement équitable basé sur le temps virtuel (`vruntime`) et pondération de nice.
  - `realtime.rs` : Politiques `SCHED_FIFO` et `SCHED_RR` avec quantum fixe.
  - `deadline.rs` : Ordonnancement à échéance EDF (`runtime`, `deadline`, `period`).
  - `idle.rs` : Politique d'inactivité exécutant l'instruction `HLT` sous contrôle des interruptions.
- `smp/` :
  - `topology.rs` : Détection de la topologie matérielle (packages, cœurs, threads SMT).
  - `affinity.rs` : Masques de bits d'affinité CPU (`cpu_affinity: AtomicU64`).
  - `load_balance.rs` : Équilibrage périodique et vol de travail (work stealing).
  - `migration.rs` : Files de migration inter-CPU sans allocation mémoire (`MigrationQueue`), pilotées par des interruptions inter-processeurs (IPI reschedule).
- `stats/` :
  - `latency.rs` : Histogrammes logarithmiques de latence de switch, de wakeup et d'IPI (`LatencyHist`).
  - `per_cpu.rs` : Statistiques de changements de contexte volontaires et involontaires par cœur (`CpuStats`).
- `sync/` : Primitives de synchronisation noyau :
  - `spinlock.rs` : Verrou tournant optimiste avec désactivation locale de préemption (`PreemptGuard`).
  - `mutex.rs` : Verrou endormant avec héritage de priorité (Priority Inheritance) pour prévenir l'inversion de priorité.
  - `rwlock.rs` : Verrou multi-lecteurs / écrivain exclusif (`KRwLock`).
  - `seqlock.rs` : Verrou séquentiel pour lectures non bloquantes dans les handlers d'interruption (ISR-safe).
  - `condvar.rs` : Variables de condition (`CondVar`) avec numéro de séquence anti-perte de signal.
  - `wait_queue.rs` : File d'attente FIFO de threads endormis (`WaitQueue`).
- `timer/` :
  - `tick.rs` : Interruption d'horloge cadencée à 1000 Hz (`HZ = 1000`), réévaluation des quanta CFS/RR et surveillance Watchdog/IOMMU.
  - `hrtimer.rs` : Timers haute résolution à base d'arbre rouge-noir.
  - `deadline_timer.rs` : Détection stricte des dépassements d'échéances temps réel.

---

## 2. Analyse Ligne par Ligne et Invariants de Sécurité

### 2.1 Commutation de Contexte et Mitigations Spectre (`core/switch.rs`)
- **Barrière de Prédiction de Branchement Indirect (IBPB) :**
  - Dans `context_switch`, lors d'un basculement entre deux processus utilisateurs distincts (`prev.pid != next.pid && next.pid.0 != 0`), l'instruction d'écriture MSR `IA32_PRED_CMD` (valeur `PRED_CMD_IBPB = 1`) est émise si le CPU le supporte (`has_ibpb`). Cela élimine les attaques Spectre v2 inter-processus.
  - L'inspection des fonctionnalités CPU a été modernisée via l'idiome Rust standard `.is_some_and(...)`.
- **Shadow Stack Intel CET et PKS :**
  - Restauration du pointeur de pile d'ombre CET (`SSP`) et de la clé PKS du thread entrant sous vérification de prise en charge matérielle.

### 2.2 Bloc de Contrôle de Tâche (`core/task.rs`)
- **Vérification Statique d'Offset (ExoShield) :**
  - L'assertion à la compilation garantissant l'offset du sous-champ `_cold_reserve` à 144 octets a été nettoyée de son addition d'identité redondante (`+ 0`).
  - Simplification du constructeur `new` par retour direct de structure sans liaison intermédiaire superflue (`let_and_return`).

### 2.3 Énergie, Fréquence et SMP (`energy/`, `smp/`)
- **Contrainte C-State Non Régressive :**
  - Dans `c_states.rs`, utilisation d'inline const arrays `[const { AtomicU8::new(...) }; MAX_CPUS]` pour la table de seuils et pour le compteur de threads RT actifs.
  - Dans `frequency.rs`, remplacement de la boucle manuelle d'assignation de la table de P-states par la méthode optimisée `copy_from_slice`.
  - Dans `migration.rs`, sécurisation de l'initialisation des `MigrationQueue` par constantes inline sans déréférencement brut.

### 2.4 Synchronisation et Horloges (`sync/`, `timer/`)
- **Élimination des `collapsible_if` :**
  - Dans `rwlock.rs` et `spinlock.rs`, les conditions de boucle d'acquisition optimiste ont été fusionnées en expressions logiques concises.
- **Match Guards dans le Tick Handler (`timer/tick.rs`) :**
  - Remplacement des blocs conditionnels imbriqués dans le `match tcb.policy` par des gardes de filtrage direct `SchedPolicy::RoundRobin if rr_tick(...)` et `SchedPolicy::Deadline if deadline_tick(...)`.
  - Modernisation des vérifications de périodicité modulo avec `.is_multiple_of(...)`.

---

## 3. Synthèse des Corrections Appliquées

| Fichier | Problème Détecté | Correction Appliquée |
| :--- | :--- | :--- |
| `core/pick_next.rs` | En-têtes doc `# Safety` manquants | Ajout des sections `# Safety` sur `pick_next_task` et `account_time` |
| `core/preempt.rs` | `declare_interior_mutable_const` sur `PREEMPT_COUNT` | Initialisation inline const `[const { ... }; MAX_CPUS]` |
| `core/preempt.rs` | Absence de `Default` sur `PreemptGuard` et `IrqGuard` | `impl Default for PreemptGuard` & `impl Default for IrqGuard` |
| `core/switch.rs` | Usage non idiomatique de `map_or(false, ...)` | Remplacement par `features.is_some_and(...)` sur 3 vérifications |
| `core/switch.rs` | En-têtes doc `# Safety` manquants | Ajout des sections `# Safety` sur 4 fonctions de switch |
| `core/task.rs` | Opération d'identité `+ 0` dans macro assert | Suppression de `+ 0` sur `offset_of!` |
| `core/task.rs` | `let_and_return` sur `Self` | Retour direct de la structure sans variable intermédiaire |
| `core/task.rs` | Absence de `Default` sur `TaskStats` | `impl Default for TaskStats` |
| `energy/c_states.rs` | Constantes intérieures mutables sur `CSTATE_MAX` / `RT_ACTIVE_COUNT` | Remplacement par des constantes inline `[const { ... }; MAX_CPUS]` |
| `energy/frequency.rs` | Boucle manuelle de copie d'éléments | `copy_from_slice` & constante inline sur `CURRENT_PSTATE` |
| `smp/migration.rs` | Constantes intérieures mutables | Initialisation inline const sur `MigrationQueue` et `MIGRATION_QUEUES` |
| `smp/migration.rs` | En-tête doc `# Safety` manquant | Ajout de `# Safety` sur `drain_pending_migrations` |
| `stats/latency.rs` | `manual_checked_ops` sur `avg_ns` & constante intérieure mutable | `checked_div(t).unwrap_or(0)` & constante inline |
| `stats/per_cpu.rs` | Constante intérieure mutable sur `INIT_STATS` | Constante inline `[const { CpuStats::new() }; MAX_CPUS]` & `impl Default` |
| `sync/condvar.rs` | Absence de `Default` sur `CondVar` | `impl Default for CondVar` |
| `sync/rwlock.rs` | `collapsible_if` & absence de `Default` | Fusion des `if` & `impl Default for KRwLock<T>` |
| `sync/seqlock.rs` | Indentation excessive de liste doc Markdown | Réduction de l'indentation & `impl Default for SeqLock<T>` |
| `sync/spinlock.rs` | `collapsible_if` & absence de `Default` | Fusion des `if` & `impl Default for SpinLock<T>` |
| `sync/wait_queue.rs` | Absence de `Default` & doc `# Safety` manquante | `impl Default for WaitQueue` & documentation `# Safety` |
| `timer/tick.rs` | Constantes intérieures mutables & `collapsible_match` | Inline consts, match guards, `.is_multiple_of()` et nettoyage doc |

---

## 4. Statut de Validation

Le sous-système `kernel/src/scheduler/` est intégralement analysé, optimisé pour les architectures multicœurs modernes, et rigoureusement documenté.
