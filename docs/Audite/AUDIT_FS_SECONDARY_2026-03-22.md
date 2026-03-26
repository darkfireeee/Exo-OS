# Audit complémentaire des fichiers secondaires `fs` (Exo-OS)

Date: 2026-03-22
Périmètre: `kernel/src/fs/**` (fichiers secondaires ExoFS)
Objectif: compléter l’audit principal en couvrant les intégrations fines (epoch/recovery/cache/crypto/observabilité) et les contrôles de non-régression

---

## 1) Cadrage secondaire

Ce document complète `AUDIT_FS_2026-03-22.md`.
Il cible les fichiers secondaires qui influencent directement la robustesse du pipeline ExoFS en dehors des handlers syscall centraux.

---

## 2) Inventaire secondaire ciblé (fs)

- `kernel/src/fs/exofs/audit/audit_entry.rs`
- `kernel/src/fs/exofs/audit/audit_export.rs`
- `kernel/src/fs/exofs/audit/audit_filter.rs`
- `kernel/src/fs/exofs/audit/audit_log.rs`
- `kernel/src/fs/exofs/audit/audit_reader.rs`
- `kernel/src/fs/exofs/audit/audit_rotation.rs`
- `kernel/src/fs/exofs/audit/audit_writer.rs`
- `kernel/src/fs/exofs/cache/blob_cache.rs`
- `kernel/src/fs/exofs/cache/cache_eviction.rs`
- `kernel/src/fs/exofs/cache/cache_policy.rs`
- `kernel/src/fs/exofs/cache/cache_pressure.rs`
- `kernel/src/fs/exofs/cache/cache_shrinker.rs`
- `kernel/src/fs/exofs/cache/cache_stats.rs`
- `kernel/src/fs/exofs/cache/cache_warming.rs`
- `kernel/src/fs/exofs/cache/extent_cache.rs`
- `kernel/src/fs/exofs/cache/metadata_cache.rs`
- `kernel/src/fs/exofs/cache/object_cache.rs`
- `kernel/src/fs/exofs/cache/path_cache.rs`
- `kernel/src/fs/exofs/compress/algorithm.rs`
- `kernel/src/fs/exofs/compress/compress_benchmark.rs`
- `kernel/src/fs/exofs/compress/compress_choice.rs`
- `kernel/src/fs/exofs/compress/compress_header.rs`
- `kernel/src/fs/exofs/compress/compress_stats.rs`
- `kernel/src/fs/exofs/compress/compress_threshold.rs`
- `kernel/src/fs/exofs/compress/compress_writer.rs`
- `kernel/src/fs/exofs/compress/decompress_reader.rs`
- `kernel/src/fs/exofs/compress/lz4_wrapper.rs`
- `kernel/src/fs/exofs/compress/zstd_wrapper.rs`
- `kernel/src/fs/exofs/crypto/crypto_audit.rs`
- `kernel/src/fs/exofs/crypto/crypto_shredding.rs`
- `kernel/src/fs/exofs/crypto/entropy.rs`
- `kernel/src/fs/exofs/crypto/key_derivation.rs`
- `kernel/src/fs/exofs/crypto/key_rotation.rs`
- `kernel/src/fs/exofs/crypto/key_storage.rs`
- `kernel/src/fs/exofs/crypto/master_key.rs`
- `kernel/src/fs/exofs/crypto/object_key.rs`
- `kernel/src/fs/exofs/crypto/secret_reader.rs`
- `kernel/src/fs/exofs/crypto/secret_writer.rs`
- `kernel/src/fs/exofs/crypto/volume_key.rs`
- `kernel/src/fs/exofs/crypto/xchacha20.rs`
- `kernel/src/fs/exofs/dedup/blob_registry.rs`
- `kernel/src/fs/exofs/dedup/blob_sharing.rs`
- `kernel/src/fs/exofs/dedup/chunker_cdc.rs`
- `kernel/src/fs/exofs/dedup/chunker_fixed.rs`
- `kernel/src/fs/exofs/dedup/chunking.rs`
- `kernel/src/fs/exofs/dedup/chunk_cache.rs`
- `kernel/src/fs/exofs/dedup/chunk_fingerprint.rs`
- `kernel/src/fs/exofs/dedup/chunk_index.rs`
- `kernel/src/fs/exofs/dedup/content_hash.rs`
- `kernel/src/fs/exofs/dedup/dedup_api.rs`
- `kernel/src/fs/exofs/dedup/dedup_policy.rs`
- `kernel/src/fs/exofs/dedup/dedup_stats.rs`
- `kernel/src/fs/exofs/dedup/similarity_detect.rs`
- `kernel/src/fs/exofs/epoch/epoch_barriers.rs`
- `kernel/src/fs/exofs/epoch/epoch_checksum.rs`
- `kernel/src/fs/exofs/epoch/epoch_commit.rs`
- `kernel/src/fs/exofs/epoch/epoch_commit_lock.rs`
- `kernel/src/fs/exofs/epoch/epoch_delta.rs`
- `kernel/src/fs/exofs/epoch/epoch_gc.rs`
- `kernel/src/fs/exofs/epoch/epoch_id.rs`
- `kernel/src/fs/exofs/epoch/epoch_pin.rs`
- `kernel/src/fs/exofs/epoch/epoch_record.rs`
- `kernel/src/fs/exofs/epoch/epoch_recovery.rs`
- `kernel/src/fs/exofs/epoch/epoch_root.rs`
- `kernel/src/fs/exofs/epoch/epoch_root_chain.rs`
- `kernel/src/fs/exofs/epoch/epoch_slots.rs`
- `kernel/src/fs/exofs/epoch/epoch_snapshot.rs`
- `kernel/src/fs/exofs/epoch/epoch_stats.rs`
- `kernel/src/fs/exofs/epoch/epoch_writeback.rs`
- `kernel/src/fs/exofs/export/exoar_format.rs`
- `kernel/src/fs/exofs/export/exoar_reader.rs`
- `kernel/src/fs/exofs/export/exoar_writer.rs`
- `kernel/src/fs/exofs/export/export_audit.rs`
- `kernel/src/fs/exofs/export/incremental_export.rs`
- `kernel/src/fs/exofs/export/metadata_export.rs`
- `kernel/src/fs/exofs/export/stream_export.rs`
- `kernel/src/fs/exofs/export/stream_import.rs`
- `kernel/src/fs/exofs/export/tar_compat.rs`
- `kernel/src/fs/exofs/recovery/boot_recovery.rs`
- `kernel/src/fs/exofs/recovery/checkpoint.rs`
- `kernel/src/fs/exofs/recovery/epoch_replay.rs`
- `kernel/src/fs/exofs/recovery/fsck.rs`
- `kernel/src/fs/exofs/recovery/fsck_phase1.rs`
- `kernel/src/fs/exofs/recovery/fsck_phase2.rs`
- `kernel/src/fs/exofs/recovery/fsck_phase3.rs`
- `kernel/src/fs/exofs/recovery/fsck_phase4.rs`
- `kernel/src/fs/exofs/recovery/fsck_repair.rs`
- `kernel/src/fs/exofs/recovery/recovery_audit.rs`
- `kernel/src/fs/exofs/recovery/recovery_log.rs`
- `kernel/src/fs/exofs/recovery/slot_recovery.rs`
- `kernel/src/fs/exofs/observability/alert.rs`
- `kernel/src/fs/exofs/observability/debug_interface.rs`
- `kernel/src/fs/exofs/observability/health_check.rs`
- `kernel/src/fs/exofs/observability/latency_histogram.rs`
- `kernel/src/fs/exofs/observability/metrics.rs`
- `kernel/src/fs/exofs/observability/perf_counters.rs`
- `kernel/src/fs/exofs/observability/space_tracker.rs`
- `kernel/src/fs/exofs/observability/throughput_tracker.rs`
- `kernel/src/fs/exofs/observability/tracing.rs`

---

## 3) Checklist d’intégration secondaire (FS-S2-INT)

- FS-S2-INT-001 valider `cache_shrinker.rs` avec `memory::shrinker`.
- FS-S2-INT-002 valider `cache_pressure.rs` avec events mémoire.
- FS-S2-INT-003 valider `cache_eviction.rs` sans I/O sous lock long.
- FS-S2-INT-004 valider `compress_writer.rs` et fallback algorithmique.
- FS-S2-INT-005 valider `decompress_reader.rs` en cas d’entrée corrompue.
- FS-S2-INT-006 valider `crypto/secret_writer.rs` et nonce policy.
- FS-S2-INT-007 valider `crypto/secret_reader.rs` et vérification intégrité.
- FS-S2-INT-008 valider `dedup/chunk_index.rs` avec collisions hash.
- FS-S2-INT-009 valider `dedup/dedup_policy.rs` avec charge mixte.
- FS-S2-INT-010 valider `epoch/epoch_commit_lock.rs` unicité lock global.
- FS-S2-INT-011 valider `epoch/epoch_barriers.rs` avec flush réel.
- FS-S2-INT-012 valider `epoch/epoch_writeback.rs` ordre durable.
- FS-S2-INT-013 valider `epoch/epoch_recovery.rs` placeholders tracés.
- FS-S2-INT-014 valider `export/stream_export.rs` avec snapshots actifs.
- FS-S2-INT-015 valider `export/stream_import.rs` et droits capability.
- FS-S2-INT-016 valider `recovery/boot_recovery.rs` avant exposition service.
- FS-S2-INT-017 valider `recovery/fsck_phase4.rs` politique stub explicite.
- FS-S2-INT-018 valider `recovery/fsck_repair.rs` et rollback.
- FS-S2-INT-019 valider `observability/metrics.rs` overhead maîtrisé.
- FS-S2-INT-020 valider `observability/alert.rs` seuils pertinents.
- FS-S2-INT-021 valider `audit/audit_log.rs` rotation sans perte critique.
- FS-S2-INT-022 valider `audit/audit_writer.rs` en conditions OOM.
- FS-S2-INT-023 valider `dedup/blob_registry.rs` cohérence refcount.
- FS-S2-INT-024 valider `dedup/similarity_detect.rs` faux positifs.
- FS-S2-INT-025 valider `epoch/epoch_checksum.rs` chaîne intègre.
- FS-S2-INT-026 valider `epoch/epoch_root_chain.rs` liens stables.
- FS-S2-INT-027 valider `crypto/key_rotation.rs` et fenêtres de transition.
- FS-S2-INT-028 valider `crypto/key_storage.rs` verrous + sécurité.
- FS-S2-INT-029 valider `compress/zstd_wrapper.rs` compat no_std.
- FS-S2-INT-030 valider `cache/path_cache.rs` invalidation correcte.
- FS-S2-INT-031 valider `cache/object_cache.rs` cohérence avec GC.
- FS-S2-INT-032 valider `epoch/epoch_gc.rs` avec `gc/*`.
- FS-S2-INT-033 valider `export/tar_compat.rs` tailles bornées.
- FS-S2-INT-034 valider `observability/tracing.rs` désactivation runtime.
- FS-S2-INT-035 valider `recovery/recovery_log.rs` persistance minimale.
- FS-S2-INT-036 valider `recovery/slot_recovery.rs` sur corruption partielle.
- FS-S2-INT-037 valider `audit/audit_filter.rs` sans coût excessif.
- FS-S2-INT-038 valider `audit/audit_export.rs` permissions d’export.
- FS-S2-INT-039 valider `dedup/chunker_cdc.rs` avec payload aléatoire.
- FS-S2-INT-040 valider `dedup/chunker_fixed.rs` avec petits objets.
- FS-S2-INT-041 valider `compress/compress_choice.rs` politique stable.
- FS-S2-INT-042 valider `compress/compress_stats.rs` précision métriques.
- FS-S2-INT-043 valider `crypto/entropy.rs` source robuste.
- FS-S2-INT-044 valider `crypto/master_key.rs` cycle de vie.
- FS-S2-INT-045 valider `crypto/object_key.rs` dérivation unique.
- FS-S2-INT-046 valider `epoch/epoch_pin.rs` et concurrence snapshots.
- FS-S2-INT-047 valider `epoch/epoch_slots.rs` rotation bornée.
- FS-S2-INT-048 valider `epoch/epoch_stats.rs` overhead.
- FS-S2-INT-049 valider `export/exoar_reader.rs` entrées malformées.
- FS-S2-INT-050 valider `export/exoar_writer.rs` cohérence métadonnées.
- FS-S2-INT-051 valider `recovery/checkpoint.rs` fréquence/coût.
- FS-S2-INT-052 valider `recovery/epoch_replay.rs` déterminisme.
- FS-S2-INT-053 valider `observability/health_check.rs` signaux utiles.
- FS-S2-INT-054 valider `observability/latency_histogram.rs` bins stables.
- FS-S2-INT-055 valider `observability/throughput_tracker.rs` fenêtres.
- FS-S2-INT-056 valider `observability/space_tracker.rs` exactitude.
- FS-S2-INT-057 valider `cache/cache_warming.rs` budget mémoire.
- FS-S2-INT-058 valider `cache/cache_policy.rs` anti-thrashing.
- FS-S2-INT-059 valider `audit/audit_rotation.rs` fréquence rotation.
- FS-S2-INT-060 valider compatibilité globale des secondaires avec syscalls 500..520.
- FS-S2-INT-061 valider `crypto/crypto_shredding.rs` effacement contrôlé.
- FS-S2-INT-062 valider `dedup/dedup_stats.rs` sans overflow.
- FS-S2-INT-063 valider `epoch/epoch_record.rs` format strict.
- FS-S2-INT-064 valider `export/metadata_export.rs` schéma stable.
- FS-S2-INT-065 valider `recovery/recovery_audit.rs` traçabilité incidents.
- FS-S2-INT-066 valider `observability/perf_counters.rs` coût acceptable.
- FS-S2-INT-067 valider `audit/audit_reader.rs` pagination sûre.
- FS-S2-INT-068 valider `compress/compress_threshold.rs` bornes.
- FS-S2-INT-069 valider `crypto/crypto_audit.rs` secrets non exposés.
- FS-S2-INT-070 valider compilation warnings secondary files FS.

---

## 4) Registre risques secondaires (FS-S2-RSK)

- FS-S2-RSK-001 risque de contention excessive sur commit lock epoch.
- FS-S2-RSK-002 risque de flush barrier stub laissé actif.
- FS-S2-RSK-003 risque de dérive pipeline compression/chiffrement.
- FS-S2-RSK-004 risque de nonce policy affaiblie.
- FS-S2-RSK-005 risque de collisions dedup mal gérées.
- FS-S2-RSK-006 risque de corruption partielle non réparée.
- FS-S2-RSK-007 risque de recovery trop permissif.
- FS-S2-RSK-008 risque d’export/import sans validation intégrité.
- FS-S2-RSK-009 risque d’audit logs non exploitables.
- FS-S2-RSK-010 risque de métriques bruitées sous charge.
- FS-S2-RSK-011 risque de cache thrashing.
- FS-S2-RSK-012 risque de fuite mémoire cache secondaire.
- FS-S2-RSK-013 risque de deadlock lock ordering FS/memory/scheduler.
- FS-S2-RSK-014 risque de no_std break via wrappers externes.
- FS-S2-RSK-015 risque de tests secondaires insuffisants.
- FS-S2-RSK-016 risque de placeholder epoch non résolu.
- FS-S2-RSK-017 risque de placeholder fsck phase4 non tracé.
- FS-S2-RSK-018 risque de secret leakage via observabilité.
- FS-S2-RSK-019 risque de latence p99 dégradée.
- FS-S2-RSK-020 risque de non-régression non couverte.
- FS-S2-RSK-021 risque de saturation export stream.
- FS-S2-RSK-022 risque de saturation import stream.
- FS-S2-RSK-023 risque de backlog GC relation/dedup.
- FS-S2-RSK-024 risque de checksum chain cassée.
- FS-S2-RSK-025 risque de migration clé incomplète.
- FS-S2-RSK-026 risque de rollback recovery ambigu.
- FS-S2-RSK-027 risque de droits capability incomplets.
- FS-S2-RSK-028 risque de trace haute volumétrie.
- FS-S2-RSK-029 risque de faux positif health-check.
- FS-S2-RSK-030 risque de dérive index path cache.
- FS-S2-RSK-031 risque de stats overflow silencieux.
- FS-S2-RSK-032 risque de comportement divergent debug/release.
- FS-S2-RSK-033 risque de fragmentation storage secondaire.
- FS-S2-RSK-034 risque de writeback starvation.
- FS-S2-RSK-035 risque de commit starvation.
- FS-S2-RSK-036 risque d’init/shutdown non symétrique.
- FS-S2-RSK-037 risque de crash loop recovery.
- FS-S2-RSK-038 risque de route erreur non audité.
- FS-S2-RSK-039 risque de dépendance transversale cachée.
- FS-S2-RSK-040 risque de dette secondaire non priorisée.
- FS-S2-RSK-041 risque de couplage fort cache/recovery.
- FS-S2-RSK-042 risque de couplage fort crypto/export.
- FS-S2-RSK-043 risque de couplage fort observabilité/latence.
- FS-S2-RSK-044 risque de couplage fort dedup/quota.
- FS-S2-RSK-045 risque de couplage fort epoch/gc.
- FS-S2-RSK-046 risque d’alignement ABI cassé dans exports.
- FS-S2-RSK-047 risque d’erreurs silencieuses dans wrappers compress.
- FS-S2-RSK-048 risque de contention audit writer.
- FS-S2-RSK-049 risque de lock inversé lors rotation audit.
- FS-S2-RSK-050 risque de clôture incomplète du lot secondaire FS.

---

## 5) Campagne validations secondaires (FS-S2-VAL)

- FS-S2-VAL-001 vérifier stress cache eviction/warming.
- FS-S2-VAL-002 vérifier stress dedup chunk index.
- FS-S2-VAL-003 vérifier stress compression/decompression.
- FS-S2-VAL-004 vérifier stress rotation clés.
- FS-S2-VAL-005 vérifier stress epoch commit/writeback.
- FS-S2-VAL-006 vérifier stress recovery replay/checkpoint.
- FS-S2-VAL-007 vérifier stress export/import stream.
- FS-S2-VAL-008 vérifier stress observabilité on/off.
- FS-S2-VAL-009 vérifier robustesse fsck phase1..4.
- FS-S2-VAL-010 vérifier robustesse placeholders explicitement signalés.
- FS-S2-VAL-011 vérifier robustesse audit log sur surcharge.
- FS-S2-VAL-012 vérifier robustesse cache shrinker sous pression mémoire.
- FS-S2-VAL-013 vérifier robustesse dedup collisions.
- FS-S2-VAL-014 vérifier robustesse checksum chain.
- FS-S2-VAL-015 vérifier robustesse rollback recovery.
- FS-S2-VAL-016 vérifier robustesse ACL sur export/import.
- FS-S2-VAL-017 vérifier robustesse no_std wrappers externes.
- FS-S2-VAL-018 vérifier robustesse latence p95/p99.
- FS-S2-VAL-019 vérifier robustesse métriques secondaires.
- FS-S2-VAL-020 vérifier robustesse interactions FS-memory.
- FS-S2-VAL-021 vérifier robustesse interactions FS-scheduler.
- FS-S2-VAL-022 vérifier robustesse interactions FS-security.
- FS-S2-VAL-023 vérifier robustesse interactions FS-ipc.
- FS-S2-VAL-024 vérifier robustesse interactions FS-process.
- FS-S2-VAL-025 vérifier stabilité sur 3 runs successifs.
- FS-S2-VAL-026 vérifier absence deadlock longue exécution.
- FS-S2-VAL-027 vérifier absence fuite mémoire longue exécution.
- FS-S2-VAL-028 vérifier cohérence index et docs secondaires.
- FS-S2-VAL-029 vérifier clôture risques secondaires critiques.
- FS-S2-VAL-030 vérifier préparation lot secondaire FS-L2.
- FS-S2-VAL-031 vérifier préparation lot secondaire FS-L3.
- FS-S2-VAL-032 vérifier critères sortie secondaires FS.
- FS-S2-VAL-033 vérifier stratégie rollback secondaire.
- FS-S2-VAL-034 vérifier traçabilité preuves de validation.
- FS-S2-VAL-035 vérifier conformité style et sections.
- FS-S2-VAL-036 vérifier checklists numérotées complètes.
- FS-S2-VAL-037 vérifier lisibilité pour revue future.
- FS-S2-VAL-038 vérifier stabilité des termes et taxonomie.
- FS-S2-VAL-039 vérifier maintien de la compat ABI.
- FS-S2-VAL-040 vérifier clôture opérationnelle de l’annexe.
- FS-S2-VAL-041 vérifier non-régression sur set_meta cap_token.
- FS-S2-VAL-042 vérifier non-régression `read_range` strict.
- FS-S2-VAL-043 vérifier non-régression `inline_blake3` wrapper.
- FS-S2-VAL-044 vérifier non-régression lock_count réel.
- FS-S2-VAL-045 vérifier non-régression zstd shims no_std.
- FS-S2-VAL-046 vérifier non-régression path hash DoS guard.
- FS-S2-VAL-047 vérifier non-régression nonce monotone HKDF.
- FS-S2-VAL-048 vérifier non-régression blob pipeline raw->encrypt.
- FS-S2-VAL-049 vérifier non-régression decrypt->decompress lecture.
- FS-S2-VAL-050 vérifier disponibilité du lot secondaire FS.

---

## 6) Synthèse actionnable

Le périmètre secondaire FS révèle que la robustesse globale dépend surtout des chemins de repli et des orchestrations commit/recovery/cache.
Le lot prioritaire cible `epoch/*`, `recovery/*`, `cache/*`, `crypto/*` et `observability/*` pour réduire le risque opérationnel.
Cette annexe sert de base de contrôle de non-régression pour les fichiers secondaires ExoFS.