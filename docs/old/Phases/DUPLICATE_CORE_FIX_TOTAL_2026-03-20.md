# Exo-OS — Correctif complet `duplicate core` + pipeline tests ExoFS (2026-03-20)

## Objectif

Éliminer durablement l’erreur `duplicate lang item in crate core: sized` sur les tests ExoFS,
sans casser le build/runtime noyau, et documenter une chaîne de test sûre.

---

## Diagnostic racine (global)

Le blocage provenait d’un **empilement de 3 causes** :

1. `build-std` activé globalement dans `.cargo/config.toml`
   - provoquait des graphes de build hétérogènes entre build bare-metal et tests.

2. stratégie de panic test non alignée
   - sans `panic-abort-tests`, le pipeline pouvait mixer des artefacts incompatibles (source du `duplicate core`).

3. dépendance `proptest` en no_std
   - `num-traits::Float` indisponible sans `libm` (erreurs E0432/E0599).

4. tests compilés vers la cible bare-metal
   - apparition de `can't find crate for test` (E0463) car `libtest` n’existe pas pour `x86_64-unknown-none`.

---

## Modifications appliquées

### 1) Stabilisation tests no_std (`panic-abort-tests`)

- Fichier: `kernel/.cargo/config.toml`
- Ajout:
  - `[unstable]`
  - `panic-abort-tests = true`

Effet: suppression de la classe d’erreur `duplicate core` observée auparavant.

### 2) Correction `proptest` / `num-traits`

- Fichier: `Cargo.toml` (workspace)
  - ajout `num-traits = { version = "0.2", default-features = false, features = ["libm"] }`
- Fichier: `kernel/Cargo.toml`
  - ajout `num-traits = { workspace = true }` dans `[dev-dependencies]`

Effet: les erreurs `num_traits::float::Float`/`floor`/`ceil`/`mul_add` ont été levées.

### 3) Séparation propre build bare-metal vs tests

- Fichier: `.cargo/config.toml`
  - retrait de `build-std` global (pour éviter de polluer les tests).
  - commentaire explicite: `build-std` est désormais appliqué via Makefile sur les builds noyau uniquement.

### 4) Makefile durci

- Fichier: `Makefile`
- Ajouts:
  - `CARGO_BAREMETAL_FLAGS = -Z build-std=core,alloc,compiler_builtins -Z build-std-features=compiler-builtins-mem`
  - `HOST_TEST_TARGET ?= x86_64-unknown-linux-gnu`
  - `HOST_TEST_OVERRIDES = --target $(HOST_TEST_TARGET)`
- Modifs:
  - `build`, `release`, `check` utilisent `$(CARGO_BAREMETAL_FLAGS)`
  - `test`, `test-exofs` utilisent la cible host (`$(HOST_TEST_OVERRIDES)`) + `panic-abort-tests`

Effet: build kernel inchangé côté bare-metal, tests orientés vers une cible supportant `libtest`.

---

## Validation exécutée

### A) Preuve disparition de `duplicate core`

Log: `exofs_tests_after_num_traits.log`

Constat:
- `proptest` compile (ligne `Compiling proptest v1.10.0`),
- **aucune** occurrence `duplicate lang item in crate core`.

### B) Nouveau point atteint (normal)

Toujours dans `exofs_tests_after_num_traits.log`:
- on observe ensuite `E0463: can't find crate for test` sur cible bare-metal,
- ce qui confirme que la phase `duplicate core` est dépassée et qu’on est sur le problème attendu de cible de test.

### C) Vérification phases précédentes (runtime)

Artefact runtime: `zero_surprise_e9_tail.txt`

Signature observée:
- `XK12356789abcdefgZA23[CAL:PIT-DRV-FAIL][CAL:FB3G][TIME-INIT hz=3000000000]`
- `456abcdP789!I`
- `OK`

Interprétation:
- la séquence de boot reste valide,
- pas de cassure brutale des phases antérieures (jusqu’à la phase ExoFS du boot).

---

## État final

- ✅ `duplicate core` traité structurellement.
- ✅ blocage `proptest/num-traits` traité (`libm`).
- ✅ séparation build noyau / tests introduite pour éviter régression.
- ✅ runtime antérieur conservé (`OK` E9).

### Point restant à exécuter en routine CI

Lancer la cible officielle:
- `make test-exofs`

(elle utilise maintenant la cible host dédiée et le routage de flags adéquat).

---

## Risque / compatibilité

- Risque faible: modifications additives et orientées outillage.
- Pas de changement de logique fonctionnelle ExoFS en production.
- Build bare-metal conservé via flags explicites Makefile.

---

## Validation finale relancée (WSL) — mise à jour

### Correctif complémentaire appliqué pour les tests host

- Fichier: `kernel/src/lib.rs`
  - `#![no_std]` devient `#![cfg_attr(not(test), no_std)]`
  - ajout `#[cfg(test)] extern crate std;`
  - `#[panic_handler]` et `#[alloc_error_handler]` limités à `#[cfg(not(test))]`

But: supprimer le conflit `panic_impl/alloc_error_handler` lors de la compilation des tests host.

### Résultat de la relance `make test-exofs`

- La relance compile bien le crate test host (`--target x86_64-unknown-linux-gnu`).
- Le blocage actuel n'est plus `duplicate core` mais un ensemble d'erreurs de modules de tests ExoFS (imports/visibilités/API test interne), par exemple:
  - imports manquants (`rel_kind`, `SNAPSHOT_MAGIC`, `EBADF/EINVAL/ERANGE`, etc.),
  - fonctions privées utilisées depuis des tests (`create_snapshot`),
  - symboles de test non exportés (`export_blob_pub`, `crc32c_compute`, etc.).

Conclusion: la classe d'erreur visée (`duplicate core`) est traitée, mais la passe ExoFS complète reste bloquée par la cohérence interne des modules de tests ExoFS.
