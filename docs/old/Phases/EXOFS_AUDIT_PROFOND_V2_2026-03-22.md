# ExoFS — Audit approfondi v2 (2026-03-22)

## Portée de ce second passage

Ce passage v2 a été réalisé **après** un premier audit déjà correctif, avec un objectif plus strict :

- réduire les cas de « succès partiel silencieux »,
- durcir les invariants structurels en lecture objet/range,
- supprimer les conversions tolérantes pouvant masquer une corruption,
- renforcer l’initialisation de clé de montage en contexte concurrent.

Fichiers ciblés en profondeur :

- `kernel/src/fs/exofs/storage/object_reader.rs`
- `kernel/src/fs/exofs/path/path_index.rs`
- `kernel/src/fs/exofs/syscall/readdir.rs`
- `kernel/src/fs/exofs/dedup/content_hash.rs`
- (lecture complémentaire sans patch) `kernel/src/fs/exofs/core/blob_id.rs`, `kernel/src/fs/exofs/objects/inline_data.rs`

## Résultats majeurs (v2)

### 1) Invariants structurels durcis dans `ObjectReader::read_object`

**Ajouts de garde-fous :**

- rejet si `content_size > MAX_OBJECT_SIZE`,
- rejet si `has_extent_map == true` et `blob_count == 0`,
- rejet si `has_extent_map == false` et `blob_count > 1`.

**Risque traité :** accepter des métadonnées incohérentes puis continuer sur un pipeline de lecture ambigu.

---

### 2) Assemblage multi-blob rendu strict (`read_and_assemble`)

**Durcissements :**

- refus explicite des refs vides si `expected_size > 0`,
- tri déterministe par `chunk_index`,
- détection des gaps/duplicates (`0..N-1` attendu),
- refus d’assemblage incomplet (`assembled.len() < expected_size`).

**Risque traité :** reconstruction partielle silencieuse et/ou ordre de chunk non canonique.

---

### 3) Lecture de plage (`ObjectRangeReader::read_range`) sécurisée

**Durcissements :**

- rejet `chunk_size == 0`,
- `range.length == 0` retourne immédiatement `Vec::new()`,
- rejet des refs vides,
- tri des refs par `chunk_index` avant extraction,
- vérification finale stricte : `result.len() == range.length`.

**Risque traité :** retour « OK » avec payload tronqué ou ambigu en cas de métadonnées incohérentes.

---

### 4) Initialisation de clé de montage rendue thread-safe (`path_index.rs`)

**Durcissements :**

- ajout d’un verrou d’initialisation atomique (`MOUNT_KEY_INIT_LOCK`),
- double-check `MOUNT_KEY_READY` avant/après acquisition,
- publication ordonnée (`Release/Acquire`) de la clé,
- suppression de conversion tolérante (`unwrap_or([0u8;8])`) au profit de `copy_from_slice` explicite.

**Risque traité :** course d’initialisation et clé de montage possiblement écrasée/non déterministe.

---

### 5) Nettoyage anti-fallback silencieux

Conversions fixes rendues explicites (sans valeurs de secours implicites) :

- `readdir.rs` : `ObjectId[..8] -> ino` via buffer `[u8;8]` + `copy_from_slice`,
- `content_hash.rs` : `shard_key()` via buffer `[u8;8]` + `copy_from_slice`.

**Risque traité :** fallback masquant une anomalie de format et dégradant la traçabilité.

## Points notables de robustesse de patch

- Une branche résiduelle incorrecte introduite pendant l’édition de `read_and_assemble` a été détectée immédiatement puis supprimée (contrôle post-patch).
- Les changements sont restés **ciblés** (pas de refonte API, pas de changement ABI syscall dans ce passage v2).

## Validation

- Diagnostics éditeur sur les fichiers modifiés : **aucune erreur**.
- Compilation WSL post-patch : `cargo check -q` dans `kernel/` **OK**.

## Conclusion

Ce second audit v2 renforce la sûreté ExoFS sur les chemins de lecture les plus sensibles :

- moins de tolérance implicite,
- invariants plus stricts,
- comportements déterministes,
- réduction claire des faux succès silencieux.

La base est maintenant plus robuste pour les futures passes (tests d’intégration ExoFS ciblés et scénarios de corruption volontaire).