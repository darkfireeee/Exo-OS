# CORR-59 — assert_preempt_disabled : activer en release

**Source :** Audit Qwen (P2-06)  
**Fichier :** `kernel/src/scheduler/core/preempt.rs`  
**Priorité :** Phase 2

---

## Constat

```rust
// preempt.rs:233 — actuel
pub fn assert_preempt_disabled() {
    debug_assert!(
        PreemptGuard::is_preempted_disabled(),
        "Assertion préemption désactivée : ÉCHEC — appel hors section protégée"
    );
}
```

`debug_assert!` est éliminé par le compilateur en build release (`--release`).
Les violations de préemption dans le code de production passent silencieusement.

---

## Correction

```rust
// preempt.rs — remplacement
pub fn assert_preempt_disabled() {
    // CHOIX DÉLIBÉRÉ : assert! (pas debug_assert!) — ce check reste actif en release.
    //
    // Rationale : une violation de préemption dans le scheduler (context_switch,
    // runqueue manipulation, etc.) est UB non-récupérable sur SMP. Un panic contrôlé
    // est préférable à une corruption silencieuse.
    //
    // Overhead : un seul load AtomicU32 + comparaison = ~2 cycles. Acceptable sur
    // les chemins où cette assertion est appelée (tous hors hot-loop).
    assert!(
        PreemptGuard::is_preemption_disabled(),
        "assert_preempt_disabled ÉCHEC — appel depuis une section sans PreemptGuard/IrqGuard"
    );
}

pub fn assert_preempt_enabled() {
    assert!(
        !PreemptGuard::is_preemption_disabled(),
        "assert_preempt_enabled ÉCHEC — appel depuis une section avec préemption désactivée"
    );
}
```

**Note :** Vérifier que `PreemptGuard::is_preemption_disabled()` (ou `is_preempted_disabled()`)
est publique et utilisable ici. Si elle est `pub(super)`, la rendre `pub(crate)`.

---

## Validation

- [ ] Build release : les assertions sont présentes dans le binaire (`objdump` pour vérifier)
- [ ] Test intentionnel de violation → panic avec message clair
- [ ] Performance : mesurer overhead sur hot path scheduler (doit être < 1%)
