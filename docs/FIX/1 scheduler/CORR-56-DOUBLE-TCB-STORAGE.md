# CORR-56 — Double stockage TCB per-CPU : cohérence garantie

**Source :** Audit Qwen (P0-02)  
**Fichiers :** `kernel/src/scheduler/core/switch.rs`, `kernel/src/ipc/sync/sched_hooks.rs`  
**Priorité :** Phase 0

---

## Constat exact (code vérifié)

`switch.rs` maintient **deux** sources de vérité pour le TCB courant :

```rust
// Source 1 — tableau atomique cross-CPU (switch.rs:39)
pub static CURRENT_THREAD_PER_CPU: [AtomicUsize; MAX_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_CPUS];

// Source 2 — registre GS:[0x20] per-CPU natif (percpu.rs)
percpu::set_current_tcb(next as *mut ThreadControlBlock);
```

Dans `context_switch()`, les deux sont écrits séquentiellement :
```rust
// switch.rs:238
CURRENT_THREAD_PER_CPU[next.current_cpu().0 as usize]
    .store(next as *mut _ as usize, Ordering::Release);

// switch.rs:243
percpu::set_current_tcb(next as *mut ThreadControlBlock);
```

**Consommateur légitime du tableau :**
`ipc/sync/sched_hooks.rs:187` lit `CURRENT_THREAD_PER_CPU` pour observer le thread
courant d'un **CPU distant** (cross-CPU observation). GS ne peut pas servir cet usage
(GS lit uniquement le slot du CPU exécutant).

**Le bug réel :** entre les lignes 238 et 243, il y a une fenêtre où le tableau
dit "thread NEXT" mais GS dit encore "thread PREV". Sur SMP, si un CPU distant
lit `CURRENT_THREAD_PER_CPU[cpu]` pendant cette fenêtre, il obtient une valeur
cohérente. Mais si le CPU courant appelle `read_current_tcb()` depuis GS avant
la ligne 243 (impossible en pratique car la fenêtre est dans une section avec
préemption désactivée), il obtiendrait PREV.

**Risque concret :** la fenêtre est minuscule et sous préemption désactivée. Mais
l'ordering actuel (`Release` sur le tableau, pas de fence entre les deux écritures)
ne garantit pas qu'un CPU distant observant le tableau voie les données du TCB
`next` comme déjà consistantes (le store Release sur le tableau est suffisant pour
le pointeur lui-même, mais pas pour les champs de `next` si ceux-ci ont été écrits
sans Release préalable).

---

## Correction

### Option retenue : documenter + ajouter fence + aligner les orderings

La suppression du tableau est impossible (sched_hooks.rs en a besoin).
La correction est de rendre les deux écritures atomiquement cohérentes.

```rust
// switch.rs — dans context_switch(), après la mise à jour de l'état TCB

// Écriture GS en premier (local, immédiat)
// SAFETY: set_current_tcb écrit gs:[0x20] per-CPU.
percpu::set_current_tcb(next as *mut ThreadControlBlock);

// Fence SeqCst : garantit que toutes les écritures sur `next` (état, cpu, etc.)
// sont visibles AVANT que les CPUs distants lisent CURRENT_THREAD_PER_CPU.
core::sync::atomic::fence(Ordering::SeqCst);

// Ensuite le tableau cross-CPU avec Release (les CPUs distants voient next complet)
CURRENT_THREAD_PER_CPU[next.current_cpu().0 as usize]
    .store(next as *mut _ as usize, Ordering::Release);
```

### Ajouter un commentaire de contrat sur la variable publique

```rust
/// TCB courant par CPU — source de vérité pour l'**observation cross-CPU**.
///
/// # Invariant
/// Après chaque context_switch(), ce slot contient le pointeur vers le TCB
/// en train de s'exécuter sur le CPU correspondant.
///
/// # Usage
/// - Écriture : uniquement dans `context_switch()`, préemption désactivée.
/// - Lecture locale : préférer `percpu::read_current_tcb()` (plus rapide, pas de
///   déréférencement de tableau).
/// - Lecture distante (cross-CPU) : ce tableau uniquement.
///
/// # Ordering
/// Écriture avec `Release` après fence `SeqCst` — garantit que les champs du TCB
/// cible sont visibles au moment où un CPU distant observe ce slot.
pub static CURRENT_THREAD_PER_CPU: [AtomicUsize; MAX_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_CPUS];
```

### Lire en Acquire depuis sched_hooks.rs

```rust
// ipc/sync/sched_hooks.rs — lecture cross-CPU
// AVANT
let tcb_ptr = CURRENT_THREAD_PER_CPU[cpu_id.0 as usize].load(Ordering::Relaxed);

// APRÈS
let tcb_ptr = CURRENT_THREAD_PER_CPU[cpu_id.0 as usize].load(Ordering::Acquire);
// Acquire pair avec le Release dans context_switch → tous les champs de TCB visibles.
```

---

## Validation

- [ ] Vérifier que `ipc/sync/sched_hooks.rs` utilise `Ordering::Acquire`
- [ ] Vérifier que `context_switch()` écrit GS avant le tableau (fence entre les deux)
- [ ] Test SMP : observer CURRENT_THREAD_PER_CPU depuis un CPU distant pendant un context_switch
- [ ] Aucune régression sur les benchmarks scheduler (fence SeqCst = ~5 cycles overhead)
