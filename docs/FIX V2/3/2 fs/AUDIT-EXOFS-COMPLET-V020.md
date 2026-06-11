# AUDIT-EXOFS — Passe profonde complète & feuille de route vers 100 % fonctionnel

| Champ | Valeur |
|---|---|
| Projet | ExoOS v0.2.0 "Strata" — **OS hybride** (FS en kernel Ring0 pour écriture rapide, par conception) |
| Dépôt | `github.com/darkfireeee/Exo-OS` |
| HEAD audité | `601f445` |
| Date | 2026-06-10 |
| Référentiel normatif | `docs/recast/ExoFS_Reference_Complete_v3.md` (« CE DOCUMENT EST LA LOI ») + `ExoOS_Corrections_04_ExoFS.md` |
| Périmètre | `kernel/src/fs/exofs/` — **~250 fichiers**, 20 sous-modules + chemin `fs_bridge` |
| Objectif | Rendre ExoFS **fonctionnel à 100 %** pour brancher les libs POSIX ensuite |
| Auditeur | claude-alpha |

> **Cadrage.** ExoOS est hybride **volontairement** : ExoFS tourne en Ring0 pour des écritures rapides (conforme à la spec, §1.1 : « Il tourne en Ring 0, no_std »). Le double-chemin Ring0/Ring1 signalé dans l'audit datapath précédent **n'est donc pas un défaut** — c'est l'architecture voulue. Cet audit se concentre exclusivement sur ce qui empêche ExoFS d'être un vrai FS complet.

---

## 0. Verdict en une phrase

**ExoFS est un FS dont les deux moitiés ne se rejoignent pas** : un **datapath fonctionnel mais minimal** (cache RAM → writeback paresseux de blobs **bruts** vers le disque) coexiste avec un **moteur transactionnel + un pipeline de stockage complets et corrects** (commit d'epoch conforme EPOCH-01, dédup/compression/chiffrement/checksum) qui ne sont **invoqués par aucun chemin de production**. Rendre le FS fonctionnel à 100 % = **brancher le moteur existant sur le datapath**, pas le réécrire.

C'est la bonne nouvelle : l'essentiel du code dur est déjà là et correct. Le travail est du **câblage**, pas de la création.

---

## 1. Ce qui FONCTIONNE déjà (vérifié, à préserver)

| Élément | Statut | Preuve |
|---|---|---|
| **Boot & montage** | ✅ | `exofs_init()` appelé à `lib.rs:311` : monte le disque virtio, lance recovery, démarre les kthreads GC + writeback. |
| **Persistance disque réelle** | ✅ | `object_store::persist_blob_data_if_disk` → `device.write_block` + `flush` (object_store.rs:543-588). Catalogue `BlobId→LBA` persisté. |
| **Commit d'epoch (cœur transactionnel)** | ✅ **conforme** | `commit_epoch` (epoch_commit.rs:120) respecte EPOCH-01 à la lettre : barrière-data → barrière-root → write-record → barrière-record → advance-superblock, avec vérification `bytes_written==104` (WRITE-01). |
| **Barrières NVMe** | ✅ câblées | `register_nvme_flush_fn` enregistré au boot (`mod.rs:187`, `virtio_adapter.rs:70`) → `flush_global_disk`. Compteur de flush non-hookés pour détecter le no-op. |
| **Structures on-disk** | ✅ ONDISK-OK | Séparation Disk/InMemory respectée (`PhysicalBlobDisk` plain vs `PhysicalBlobInMemory` AtomicU32 ; `EpochRecord` repr(C,packed) 104 B). Aucun AtomicU64/Vec dans une struct on-disk. |
| **Refcount** | ✅ REFCNT-OK | `logical_object.rs:335` et `physical_blob.rs:276` utilisent `compare_exchange_weak` (pas de `fetch_sub` nu sur les ref_count d'objets). |
| **GC / deadlock** | ✅ DEAD-01 OK | Le GC ne référence **jamais** `EPOCH_COMMIT_LOCK` (recherche exhaustive : 0 occurrence dans `gc/` et `gc_trigger`). |
| **fsync** | ✅ partiel | `fs_fsync` (fs_bridge.rs:3002) persiste réellement le blob dirty via `persist_blob_data_if_disk(..., sync=true)` puis `mark_clean`. |
| **readdir / truncate / fallocate** | ✅ présents | `sys_exofs_readdir` sérialise en `linux_dirent64` ; `fs_truncate`/`fs_ftruncate`/`fs_fallocate` implémentés. |
| **execve depuis ExoFS** | ✅ zéro-copie | `resolve_blob_id → read_blob_from_cache → Arc<[u8]>` (cf. audit datapath B1). |

---

## 2. 🔴 LE BLOCAGE CENTRAL — le moteur transactionnel est débranché

### EXOFS-CORE-1 · P0 — `commit_epoch` n'est jamais appelé en fonctionnement normal

**Constat (recherche exhaustive sur tout le kernel) :** les seuls appelants non-test de `commit_epoch`/`do_commit` sont :
- `mod.rs:206` → `exofs_shutdown()` (un commit unique au **démontage**) ;
- le syscall `SYS_EXOFS_EPOCH_COMMIT` (epoch_commit.rs:592) — invoquable depuis l'userspace mais **jamais déclenché par write/fsync**.

**Aucun** `write()`, `fsync()`, `create()`, `delete()` ne déclenche de commit d'epoch.

**Conséquence sur le datapath réel :**
```
fs_write → BLOB_CACHE.write_at (RAM)  →  [writeback async] persist_blob_data_if_disk → device.write_block (blob BRUT)
```
Les données sont écrites en RAM puis copiées paresseusement sur disque **sans jamais passer par un commit d'epoch**. Donc :
- **Pas d'atomicité.** Un crash entre deux `write_block` laisse des blobs partiels sans EpochRecord pour les valider ni les annuler.
- **Recovery sans objet.** `boot_recovery_sequence` (appelé au boot) cherche l'epoch valide… mais comme rien ne commit d'epoch en écriture, il n'y a essentiellement **aucun epoch à recouvrer** au-delà de l'état de démontage propre.
- **La raison d'être d'ExoFS** (FS transactionnel par epochs) **n'est pas réalisée** sur le chemin chaud.

**C'est LE point qui empêche un FS fonctionnel à 100 %.** Tout le mécanisme correct existe — il faut l'appeler.

**Recommandation (câblage, pas réécriture) :**
1. Introduire un **batch d'epoch** : accumuler les blobs/métadonnées modifiés depuis le dernier commit (déjà à moitié là via `BLOB_CACHE.collect_dirty()`).
2. Déclencher `commit_epoch` (a) périodiquement dans le **writeback thread** (au lieu du `persist` brut isolé), (b) sur `fsync`/`fdatasync`, (c) sur `SYS_EXOFS_SYNC`.
3. Faire écrire au writeback l'**EpochRoot + EpochRecord** après les données, en réutilisant `EpochCommitArgs` déjà construit dans `mod.rs:206`.

### EXOFS-CORE-2 · P0 — Le pipeline de stockage (dédup/compression/chiffrement/checksum) est inerte

**Constat :** `blob_writer.rs` documente et implémente le pipeline canonique :
```
raw_data → BlobId(Blake3) → dédup → compression(LZ4/Zstd) → chiffrement(XChaCha20) → checksum → disque
```
Mais **`BlobWriter` / `CompressWriter` / `choose_compression` / le chiffrement XChaCha20 ne sont appelés par AUCUN chemin** (recherche exhaustive depuis `syscall/`, `fs_bridge`, `cache/` : 0 résultat). Le datapath réel (`object_store::persist_blob_data_if_disk`) écrit les blocs **bruts, non compressés, non chiffrés, sans checksum de bloc**.

**Conséquences :**
- **Aucune déduplication** réelle → l'espace disque n'est pas optimisé (alors que `dedup/` compte 14 fichiers complets).
- **Aucune compression** → débit disque et empreinte non optimisés.
- **`ObjectKind::Secret` n'est pas chiffré sur disque** — les secrets sont écrits en clair, ce qui contredit la garantie crypto de la spec (CRYPTO-*, LOBJ-03).
- **Pas de checksum par bloc** → corruption disque silencieuse non détectée à la relecture (alors que `checksum_reader.rs`/`checksum_writer.rs` existent).

**Recommandation :** router `persist_blob_data_if_disk` **à travers** `BlobWriter` (qui orchestre déjà dédup→compress→crypto→checksum) au lieu d'écrire brut. Brancher le `DecompressReader`/`checksum_reader` symétriquement sur `load_blob_data_if_available`. Pour les `Secret`, rendre le passage par XChaCha20 **obligatoire** (refuser l'écriture brute).

### EXOFS-CORE-3 · P1 — Résolution de chemin : vérifier que PathIndex on-disk est réellement consulté

**Constat :** `resolve_path_to_blob` / `resolve_blob_id` existent et sont appelés (par le loader ELF, par `fs_open`), mais ma recherche n'a pas trouvé de lien direct entre `path_resolve.rs` et le **PathIndex persistant** (`path/path_index*.rs`, `objects/object_kind/path_index.rs`). À confirmer : la résolution s'appuie-t-elle sur l'index on-disk (donc survit au reboot) ou sur une table RAM reconstruite ?

**Pourquoi c'est critique :** si les chemins ne sont résolus que via une structure RAM, alors **après reboot, les fichiers créés ne sont plus retrouvables par chemin** même si leurs blobs sont sur disque — le FS « perd » son arborescence. C'est un prérequis absolu d'un FS fonctionnel.

**Recommandation :** tracer `resolve_path_to_blob` jusqu'à sa source de vérité ; garantir que `create`/`rename`/`delete` mettent à jour le PathIndex **dans un epoch** (lié à CORE-1), et que `boot_recovery` reconstruit l'index depuis le disque.

---

## 3. 🟠 Robustesse / conformité aux règles de la LOI

### EXOFS-ROB-1 · P1 — `default_flush_stub` : fenêtre de no-op silencieux au boot

**Fichier :** `epoch/epoch_barriers.rs:31-43`

Le hook de flush NVMe est initialisé à `default_flush_stub` (**no-op**) jusqu'à l'enregistrement par le block layer. La spec EPOCH-02 dit : « omettre un flush = corruption certaine au prochain crash ». Le code prévoit bien un **compteur de flushes non-hookés** (bonne défense) et l'enregistrement a lieu dans `exofs_init`. **Mais** : tout commit qui surviendrait **avant** `register_nvme_flush_fn` (ou si l'enregistrement échoue silencieusement faute de disque) effectue ses 3 « barrières » en no-op → **fausse durabilité**.

**Recommandation :** faire échouer `commit_epoch` (plutôt que no-op) si `is_nvme_flush_registered() == false` en production ; logguer un warning fort. En dev sans disque, l'autoriser explicitement via un flag.

### EXOFS-ROB-2 · P1 — `unwrap()`/`expect()` sur chemins de désérialisation disque

**Fichiers :** `crypto/key_storage.rs:927,944,953` (`.expect("assumed valid … wire value")`), `cache/blob_cache.rs:916,929` (`.unwrap()`).

La spec OOM-01/NO-STD interdit les panics en kernel. Les `expect` sur des valeurs **lues depuis le disque** (`key_kind_from_u8`, `slot_state_from_u8`) sont particulièrement dangereux : un octet corrompu sur disque → **panic kernel total** au lieu d'une `ExofsError` récupérable. C'est un vecteur de DoS par corruption.

**Recommandation :** remplacer ces `expect`/`unwrap` par une propagation `ExofsError::CorruptedMetadata`. Auditer tous les `from_u8`/`from_wire` lisant du disque.

### EXOFS-ROB-3 · P2 — `Vec::with_capacity` / `vec![]` hors `try_` dans des chemins non-test

**Fichiers (échantillon non-test) :** `cache/cache_warming.rs:129` (`Vec::with_capacity(take)`), `io/writer.rs:178`, `path/path_index.rs:620` (`Vec::with_capacity(148)`), `audit/audit_rotation.rs:444` (`alloc::vec![]`).

Violations OOM-03/OOM-04 : allocation infaillible → panic possible en OOM. La plupart des `vec![]` détectés sont en `#[cfg(test)]` (acceptable), mais ceux ci-dessus sont sur des chemins réels.

**Recommandation :** convertir en `try_with_capacity` / `try_reserve` avec propagation `NoMemory`. Priorité aux chemins `path/` et `cache/` (hot path).

### EXOFS-ROB-4 · P2 — `fsync` ne committe pas d'epoch (lié à CORE-1)

**Fichier :** `fs_bridge.rs:3002`

`fs_fsync` persiste le blob (`sync=true`) mais **ne déclenche pas de commit d'epoch** ni ne flush les **métadonnées** (taille, cursor, PathIndex). Un `fsync()` POSIX doit garantir la durabilité **données + métadonnées**. Ici seules les données du blob sont forcées. `data_only` est même ignoré (`let _ = data_only;`).

**Recommandation :** après le branchement CORE-1, faire que `fsync` déclenche un commit d'epoch couvrant les métadonnées de l'objet ; respecter `data_only` (fdatasync = données seules).

---

## 4. État détaillé des 20 sous-modules

| Sous-module | Fichiers | État fonctionnel | Action pour 100 % |
|---|---|---|---|
| **core/** | 14 | ✅ Types solides, ONDISK respecté | RAS |
| **objects/** | ~20 | ✅ L-Obj/P-Blob, extent-tree, inline | Vérifier promotion Class1→Class2 sur mmap-write (LOBJ-04) |
| **storage/** | ~30 | ⚠️ Écrit brut (pipeline débranché) | **CORE-2** : router via `BlobWriter` |
| **epoch/** | ~20 | ✅ commit conforme, ⚠️ jamais appelé | **CORE-1** : déclencher sur write/fsync/writeback |
| **cache/** | 12 | ✅ blob/object/path/extent caches | Brancher writeback sur commit (CORE-1) |
| **path/** | 13 | ⚠️ PathIndex on-disk à confirmer | **CORE-3** : garantir persistance + recovery de l'index |
| **gc/** | 18 | ✅ tricolore, DEAD-01 OK | Valider que GC respecte EPOCH_PINNED (GC-07) |
| **dedup/** | 14 | ⚠️ Complet mais inerte | **CORE-2** : brancher dans le pipeline d'écriture |
| **compress/** | 11 | ⚠️ LZ4/Zstd prêts mais inertes | **CORE-2** : brancher |
| **crypto/** | 13 | ⚠️ XChaCha20 prêt mais Secrets en clair | **CORE-2** : chiffrement Secret obligatoire ; ROB-2 (expect) |
| **relation/** | 11 | ✅ graphe typé | Vérifier que GC traverse les relations (GC-02) |
| **snapshot/** | 12 | ✅ epoch-as-snapshot | Dépend de CORE-1 (snapshots = epochs durables) |
| **recovery/** | 16 | ✅ fsck 4 phases, boot recovery | Devient utile une fois CORE-1 branché |
| **io/** | 14 | ✅ buffered/direct/zero-copy/readahead | Brancher `ZeroCopyReader` sur `fs_read` (cf. audit datapath Z1) |
| **quota/** | 7 | ✅ capability-bound | Vérifier enforcement sur write réel |
| **audit/** | 9 | ✅ ring buffer non-bloquant | RAS |
| **numa/** | 6 | ✅ awareness | RAS (optimisation) |
| **observability/** | 11 | ✅ métriques/tracing/health | RAS |
| **posix_bridge/** | 5 | ✅ inode-emul, mmap, fcntl-lock | Brancher sur le datapath complet |
| **syscall/** | ~28 | ✅ 500-518 mappés | Lier `object_write` au pipeline (CORE-2) |
| **export/** | 10 | ✅ EXOAR | RAS (hors chemin chaud) |

**Lecture d'ensemble :** la quasi-totalité des sous-modules est **implémentée et correcte**. Le problème n'est pas l'absence de code mais **trois jonctions manquantes** (CORE-1, CORE-2, CORE-3) qui relient les modules avancés au datapath.

---

## 5. Conformité aux règles normatives (la LOI)

| Règle | Verdict | Note |
|---|---|---|
| **DAG-01** (deps autorisées) | ✅ | `epoch/` utilise des callbacks injectés (`CommitInput.callbacks`) — pas d'import direct de `ipc/process/arch`. Bon découplage. |
| **EPOCH-01** (ordre barrières) | ✅ | Respecté à la lettre dans `commit_epoch`. |
| **EPOCH-02** (flush obligatoire) | ⚠️ | Mécanisme correct mais no-op possible avant enregistrement (ROB-1). |
| **REFCNT-01** (underflow) | ✅ | `compare_exchange_weak` sur les ref_count d'objets. |
| **ONDISK-03/04** (pas d'Atomic/Vec on-disk) | ✅ | Séparation Disk/InMemory partout. |
| **DEAD-01 / EPOCH-04** (GC ≠ EPOCH_COMMIT_LOCK) | ✅ | 0 occurrence dans le GC. |
| **OOM-01/03/04** (try_ obligatoire) | ⚠️ | Quelques `with_capacity`/`vec!` non-try sur chemins réels (ROB-3). |
| **UNSAFE-02** (SAFETY comments) | ✅ | `epoch_record.rs` abondamment commenté. |
| **CRYPTO-02/03** (compress avant crypto, nonce unique) | ⚠️ | Ordre correct dans `blob_writer` **mais pipeline inerte** (CORE-2) → non vérifiable en pratique tant que débranché. |
| **LOBJ-03 / SEC-07** (Secret jamais en clair) | 🔴 | **Violé en pratique** : datapath écrit brut → Secrets non chiffrés (CORE-2). |
| **PATH-10** (rename atomique) | ⚠️ | À vérifier sous CORE-3 (rename dans un epoch). |

---

## 6. Feuille de route priorisée vers 100 % fonctionnel

L'objectif (brancher les libs POSIX ensuite) exige que le Fso soit **durable, atomique et cohérent après reboot**. Ordre recommandé :

**Étape 1 — Durabilité transactionnelle (débloque tout le reste)**
1. **EXOFS-CORE-1** : brancher `commit_epoch` sur le writeback thread + `fsync` + `SYS_EXOFS_SYNC`. Réutilise `EpochCommitArgs` déjà construit. → *Atomicité et recovery deviennent réels.*
2. **EXOFS-ROB-1** : faire échouer le commit si flush NVMe non enregistré en prod. → *Plus de fausse durabilité.*

**Étape 2 — Intégrité & cohérence après reboot**
3. **EXOFS-CORE-3** : garantir que la résolution de chemin s'appuie sur le PathIndex on-disk et que create/rename/delete l'updatent dans un epoch. → *L'arborescence survit au reboot.*
4. **EXOFS-ROB-2** : éliminer les `expect`/`unwrap` sur désérialisation disque. → *Corruption disque ≠ panic kernel.*

**Étape 3 — Pipeline de stockage (sécurité + efficacité)**
5. **EXOFS-CORE-2** : router les écritures via `BlobWriter` (dédup→compress→crypto→checksum) ; chiffrement Secret obligatoire ; checksum par bloc à la relecture. → *Secrets protégés, espace optimisé, corruption détectée.*

**Étape 4 — Conformité POSIX fine (prérequis libs)**
6. **EXOFS-ROB-4** : `fsync` durabilise données **+ métadonnées** ; respecter `fdatasync`.
7. Brancher `ZeroCopyReader` sur `fs_read` (perf, cf. audit datapath Z1) ; vérifier rename atomique (PATH-10), promotion Class1→Class2 sur mmap-write (LOBJ-04), enforcement quota sur write.

**Étape 5 — Validation**
8. Activer/étendre les tests d'intégration existants (`tests/integration/tier_*`) **après** branchement, en particulier `tier_3_stress` et `tier_6_virtio_vfs`, pour valider durabilité + recovery sur crash simulé.

---

## 7. Note de synthèse pour Eric

L'effort déjà investi dans ExoFS est considérable et de bonne qualité : structures on-disk correctes, commit d'epoch rigoureusement conforme à ta propre LOI, pipeline crypto/compress/dedup complet, recovery 4 phases, GC tricolore sans deadlock. **Rien de tout cela n'est à jeter.**

Le FS n'est « pas fonctionnel à 100 % » non pas parce qu'il manque des briques, mais parce que **trois fils ne sont pas connectés** entre les briques avancées et le chemin d'écriture réel :
- le **commit d'epoch** (durabilité transactionnelle) ne se déclenche qu'au démontage ;
- le **pipeline de stockage** (compression/chiffrement/dédup) est court-circuité par une écriture brute ;
- la **persistance du PathIndex** reste à confirmer pour la survie de l'arborescence au reboot.

Ces trois jonctions sont du **câblage ciblé**, pas une refonte. Une fois faites, ExoFS sera un vrai FS durable et cohérent, prêt à recevoir la couche libs POSIX pour la compatibilité à l'échelle d'un FS de production.

---

*AUDIT-EXOFS-COMPLET-V020.md — ExoOS v0.2.0 Strata — HEAD `601f445` — 2026-06-10 — claude-alpha*
