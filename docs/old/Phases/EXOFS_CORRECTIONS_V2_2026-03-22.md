# ExoFS — Journal des corrections v2 (2026-03-22)

## Résumé exécutable

Passage v2 orienté durcissement :

- invariants de lecture objet renforcés,
- suppression de fallbacks silencieux ciblés,
- initialisation de clé de montage rendue sûre en concurrence,
- validation compilation WSL effectuée.

## Fichiers modifiés et deltas

### 1) `kernel/src/fs/exofs/storage/object_reader.rs`

- `read_object(...)`
  - validation `content_size <= MAX_OBJECT_SIZE`,
  - validation cohérence `has_extent_map/blob_count`.

- `read_and_assemble(...)`
  - rejet refs vides si taille attendue non nulle,
  - tri par `chunk_index`,
  - rejet gaps/duplicates d’index,
  - rejet reconstruction incomplète (`len < expected_size`).

- `ObjectRangeReader::read_range(...)`
  - rejet `chunk_size == 0`,
  - retour immédiat si `range.length == 0`,
  - tri refs déterministe,
  - rejet résultat partiel (`result.len() != range.length`).

### 2) `kernel/src/fs/exofs/path/path_index.rs`

- ajout `MOUNT_KEY_INIT_LOCK`.
- `ensure_mount_key_initialized()` sérialisée (CAS + spin-loop contrôlée).
- conversions `[u8] -> [u8;8]` converties en copies explicites (`copy_from_slice`).

### 3) `kernel/src/fs/exofs/syscall/readdir.rs`

- conversion `ObjectId[..8] -> u64` (`ino`) via copie explicite dans buffer `[u8;8]`.

### 4) `kernel/src/fs/exofs/dedup/content_hash.rs`

- `shard_key()` converti vers copie explicite `[u8;8]` (plus de fallback implicite).

## Correctif de qualité appliqué pendant la passe

- suppression d’un résidu logique accidentel dans `read_and_assemble` détecté en revue immédiate post-patch.

## Vérifications réalisées

- diagnostics éditeur des fichiers modifiés : **OK**.
- compilation WSL : `cargo check -q` dans `kernel/` : **OK**.

## Impact attendu

- baisse du risque de corruption silencieuse,
- lecture objet/range plus déterministe,
- meilleure observabilité des incohérences on-disk,
- initialisation de clé de montage plus robuste en SMP.
