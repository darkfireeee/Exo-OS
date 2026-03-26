# Exo-OS — Modifications « zéro surprise » (20/03/2026)

## Contexte

Objectif: appliquer une stratégie **non invasive** en mode « production read »:
- ne rien casser dans les cibles existantes;
- ajouter une voie de validation headless stable;
- documenter les résultats de tests/validations de manière explicite.

---

## Modifications apportées

### 1) `Makefile` — ajout d’une cible headless sûre (sans remplacement)

Fichier modifié: `Makefile`

Changements:
- ajout de nouvelles variables de logs dédiés:
  - `QEMU_SAFE_SERIAL_LOG ?= /tmp/exoos-serial.log`
  - `QEMU_SAFE_INT_LOG    ?= /tmp/exoos-qemu-int.log`
  - `QEMU_SAFE_E9_LOG     ?= /tmp/exoos-e9.log`
- ajout de `QEMU_HEADLESS_SAFE_FLAGS` avec:
  - `-serial file:...` (au lieu de `stdio`)
  - `-display none`
  - `-debugcon file:...`
- ajout d’une nouvelle cible:
  - `qemu-headless-safe: iso`
- mise à jour de `.PHONY` pour inclure `qemu-headless-safe`
- mise à jour de `clean` pour nettoyer les nouveaux logs
- mise à jour de `help` pour afficher la nouvelle cible

### 2) Garanties « zéro surprise » respectées

- aucune suppression de cible existante;
- aucune modification du comportement de `qemu`, `qemu-release`, `qemu-nographic`;
- ajout uniquement (opt-in) via `make qemu-headless-safe`.

---

## Validations exécutées

### A. Boot headless sûr

Commande exécutée:
- `timeout 80s make qemu-headless-safe`

Résultat:
- statut `QEMU_HEADLESS_SAFE_RC=nonzero` (attendu car arrêt par `timeout`/signal 15)
- **trace E9 valide** capturée (`zero_surprise_e9_tail.txt`):
  - `XK12356789abcdefgZA23[CAL:PIT-DRV-FAIL][CAL:FB3G][TIME-INIT hz=3000000000]`
  - `456abcdP789!I`
  - `OK`

Conclusion:
- le boot atteint bien la signature finale attendue avant arrêt contrôlé.

### B. Vérification compilation

Commande exécutée:
- `cargo check -q` dans `kernel/`

Résultat:
- log vide (`zero_surprise_cargo_check.log`), donc pas d’erreur remontée.

### C. Tests ExoFS ciblés

Commande exécutée:
- `cargo test fs::exofs -- --list` dans `kernel/`

Résultat:
- échec reproduit: `duplicate lang item in crate core: sized`
- impact sur dépendances de test (`cfg-if`, `cpufeatures`, `bitflags`, `rand_core`, etc.)

Conclusion:
- blocage de pipeline test connu, non causé par la nouvelle cible Makefile.

---

## Impact risque (grave → mineur)

- `Makefile` (ajout cible opt-in): **mineur**
- workflow runtime headless: **mineur** (améliore la reproductibilité)
- roadmap/projet: **aucun impact grave détecté**
- tests ExoFS: **blocage préexistant** confirmé, à traiter séparément du changement tooling

---

## Utilisation recommandée

- validation headless stable:
  - `make qemu-headless-safe`
- logs associés:
  - serial: `/tmp/exoos-serial.log`
  - interruptions: `/tmp/exoos-qemu-int.log`
  - debug E9: `/tmp/exoos-e9.log`

---

## Artefacts de session (workspace)

- `zero_surprise_qemu_make.log`
- `zero_surprise_qemu_status.txt`
- `zero_surprise_e9_tail.txt`
- `zero_surprise_cargo_check.log`
- `zero_surprise_exofs_tests.log`

