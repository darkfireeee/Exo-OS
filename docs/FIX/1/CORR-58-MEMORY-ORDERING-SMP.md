# CORR-58 — Memory Ordering : chemins de migration SMP

**Source :** Audit Qwen (P1-04, corrigé en scope réduit après analyse)  
**Fichiers :** `kernel/src/scheduler/smp/migration.rs`, `kernel/src/scheduler/core/switch.rs`  
**Priorité :** Phase 1

---

## Constat après vérification

**Les `Ordering::Relaxed` sur les compteurs stats (nr_running, picks_total, load_avg)
sont ACCEPTABLES** — ce sont des métriques de monitoring non utilisées pour prendre
des décisions de scheduling critiques.

**Les `Ordering::Relaxed` problématiques** sont dans les chemins où un CPU publie
un état que d'autres CPUs doivent observer pour décider d'une migration :

### Cas 1 — vruntime en runqueue (runqueue.rs:244)
```rust
// runqueue.rs:244 — hot path pick_next
let vr = unsafe { tcb.as_ref() }.vruntime.load(Ordering::Relaxed);
```
Si un CPU A vient de mettre à jour `vruntime` (après time-slice) et qu'un CPU B
lit ce vruntime pour décider de migrer le thread, B peut voir une valeur stale.
Le résultat est une décision de migration sub-optimale (pas un crash), mais peut
conduire à des déséquilibres de charge prolongés.

### Cas 2 — PREEMPT_COUNT cross-CPU (preempt.rs:136)
```rust
// preempt.rs:136
PREEMPT_COUNT[cpu].0.load(Ordering::Relaxed)
```
Un CPU distant lisant la préemption d'un autre CPU (pour load-balancing) peut
lire une valeur obsolète. Risque : tenter une migration vers un CPU qui vient de
désactiver la préemption.

---

## Correction ciblée

### Switch : publication de vruntime après time-slice

```rust
// task.rs — après décrément du quantum / mise à jour vruntime
// AVANT
self.vruntime.fetch_add(delta, Ordering::Relaxed);

// APRÈS — Release : toute lecture Acquire sur un autre CPU voit la valeur à jour
self.vruntime.fetch_add(delta, Ordering::Release);
```

### Migration : lecture avec Acquire

```rust
// migration.rs — lors du calcul du score de migration
// AVANT
let vr = unsafe { candidate.as_ref() }.vruntime.load(Ordering::Relaxed);

// APRÈS
let vr = unsafe { candidate.as_ref() }.vruntime.load(Ordering::Acquire);
```

### PREEMPT_COUNT observation distante

```rust
// preempt.rs — fn is_preemption_disabled_on(cpu: usize)
// AVANT
PREEMPT_COUNT[cpu].0.load(Ordering::Relaxed)

// APRÈS — si lecture cross-CPU (cpu != current_cpu)
PREEMPT_COUNT[cpu].0.load(Ordering::Acquire)
```

Garder `Relaxed` pour la lecture du CPU courant sur lui-même (cas le plus fréquent,
seul CPU autorisé à écrire dans son slot).

### Règle générale documentée (à ajouter en en-tête de preempt.rs)

```rust
// RÈGLE ORDERING SMP ExoOS :
//
// Relaxed  → lecture/écriture LOCAL-ONLY (un seul CPU accède, OU stats non-critiques)
// Release  → publication d'état qu'un autre CPU lira (push runqueue, wakeup, vruntime update)
// Acquire  → lecture d'état publié par un autre CPU (pop runqueue, vruntime read cross-CPU)
// SeqCst   → ordre total requis (context_switch fence, IRQ mask global)
```

---

## Périmètre — NE PAS CHANGER

Les opérations suivantes restent en `Relaxed` (justification documentée) :
- `nr_running.fetch_add/sub` — compteur stat, jamais utilisé comme gate de décision
- `picks_total`, `picks_rt`, `picks_cfs` — métriques de monitoring
- `load_avg` — lissage exponentiel, tolérant aux valeurs légèrement obsolètes
- `min_vruntime.store` — borne approximative par design (CFS accepte une légère imprécision)

---

## Validation

- [ ] Test SMP : migration de threads en charge avec `perf stat` — vérifier équilibre de charge
- [ ] Aucune régression sur les benchmarks scheduler (overhead Release/Acquire ≈ 1-2 cycles)
- [ ] `cargo check` sans warnings nouveaux
