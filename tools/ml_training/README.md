# ExoShield NGAV — Entraînement ML

Produit le **premier jeu de poids fonctionnel** pour le NGAV ML d'ExoShield
(MLP profond + Isolation Forest), exporté au format Q16.16 directement chargeable
par le serveur `exo_shield`.

## ⚠️ Données synthétiques — modèle à ré-entraîner

`train_ngav.py` génère des événements **synthétiques** bénins/malveillants calqués
**exactement** sur la distribution que le kernel fournit au runtime
(`behaviour_data_for_event` dans `servers/exo_shield/src/main.rs` : features creuses
par type d'événement, valeurs clampées à `[0,99]`).

Ce jeu produit un détecteur **fonctionnel mais non final**. Le modèle de production
DEVRA être ré-entraîné sur des **traces réelles Exo-OS** :

1. Instrumenter `profiler.rs` pour journaliser les `ProcessBehaviourData` réels
   sous QEMU (workloads bénins : shell, compilation, serveur ; vs malveillants
   simulés : scan de ports, escalade de privilèges, exfiltration…).
2. Exporter ces traces en CSV (32 features + label).
3. Remplacer `make_dataset()` par un chargeur de ces traces.
4. Ré-exécuter le script → nouveaux `trained_weights.rs`.

> « La promesse d'une fausse sécurité est pire que l'absence de sécurité. » Tant que
> le modèle tourne sur données synthétiques, il est documenté comme **premier jet**,
> pas comme détecteur validé.

## Architecture (doit matcher le kernel)

| Composant | Forme | Fichier kernel |
|-----------|-------|----------------|
| MLP | 32 → 128 → 64 → 4, LeakyReLU(0.01) + sigmoïde linéaire | `ml/mlp.rs` |
| Isolation Forest | 8 arbres × 63 nœuds, seuils u16 sur features brutes | `ml/iforest.rs` |
| Markov | ordre-2, appris **en ligne** (pas de pré-entraînement) | `ml/markov.rs` |
| Fusion | MLP 0.45 + IF 0.35 + Markov 0.20 | `ml/ensemble.rs` |

### Normalisation (FIX-F10)

Le kernel fournit des features **brutes** `[0,99]` mais le MLP fait son produit en
Q16.16 → sans normalisation, l'entrée est vue comme ≈0 et le MLP est **inerte**.
Le script exporte `FEATURE_MAX[32]` ; le kernel normalise via
`FeatureVector::normalise_minmax(&[0;32], &FEATURE_MAX)` AVANT le MLP, tout en
gardant les features brutes pour l'IF (seuils définis sur l'échelle brute).

### Chargement authentifié (FIX-F3)

`trained_weights.rs` embarque `TRAINED_MLP_VERSION` + `TRAINED_MLP_CHECKSUM`
(FNV-1a 64-bit). Le kernel (`mlp_load_trained`) recalcule le checksum et **refuse**
le chargement si mismatch ou régression de version → pas d'écrasement silencieux
des poids du modèle phare.

## Exécution

```bash
# Setup (une fois) — venv hors /mnt/c (pip cassé sur le mount Windows)
python3 -m venv ~/.venvs/exo_ml
~/.venvs/exo_ml/bin/pip install numpy

# Entraînement + export
~/.venvs/exo_ml/bin/python tools/ml_training/train_ngav.py
```

Sortie : `servers/exo_shield/src/ml/trained_weights.rs` (régénéré, `@generated`).

## Dépendances

`numpy` uniquement (MLP implémenté à la main : forward/backward, SGD momentum).
Aucune dépendance lourde — le script reste portable et auditable.
