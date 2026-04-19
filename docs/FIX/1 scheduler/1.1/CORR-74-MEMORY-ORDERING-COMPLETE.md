# CORR-74 — Memory Ordering : complétion de la paire Release/Acquire

**Source :** Audit post-Fix1 (P3), confirmé en vérification à froid  
**Fichiers :** `kernel/src/scheduler/core/runqueue.rs`, `kernel/src/scheduler/smp/migration.rs`  
**Priorité :** Phase 1 — race condition SMP subtile mais réelle

---

## Contexte — ce qui a été fait dans CORR-58

CORR-58 (commit 0ac12411) a correctement ajouté `Ordering::Release` sur l'écriture
de vruntime dans `task.rs:573` :
```rust
self.vruntime.fetch_add(weighted, Ordering::Release);
```

Cette correction est incomplète sans la contrepartie `Acquire` sur les lectures.
En Rust (et x86 TSO), une paire Release/Acquire garantit que le CPU lecteur voit
**toutes les écritures effectuées avant le Release** au moment où il réussit
l'**Acquire**. Sans Acquire côté lecture, la garantie est nulle.

---

## Bug 1 — runqueue.rs : vruntime en Relaxed (3 occurrences)

### Localisation

```rust
// runqueue.rs:244 — dans insert_sorted() — lecture pour comparaison binaire
let vr = unsafe { tcb.as_ref() }.vruntime.load(Ordering::Relaxed);

// runqueue.rs:255 — dans insert_sorted() — lookup intermédiaire
self.tasks[mid].unwrap().as_ref().vruntime.load(Ordering::Relaxed)

// runqueue.rs:287 — dans peek_min() — min_vruntime update
self.tasks[0].unwrap().as_ref().vruntime.load(Ordering::Relaxed)
```

### Impact SMP

Scénario de race :
1. CPU A exécute un thread, met à jour vruntime avec Release → publié correctement
2. CPU B, qui fait du load-balancing, appelle `insert_sorted()` pour insérer ce thread
   dans sa runqueue — mais lit vruntime en Relaxed
3. CPU B peut voir l'**ancienne** valeur de vruntime (avant la timeslice de CPU A)
4. Résultat : le thread est inséré à la mauvaise position dans la runqueue CFS
   → déséquilibre de scheduling non-reproductible

**Fréquence :** rare en pratique (les Relaxed sur x86 TSO sont souvent équivalents
à Acquire en pratique hardware). Mais indéfini par le modèle mémoire Rust/LLVM, et
potentiellement reproductible sur ARM ou en présence d'optimisations agressives.

### Correction

```rust
// runqueue.rs — insert_sorted()

// AVANT (ligne 244)
let vr = unsafe { tcb.as_ref() }.vruntime.load(Ordering::Relaxed);

// APRÈS — Acquire : voit la valeur Release de task.rs:573
let vr = unsafe { tcb.as_ref() }.vruntime.load(Ordering::Acquire);
```

```rust
// AVANT (ligne 255)
self.tasks[mid].unwrap().as_ref().vruntime.load(Ordering::Relaxed)

// APRÈS
self.tasks[mid].unwrap().as_ref().vruntime.load(Ordering::Acquire)
```

```rust
// AVANT (ligne 287) — peek_min pour mise à jour min_vruntime
self.tasks[0].unwrap().as_ref().vruntime.load(Ordering::Relaxed)

// APRÈS
// NOTE : la mise à jour de min_vruntime est une approximation par design (CFS accepte
// une légère imprécision). Relaxed acceptable ICI UNIQUEMENT si min_vruntime est
// utilisé comme borne approximative, pas comme valeur de décision critique.
// Garder Relaxed pour cette ligne, documenter l'intentionnalité :
self.tasks[0].unwrap().as_ref().vruntime.load(Ordering::Relaxed)
// ↑ Intentionnel : min_vruntime est une borne approximative, pas une valeur stricte.
// La garantie de précision vient des lectures Acquire dans insert_sorted().
```

---

## Bug 2 — migration.rs : cpu_id.store en Relaxed

### Localisation

```rust
// migration.rs:143 — dans receive_migration() — après déqueue du TCB migré
tcb_mut.cpu_id.store(cpu.0 as u64, Ordering::Relaxed);
```

### Impact SMP

`cpu_id` dans le TCB indique sur quel CPU le thread **doit** s'exécuter après migration.
Ce champ est lu depuis d'autres contextes (pick_next, load_balance) pour décider
où réveiller le thread. Un store Relaxed signifie que les autres CPUs peuvent ne pas
voir la mise à jour immédiatement.

Scénario :
1. CPU A migre un thread vers CPU B, écrit `cpu_id = B` en Relaxed
2. CPU C (load-balancer) lit `cpu_id` en Relaxed et voit encore l'ancienne valeur A
3. CPU C tente une deuxième migration du même thread vers CPU D (déjà en cours de migration)
4. Double migration → comportement indéfini

### Correction

```rust
// migration.rs — dans receive_migration()

// AVANT
tcb_mut.cpu_id.store(cpu.0 as u64, Ordering::Relaxed);
MIGRATIONS_RECEIVED.fetch_add(1, Ordering::Relaxed);  // stat, OK Relaxed

// APRÈS — Release : publie le nouveau cpu_id avant que le thread
// soit visible dans la nouvelle runqueue
tcb_mut.cpu_id.store(cpu.0 as u64, Ordering::Release);
MIGRATIONS_RECEIVED.fetch_add(1, Ordering::Relaxed);  // stat inchangé, OK
```

### Lecture correspondante dans load_balance.rs

Vérifier que la lecture de `cpu_id` dans le load balancer utilise Acquire :
```rust
// Si migration.rs:120 ou load_balance.rs lit cpu_id :
let home_raw = tcb.as_ref().cpu_id.load(Ordering::Relaxed);
//                                                 ↑ si cette lecture est cross-CPU
//                                                   → changer en Acquire
```

Vérification : `migration.rs:120` :
```rust
let home_raw = tcb.as_ref().cpu_id.load(Ordering::Relaxed);
// ← Ce champ est lu pour savoir quelle runqueue désigner comme "home".
// Si ce CPU ≠ le CPU qui a fait le store, la lecture devrait être Acquire.
```

Correction :
```rust
// migration.rs:120 — lecture cross-CPU de cpu_id (home du thread)
let home_raw = tcb.as_ref().cpu_id.load(Ordering::Acquire);
```

---

## Résumé des changements

| Fichier | Ligne | Avant | Après | Raison |
|---------|-------|-------|-------|--------|
| runqueue.rs | 244 | Relaxed | Acquire | paire avec vruntime Release (task.rs:573) |
| runqueue.rs | 255 | Relaxed | Acquire | idem — chemin binaire insert_sorted |
| runqueue.rs | 287 | Relaxed | Relaxed | intentionnel — min_vruntime approximatif |
| migration.rs | 120 | Relaxed | Acquire | lecture cross-CPU de cpu_id.home |
| migration.rs | 143 | Relaxed | Release | publication cpu_id après migration |

---

## Stats / compteurs — inchangés

Les occurrences suivantes restent en Relaxed (compteurs de monitoring, aucune décision
de scheduling ne dépend de leur valeur exacte à l'instant T) :
- `MIGRATIONS_DROPPED.fetch_add(1, Relaxed)` — stat
- `MIGRATIONS_SENT.fetch_add(1, Relaxed)` — stat
- `MIGRATIONS_RECEIVED.fetch_add(1, Relaxed)` — stat

---

## Validation

- [ ] `cargo check` — pas de nouvelles erreurs
- [ ] Test SMP : 1000 migrations simultanées sur 8+ CPUs → aucune double-migration
- [ ] Benchmark scheduler : overhead < 2% vs avant (Acquire ≈ Release ≈ ~1 cycle sur x86)
- [ ] Test : thread migré sur CPU 64+ est schedulé correctement sur ce CPU
