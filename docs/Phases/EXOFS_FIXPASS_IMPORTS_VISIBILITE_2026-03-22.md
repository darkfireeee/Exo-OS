# ExoFS — Fixpass imports/visibilités/API drift (2026-03-22)

## Objectif

Corriger les erreurs de compilation ciblées (imports manquants, visibilités, dérive API de tests) avec changements minimaux et traçables.

## Actions détaillées (granulaires)

### A. Imports manquants dans modules de tests

1. `kernel/src/fs/exofs/objects/extent_tree.rs`
   - Ajout imports test: `DiskOffset`, `EXTENT_FLAG_SPARSE`.

2. `kernel/src/fs/exofs/objects/object_cache.rs`
   - Ajout import test: `alloc::sync::Arc`.

3. `kernel/src/fs/exofs/storage/heap.rs`
   - Ajout import test: `HEAP_START_OFFSET`.

4. `kernel/src/fs/exofs/storage/object_reader.rs`
   - Remplacement import test inutile (`CompressionType`) par `BLOCK_SIZE`.

5. `kernel/src/fs/exofs/gc/epoch_scanner.rs`
   - Ajout imports test: `alloc::collections::BTreeMap`, `EpochRootEntry`.
   - Retrait implicite d’un import non nécessaire (`EpochFlags` non utilisé).

6. `kernel/src/fs/exofs/snapshot/snapshot_list.rs`
   - Ajout import test: `EpochId`.

7. `kernel/src/fs/exofs/snapshot/snapshot_delete.rs`
   - Ajout import test: module `snapshot::flags`.

8. `kernel/src/fs/exofs/syscall/object_read.rs`
   - Ajout imports test errno: `EBADF`, `EINVAL`, `ERANGE`.

9. `kernel/src/fs/exofs/syscall/object_write.rs`
   - Ajout imports test errno: `EBADF`, `ERANGE`.

10. `kernel/src/fs/exofs/syscall/relation_query.rs`
    - Ajout import test: `rel_kind`.

11. `kernel/src/fs/exofs/export/exoar_reader.rs`
    - Ajout import manquant: `crc32c_compute`.

### B. Visibilités/API drift

12. `kernel/src/fs/exofs/syscall/snapshot_create.rs`
    - Changement de visibilité: `create_snapshot` devient `pub(crate)`
      pour usage inter-modules syscall/tests, sans exposition publique externe.

13. `kernel/src/fs/exofs/syscall/export_object.rs`
    - Ajout wrapper de compatibilité `pub fn export_blob_pub(...)`
      délégant vers `export_blob(...)`.

14. `kernel/src/fs/exofs/syscall/snapshot_list.rs`
    - Suppression import test privé problématique (`create_snapshot` non requis ici).
    - Ajout import `SNAPSHOT_MAGIC` utilisé par les tests.

### C. Drift de noms de champs/tests

15. `kernel/src/fs/exofs/relation/relation_index.rs`
    - Import test corrigé: `RelationKind` ajouté.
    - Assertion test corrigée: `n_from_entries/n_to_entries` -> `n_from_keys/n_to_keys`.

### D. Nettoyage local anti-erreur

16. `kernel/src/fs/exofs/audit/audit_rotation.rs`
    - Retrait d’une ligne de test inutile (`AuditLog::new_const()`) causant erreur d’import/visibilité.

## Vérifications réalisées

1. Vérification diagnostics éditeur (`Problems`) sur les 16 fichiers modifiés:
   - Résultat: aucune erreur sur les fichiers touchés.

2. Validation terminal sous Windows:
   - `make test-exofs` indisponible (`make` absent en PowerShell natif).

3. Validation via WSL:
   - Relances de compilation `cargo check --tests`.
   - La compilation avance au-delà des erreurs ciblées de cette passe.
   - Échec restant actuel observé sur d’autres modules non traités ici (`kernel/src/fs/exofs/dedup/mod.rs`, macros `test/assert` hors scope de ce fixpass).

## Bilan

- Corrections ciblées imports/visibilités/API drift appliquées de manière minimale.
- Aucun changement de logique métier ExoFS runtime.
- Blocages de compilation restants identifiés, mais situés hors périmètre de cette passe.

## Clarification production vs tests (ajout post-revue)

Suite au retour utilisateur, une vérification explicite "production réelle" a été relancée :

1. Build kernel bare-metal (sans tests)
   - Commande: `cargo check -p exo-os-kernel -Z build-std=core,alloc,compiler_builtins -Z build-std-features=compiler-builtins-mem --target x86_64-unknown-none`
   - Résultat: `Finished dev profile` (pas d'erreur bloquante runtime).

2. Nettoyage runtime effectué
   - Fichier: `kernel/src/fs/exofs/export/exoar_reader.rs`
   - Action: retrait de l'import runtime inutile `crc32c_compute` et déplacement en scope `#[cfg(test)]`.
   - Effet: suppression d'un warning en build production.

Conclusion de clarification:
- La majorité des corrections précédentes étaient bien des corrections de compilation de tests.
- La voie production FS/runtime n'était pas cassée sur ce point précis au moment de la vérification.
- Un correctif runtime concret a néanmoins été appliqué (nettoyage import inutilisé) pour garder un build production propre.