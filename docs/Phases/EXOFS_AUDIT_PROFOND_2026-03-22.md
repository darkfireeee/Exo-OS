# ExoFS — Audit profond ciblé (2026-03-22)

## Portée et méthode

- **Portée analysée en profondeur** : `io/*`, `storage/object_reader.rs`, `core/blob_id.rs`, `posix_bridge/*`.
- **Méthode** : lecture statique ciblée, recherche d’incohérences (`TODO/placeholder`, hash, parse tolérant), validation compilation sous WSL (`cargo check -q` dans `kernel/`).
- **Objectif** : identifier les incohérences fonctionnelles à risque élevé puis corriger uniquement les points sûrs et non destructifs.

## Résultats principaux

### 1) Incohérence critique HASH-01 dans la couche IO (corrigée)

**Constat**
- `kernel/src/fs/exofs/io/reader.rs` utilisait `inline_blake3()` avec un mélange maison (non BLAKE3 officiel).
- `kernel/src/fs/exofs/io/writer.rs` réutilise cette fonction via `use super::reader::inline_blake3;`.
- Ceci contredit la règle déclarée du projet : `BlobId = blake3(data brute)`.

**Risque**
- Vérification d’intégrité divergente entre sous-modules ExoFS.
- Faux positifs/negatifs checksum selon le chemin de lecture/écriture.

**Correction appliquée**
- `inline_blake3()` devient un **wrapper** vers `core::blob_id::blake3_hash()`.

---

### 2) Parse ExtentMap fragile en lecture partielle (corrigée)

**Constat**
- `kernel/src/fs/exofs/storage/object_reader.rs` (`ObjectRangeReader::read_range`) lisait l’ExtentMap avec un buffer fixe `65536`, puis parsing partiel permissif (`break`) et fallback `unwrap_or([0u8;...])`.

**Risque**
- Troncature silencieuse d’ExtentMap.
- Offsets/taille corrompus remplacés par `0` au lieu d’erreur explicite.
- Possibles lectures erronées non détectées.

**Correction appliquée**
- `read_range` délègue désormais à `ObjectReader::read_extent_map(...)`, qui:
  - valide strictement la taille attendue,
  - valide `count`,
  - échoue proprement en cas de format invalide.

---

### 3) Classification d’intégrité imprécise dans `verify_objects` (corrigée)

**Constat**
- `verify_objects(...)` classait `ChecksumMismatch` uniquement dans `bad_header`.
- Le champ `bad_content_hash` existait mais n’était pas alimenté.

**Risque**
- Rapport d’audit trompeur (mauvaise attribution des causes).
- Diagnostic opérationnel dégradé.

**Correction appliquée**
- Vérification en 2 phases:
  1. `read_meta` pour classer les erreurs d’en-tête,
  2. `read_object(..., Full)` pour classer les erreurs `content_hash` dans `bad_content_hash`.

---

### 4) Statistiques POSIX bridge inexactes (corrigées)

**Constat**
- `kernel/src/fs/exofs/posix_bridge/mod.rs` utilisait `lock_count_for(0)` comme placeholder pour un total global.

**Risque**
- Métriques observabilité fausses (sous/sur-estimation).
- Diagnostic de contention verrou biaisé.

**Correction appliquée**
- Ajout de `FCNTL_LOCK_TABLE.total_lock_count()` dans `fcntl_lock.rs`.
- `posix_bridge_stats().lock_count` et `total_lock_count()` utilisent désormais ce total réel.

## Incohérences relevées mais non modifiées (volontairement)

- Placeholders documentaires/tests non bloquants (ex: `tests/integration` placeholder).
- `core/blob_id.rs` contient des commentaires historiques autour de `merkle_combine`; non modifié ici pour éviter impact protocolaire/compatibilité sans campagne dédiée.

## Validation

- Vérification locale des fichiers modifiés : **aucune erreur** IDE.
- Validation WSL : `cargo check -q` dans `kernel/` → **succès** (`EXOFS_CHECK_EXIT=0`).

## Conclusion

Les incohérences à fort impact (hash, parsing, classification d’intégrité, métriques lock) ont été corrigées avec un patch minimal, sans refonte API globale et sans régression de compilation.