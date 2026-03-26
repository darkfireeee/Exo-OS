# ExoFS — Journal des corrections appliquées (2026-03-22)

## Fichiers modifiés

1. `kernel/src/fs/exofs/io/reader.rs`
   - Ajout import `core::blob_id::blake3_hash`.
   - Remplacement de l’algorithme local `inline_blake3` par un wrapper vers `blake3_hash` (source unique HASH-01).

2. `kernel/src/fs/exofs/storage/object_reader.rs`
   - `ObjectRangeReader::read_range`:
     - suppression du parsing ExtentMap permissif local,
     - utilisation de `ObjectReader::read_extent_map(...)`.
   - `verify_objects(...)`:
     - ajout d’une phase de classification header via `read_meta`,
     - séparation des erreurs `bad_header` vs `bad_content_hash`.

3. `kernel/src/fs/exofs/posix_bridge/fcntl_lock.rs`
   - Ajout de `total_lock_count(&self) -> usize` (somme réelle sur tous les slots).

4. `kernel/src/fs/exofs/posix_bridge/mod.rs`
   - `posix_bridge_stats().lock_count` utilise `FCNTL_LOCK_TABLE.total_lock_count()`.
   - `total_lock_count()` renvoie le total réel (plus d’approximation).

## Pourquoi ces corrections

- **Cohérence cryptographique** : un seul moteur de hash BlobId dans ExoFS.
- **Robustesse parsing** : éviter les valeurs par défaut silencieuses en lecture de métadonnées disque.
- **Observabilité fiable** : stats lock exactes, pas de placeholder.
- **Diagnostic intégrité précis** : distinguer corruption en-tête vs hash contenu.

## Vérifications effectuées

- Contrôle erreurs sur fichiers modifiés : OK.
- Compilation de vérification sous WSL:
  - `cargo check -q` dans `kernel/`.
  - Résultat : `EXOFS_CHECK_EXIT=0`.

## Impact

- Patch **non invasif** (pas de refonte architecture, pas de changement ABI syscall).
- Réduction du risque de faux diagnostics et de corruption silencieuse.
- Compatibilité de compilation conservée.