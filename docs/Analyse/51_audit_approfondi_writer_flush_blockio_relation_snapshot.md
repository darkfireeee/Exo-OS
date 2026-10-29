# Rapport d'Audit Approfondi #51 — ExoFS Writer, Block I/O, Relation Index & Snapshot Cascade

**Date :** 29 Octobre 2026 ~13:58 GMT  
**Périmètre :** Noyau Exo-OS — Système de fichiers `kernel/src/fs/exofs/`  
**Statut :** Corrections appliquées & Tests validés

---

## 1. Contexte & Objectifs

Dans le cadre de la session d'audit systématique approfondi (IPC, FS, SYSCALL), l'analyse des rapports d'audit tiers (`Analyse (3)`, `Analyse_2`, `Analyse_z-ai`) sur le sous-système ExoFS a identifié 4 anomalies substantielles touchant la cohérence des écritures différées, la gestion d'erreurs I/O en lecture-modification-écriture, le cycle de vie des relations bidirectionnelles de blobs et la suppression en cascade des arbres de snapshots.

---

## 2. Anomalies Détectées & Corrections Appliquées

### 2.1. `kernel/src/fs/exofs/io/writer.rs` : Perte silencieuse des écritures différées en cas d'erreur I/O partielle

- **Analyse du problème :** Dans `BlobWriter::flush_to`, la liste interne `self.entries` était intégralement vidée via `self.entries.drain(..)` avant de tenter l'écriture séquencée de chaque entrée dans le store sous-jacent. Si l'écriture de l'entrée $i$ échouait (ex: erreur de bus, timeout, disque plein), les entrées restantes $i..N$ ainsi que l'entrée $i$ étaient irrémédiablement perdues de la mémoire du writer, violant la garantie de durabilité et d'idempotence des flushes.
- **Correction :** Conservation du buffer d'entrées avant flush. En cas d'erreur lors de l'écriture de l'entrée $i$, toutes les entrées non encore persistées ($i..N$) sont réinsérées en tête de `self.entries` et `self.total_bytes_pending` est fidèlement recalculé. L'erreur `Err(e)` est ensuite propagée proprement à l'appelant sans perte de données sales.

### 2.2. `kernel/src/fs/exofs/recovery/block_io.rs` : Remplacement destructeur par des zéros en cas d'erreur de lecture partielle

- **Analyse du problème :** Dans `write_bytes_at`, lors de la phase de cycle lecture-modification-écriture d'un bloc partiel non aligné, si la lecture préliminaire du bloc existant `device.read_block(current_lba, &mut block)` échouait, le code effectuait un `block.fill(0)` silencieux et poursuivait l'écriture `device.write_block(current_lba, &block)`. Cette défaillance écrasait les octets voisins valides sur le secteur avec des zéros.
- **Correction :** Propagation immédiate de l'erreur I/O `device.read_block(current_lba, &mut block)?;`. Aucune écriture n'est tentée si le bloc sous-jacent n'a pu être fidèlement relu.

### 2.3. `kernel/src/fs/exofs/relation/relation_index.rs` : Enregistrements fantômes asymétriques dans `remove_all_for_blob`

- **Analyse du problème :** Lors de la suppression de toutes les relations associées à un blob via `remove_all_for_blob`, la méthode vidait les listes indexées par `blob_id` dans `by_from` et `by_to`, mais omettait d'itérer sur chaque ID de relation pour purger l'entrée réciproque dans la map du blob pair (`by_to` du destinataire ou `by_from` de la source). Les requêtes de traversée de graphe continuaient donc à trouver des IDs de relations fantômes pointant vers des structures désallouées.
- **Correction :** Implémentation d'un nettoyage bidirectionnel symétrique systématique :
  - Pour chaque `rel_id` dans `by_from[blob_id]`, suppression du `rel_id` dans `by_to[to_blob]` de la cible.
  - Pour chaque `rel_id` dans `by_to[blob_id]`, suppression du `rel_id` dans `by_from[from_blob]` de la source.
  - Décrémentation stricte et cohérente de `n_total_ids`, `n_from_keys`, `n_to_keys` et incrémentation de `total_removes`.
  - Enrichissement du test unitaire `test_remove_all_for_blob` pour vérifier l'absence totale de résidus symétriques.

### 2.4. `kernel/src/fs/exofs/snapshot/snapshot_delete.rs` : Récidive récursive et omission des préconditions en cascade

- **Analyse du problème :** 
  1. `cascade_delete` utilisait une récursion non bornée pouvant atteindre la profondeur maximale des snapshots (1024), en violation formelle de la règle de sécurité noyau `RECUR-01` interdisant les récursions profondes sur la pile kernel (risque avéré de Kernel Stack Overflow).
  2. Les snapshots descendants n'étaient pas soumis à `check_preconditions(&child, opts)` avant suppression, permettant la suppression arbitraire d'un snapshot descendant actif, monté ou en cours de restauration.
- **Correction :**
  1. Remplacement de l'algorithme récursif par une traversée itérative sur pile de travail (`worklist: Vec<SnapshotId>`).
  2. Phase 1 préparatoire de validation : parcours de tous les descendants avec `Self::check_preconditions(&child, opts)?` avant d'initier la moindre suppression effective. Si un descendant viole les préconditions (ex: snapshot monté), l'opération avorte de façon transactionnelle sans état corrompu.

---

## 3. Réfutation des Faux Positifs Audités

1. **`DirectIoBuffer` non-support de B4096 (`direct_io.rs`) :**
   - *Allégation tiers :* Le rapport prétendait que `DirectIoBuffer` panique ou tronque lors de l'utilisation de blocs de taille 4096.
   - *Vérification :* Réfuté. Le constructeur `DirectIoBuffer::new` vérifie explicitement `if block_size != BlockSize::B512 { return Err(ExofsError::InvalidArgument); }`, garantissant un refus contractuel fail-closed conforme à la spécification des secteurs directs 512B.
2. **`IoBatch::execute` élimination des données lues (`io_batch.rs`) :**
   - *Allégation tiers :* Le rapport affirmait que `IoBatch::execute` exécutait les lectures sans capturer les données.
   - *Vérification :* Réfuté. La ligne d'exécution appelle explicitement `result.add_read_ok(i, data, bytes)`, stockant fidèlement chaque buffer de lecture dans `result.read_data`.
3. **Deadlock same-bucket requeue (`futex.rs`) :**
   - *Allégation tiers :* Le rapport supposait que le requeue intra-bucket ne déplaçait pas les waiters.
   - *Vérification :* Réfuté. `futex_table.rs:563` appelle `bucket.requeue_inplace(src_addr, dst_addr, max_requeue)` sous le spinlock du bucket, effectuant la migration d'adresse en place.

---

## 4. Bilan des Tests et Vérifications

- `cargo check --manifest-path kernel/Cargo.toml` : **0 erreur**
- `cargo test --manifest-path kernel/Cargo.toml relation_index` : **11 tests passés, 0 échec**
- `cargo test --manifest-path kernel/Cargo.toml snapshot_delete` : **5 tests passés, 0 échec**
- `make test-drivers` : **22 tests passés, 0 échec**
- `make test-userspace` : **5 tests passés, 0 échec**
