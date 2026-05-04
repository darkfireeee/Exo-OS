# CORR-ALPHA-01 — Scheduler : `debug_assert` inversé dans `block_current_thread()`

> **Auteur :** claude-alpha  
> **Date :** 2026-05-04  
> **Classe :** 🔴 GRV — Guaranteed Wrong Behavior  
> **Fichier :** `kernel/src/scheduler/core/switch.rs`  
> **Fonction :** `block_current_thread()`  
> **Sévérité :** Critique — l'assertion de sécurité valide exactement le cas d'erreur au lieu du cas correct

---

## 1. Description du bug

Dans `block_current_thread()`, le code contient :

```rust
pub unsafe fn block_current_thread() {
    use crate::scheduler::core::runqueue::run_queue;

    debug_assert!(
        crate::scheduler::core::preempt::PreemptGuard::depth() == 0,  // ← BUG
        "block_current_thread: appelé avec PreemptGuard actif"
    );
    // ...
}
```

### Analyse

`PreemptGuard::depth()` retourne la **profondeur d'imbrication des guards** :
- `depth() == 0` → **préemption ACTIVÉE** (aucun guard actif)
- `depth() > 0` → **préemption DÉSACTIVÉE** (un ou plusieurs guards actifs)

La fonction `block_current_thread()` bloque le thread courant. Pour bloquer un thread en sécurité sur un système SMP, **la préemption doit être désactivée** afin d'éviter :
1. Qu'un autre CPU migre le thread entre la décision de blocage et le switch effectif
2. Une race entre `run_queue(cpu_id)` (lecture du CPU courant) et une migration concurrente

La règle canonique de `schedule_block()` indique explicitement : *"Préemption désactivée requise (PreemptGuard ou IrqGuard)"*.

### Conséquence de l'assert actuel

L'assert valide le cas **ERRONÉ** : il passe si la préemption est activée (`depth() == 0`), et panique si elle est désactivée (`depth() > 0`). En d'autres termes :

| État préemption | Assert actuel | Comportement |
|----------------|---------------|--------------|
| Activée (depth=0) — DANGEREUX | ✅ Passe | Bug silencieux |
| Désactivée (depth>0) — CORRECT | ❌ Panique | Faux positif |

En mode `release` (sans `debug_assert`), l'appel sans préemption désactivée ne sera pas détecté, menant à une race condition SMP lors du context switch.

---

## 2. Correctif

### Fichier : `kernel/src/scheduler/core/switch.rs`

**Avant (ligne ~85) :**
```rust
pub unsafe fn block_current_thread() {
    use crate::scheduler::core::runqueue::run_queue;

    debug_assert!(
        crate::scheduler::core::preempt::PreemptGuard::depth() == 0,
        "block_current_thread: appelé avec PreemptGuard actif"
    );
```

**Après :**
```rust
pub unsafe fn block_current_thread() {
    use crate::scheduler::core::runqueue::run_queue;

    // SÉCURITÉ INVARIANT : block_current_thread() doit être appelé avec
    // la préemption désactivée (PreemptGuard ou IrqGuard actif).
    // depth() > 0 = préemption désactivée = état correct.
    // Sans cette garantie, un CPU concurrent peut migrer le thread entre
    // la lecture de cpu_id et l'accès à run_queue(), corrompant la queue.
    debug_assert!(
        crate::scheduler::core::preempt::PreemptGuard::depth() > 0,
        "block_current_thread: appelé SANS PreemptGuard — risque de race SMP"
    );
```

---

## 3. Validation

### Test unitaire recommandé

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::core::preempt::PreemptGuard;

    #[test]
    fn test_block_current_thread_requires_preempt_guard() {
        // Vérifier que depth() > 0 est correct en contexte protégé
        let _guard = PreemptGuard::new();
        assert!(PreemptGuard::depth() > 0, "Guard doit incrémenter depth");
        // block_current_thread() est unsafe — vérification de l'invariant uniquement
    }
}
```

### Cohérence avec la spec TLA+

Le module `ContextSwitch.tla` modélise le switch en étapes atomiques (`SwitchStage` 0→11). L'invariant `S26_TssRsp0MatchesCurrentTcb` exige que hors switch (`SwitchStage=0`), le TSS RSP0 soit stable. Ce niveau de cohérence n'est possible que si la préemption est maîtrisée pendant le blocage.

---

## 4. Impact scope

- **Fichier modifié :** `kernel/src/scheduler/core/switch.rs`
- **Lignes :** ~85-90 (remplacement de l'assert)
- **Aucun changement de comportement runtime :** correction d'un assert de débogage seulement
- **Tests SMP :** les tests multicore passant par `block_current_thread()` sans PreemptGuard seront désormais correctement détectés

---

*— claude-alpha*
