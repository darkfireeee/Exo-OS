# CORR-72 — Constantes locales MAX_CPUS : import depuis source canonique

**Source :** Audit post-Fix1 (P1a, P1b) | Vérification à froid confirmée  
**Fichiers :** `kernel/src/fs/exofs/numa/numa_affinity.rs`, `kernel/src/memory/heap/thread_local/cache.rs`  
**Priorité :** Phase 2 (même pattern que CORR-55, même risque de désynchronisation)

---

## Constat exact

### numa_affinity.rs:13
```rust
// ACTUEL — constante locale non liée à la canonique
pub const MAX_CPUS: usize = 256;
```
Non importée depuis `scheduler::smp::topology::MAX_CPUS`. Si la valeur canonique change,
ce module reste à 256 silencieusement.

Impact concret :
- `CpuId::is_valid()` (ligne 36) : `(self.0 as usize) < MAX_CPUS` — rejetterait les CPUs
  >= 256 même si la canonique monte à 512.
- `cpu_nodes: [u8; MAX_CPUS]` (ligne 65) : tableau dimensionné localement.
- Test ligne 338 : `assert!(!CpuId(256).is_valid())` — ce test hardcode 256,
  il passerait en silence même si MAX_CPUS monte.

### heap/thread_local/cache.rs:20
```rust
// ACTUEL — constante locale non liée à la canonique
pub const MAX_CPUS: usize = 256;
```
Le tableau de caches per-CPU `caches: [PerCpuCache; MAX_CPUS]` est dimensionné sur
cette valeur locale. Si un CPU avec ID >= 256 est enregistré et tente d'accéder à
son cache, le `assert!(cpu_id < MAX_CPUS, ...)` (ligne 269) panique — correctement,
mais sans message indiquant la désynchronisation.

---

## Correction

### numa_affinity.rs

```rust
// AVANT
pub const MAX_CPUS: usize = 256;

// APRÈS — importer depuis la source canonique du scheduler
use crate::scheduler::smp::topology::MAX_CPUS as SCHED_MAX_CPUS;

/// Nombre maximum de CPUs supportés par le sous-système NUMA.
/// Doit rester identique à `scheduler::smp::topology::MAX_CPUS`.
pub const MAX_CPUS: usize = SCHED_MAX_CPUS;

/// Assertion compile-time : cohérence avec la constante canonique scheduler.
const _: () = assert!(
    MAX_CPUS == crate::scheduler::smp::topology::MAX_CPUS,
    "numa_affinity::MAX_CPUS doit correspondre à scheduler::topology::MAX_CPUS"
);
```

**Alternative si le circular import pose problème :** importer depuis
`memory::core::constants::MAX_CPUS` qui est la constante canonique mémoire (aussi 256).

```rust
// Alternative sans dépendance scheduler
use crate::memory::core::constants::MAX_CPUS as MEM_MAX_CPUS;
pub const MAX_CPUS: usize = MEM_MAX_CPUS;
const _: () = assert!(
    MAX_CPUS == crate::memory::core::constants::MAX_CPUS,
    "numa_affinity::MAX_CPUS doit correspondre à memory::core::constants::MAX_CPUS"
);
```

**Corriger aussi le test hardcodé :**
```rust
// AVANT
assert!(!CpuId(256).is_valid());
assert!(m.register_cpu(CpuId(256), NumaNodeId(0)).is_err());

// APRÈS — test dynamique par rapport à la constante
assert!(!CpuId(MAX_CPUS as u32).is_valid());
assert!(m.register_cpu(CpuId(MAX_CPUS as u32), NumaNodeId(0)).is_err());
```

### heap/thread_local/cache.rs

```rust
// AVANT
pub const MAX_CPUS: usize = 256;

// APRÈS — importer depuis la constante canonique mémoire
// (pas de dépendance scheduler depuis le sous-système mémoire)
pub use crate::memory::core::constants::MAX_CPUS;

/// Assertion compile-time
const _: () = assert!(
    MAX_CPUS == crate::memory::core::constants::MAX_CPUS,
    "heap cache MAX_CPUS doit correspondre à memory::core::constants::MAX_CPUS"
);
```

Supprimer aussi les commentaires hardcodés sur "256 entrées" (lignes 237, 238, 248) :
```rust
// AVANT
// Initialise les 256 caches statiquement.

// APRÈS
// Initialise les MAX_CPUS caches statiquement.
```

---

## Note sur la valeur canonique

La valeur **256** dans `memory::core::constants::MAX_CPUS` est la valeur canonique correcte
pour ExoOS Phase 3. Ce n'est pas 512. L'audit externe qui affirme "unifier à 512" est
dans l'erreur — la valeur correcte est 256 alignée sur le layout SSR (SSR_MAX_CORES_LAYOUT=256).

---

## Validation

- [ ] `cargo check --target x86_64-unknown-none` — pas d'erreur d'import circulaire
- [ ] Les assertions `const _: ()` compilent sans erreur
- [ ] Test numa : `CpuId(MAX_CPUS as u32).is_valid() == false`
- [ ] Test cache : accès CPU > MAX_CPUS → panic avec message clair (inchangé)
