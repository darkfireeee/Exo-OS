# CORR-55 — MAX_CPUS : uniformisation via import canonique

**Source :** Audit Qwen (P0-01, corrigé en sévérité P2 après vérification)  
**Fichiers :** `kernel/src/arch/x86_64/virt/stolen_time.rs`, `kernel/src/memory/physical/frame/reclaim.rs`  
**Priorité :** Phase 2

---

## Constat

Les deux modules définissent une constante locale `MAX_CPUS` au lieu d'importer
la valeur canonique depuis `scheduler::smp::topology`. Si la valeur canonique évolue,
ces deux modules restent désynchronisés silencieusement.

```
// stolen_time.rs:37
const MAX_CPUS: usize = 512;   ← LOCAL, valeur différente de la canonique (256)

// reclaim.rs:42
const MAX_CPUS: usize = 512;   ← LOCAL, valeur différente de la canonique (256)
```

La valeur `512` dans ces modules est intentionnelle (tableaux KVM et reclaim
sur-alloués), mais sans documentation de la divergence et sans lien avec la
constante canonique, c'est une bombe à retardement.

---

## Correction

### `kernel/src/arch/x86_64/virt/stolen_time.rs`

```rust
// AVANT
const MAX_CPUS: usize = 512;

// APRÈS — documenter explicitement la divergence
/// Taille de la table KVM stolen-time.
/// Volontairement supérieure à `scheduler::topology::MAX_CPUS` (256) pour
/// absorber des futures extensions sans recompilation KVM.
/// Doit rester ≥ `scheduler::topology::MAX_CPUS`.
const STOLEN_TIME_MAX_CPUS: usize = 512;
const _: () = assert!(
    STOLEN_TIME_MAX_CPUS >= crate::scheduler::smp::topology::MAX_CPUS,
    "STOLEN_TIME_MAX_CPUS doit être >= MAX_CPUS canonique"
);
```

Remplacer toutes les occurrences de `MAX_CPUS` dans ce fichier par `STOLEN_TIME_MAX_CPUS`.

### `kernel/src/memory/physical/frame/reclaim.rs`

```rust
// AVANT
const MAX_CPUS: usize = 512;

// APRÈS
/// Taille de la table RECLAIM_FLAGS.
/// Doit être ≥ MAX_CPUS canonique. Actuellement fixée à 512 pour marge.
const RECLAIM_MAX_CPUS: usize = 512;
const _: () = assert!(
    RECLAIM_MAX_CPUS >= crate::memory::core::constants::MAX_CPUS,
    "RECLAIM_MAX_CPUS doit être >= MAX_CPUS canonique"
);
```

Remplacer toutes les occurrences de `MAX_CPUS` dans ce fichier par `RECLAIM_MAX_CPUS`.

---

## Validation

- [ ] `cargo check --target x86_64-unknown-none` sans warnings sur les deux fichiers
- [ ] La constante-assertion compile sans erreur
- [ ] Si `scheduler::topology::MAX_CPUS` passe à 512, l'assertion passe toujours
- [ ] Si `scheduler::topology::MAX_CPUS` passe à 1024, l'assertion échoue avec message clair
