# Analyse structurée de `ExoFS_Reference_Complete_v3.md`

## 1. Positionnement d’ExoFS dans le périmètre du document

Ce document est une **spécification normative forte** pour ExoFS côté kernel, avec vocabulaire impératif (`INTERDIT`, `OBLIGATOIRE`, `CRITIQUE`, `LA LOI`). Il ne décrit pas seulement une architecture cible : il impose des **contraintes de code**, de layout mémoire/disque, d’ordonnancement des écritures, de verrouillage, de sécurité et de recovery.

Le périmètre utile pour la construction d’**ExoFS uniquement** est :

- `kernel/src/fs/exofs/` comme cœur d’implémentation.
- Dépendances internes strictement autorisées :
  - `memory/`
  - `scheduler/`
  - `security/capability/`
- Interfaces d’intégration minimales :
  - `fs/core/vfs.rs`
  - `syscall/*`
  - quelques extensions ciblées à d’autres modules déjà listées en section 12 du document.

Le document distingue explicitement :

- **Ring 0 / kernel / no_std** : mécanismes critiques, persistence, commit, recovery, VFS bridge, mmap, GC.
- **Ring 1 / serveur POSIX** : politique, conversion POSIX, errno, traduction de chemins “humains”, NFS, outils.

Pour l’implémentation de ExoFS dans le code actuel, cela signifie que **le cœur ExoFS ne doit pas essayer de tout faire POSIX nativement**. Il doit fournir les mécanismes kernel nécessaires, et non la politique complète.

---

## 2. Architecture conceptuelle d’ExoFS

## 2.1 Modèle fondamental

ExoFS repose sur un modèle :

- **LogicalObject (L-Obj)** : identité stable exposée au VFS / aux applications.
- **PhysicalBlob (P-Blob)** : contenu physique adressé par hash.
- **PhysicalRef** dans le L-Obj :
  - `Unique { blob_id }`
  - `Shared { blob_id, share_idx, is_writer }`
  - `Inline { data[512], len, checksum }`

C’est la séparation clé qui permet :

- déduplication
- copy-on-write
- snapshots
- relations typées
- potentiel chiffrement sélectif des secrets

### Invariants majeurs

- Le **VFS et les syscalls manipulent des `ObjectId`**, pas des `BlobId`.
- Le `BlobId` est un identifiant **interne kernel**.
- `BlobId = Blake3(contenu brut non compressé, non chiffré)`.
- Le `LogicalObject` possède l’identité stable, les métadonnées, les droits, et la référence vers le contenu physique.
- Les petits contenus peuvent être **inline** dans le L-Obj.

## 2.2 Classes d’objets

Le document impose deux classes :

- **Class1** : immuable
- **Class2** : mutable / CoW

Règles importantes :

- `PathIndex` est **toujours Class2**
- un objet `Class1` ne peut pas être `mmap` writable directement
- promotion `Class1 -> Class2` obligatoire dans certains cas (`MAP_SHARED|PROT_WRITE`)

## 2.3 Types d’objets

`ObjectKind` prévu :

- `Blob`
- `Code`
- `Config`
- `Secret`
- `PathIndex`
- `Relation`

Cela a un impact concret sur le code :
- validation spécifique potentielle
- politique d’exposition des données
- chiffrement des secrets
- intégration avec le graphe de relations
- contraintes du répertoire via `PathIndex`

---

## 3. Architecture modulaire imposée

Le document donne une arborescence quasi-spécificative du module `kernel/src/fs/exofs/`. Tout n’a probablement pas besoin d’être construit dès la première passe, mais la structure révèle la décomposition voulue.

## 3.1 Sous-systèmes réellement structurants pour ExoFS

Les modules structurants pour un ExoFS fonctionnel sont :

- `core/`
- `storage/`
- `epoch/`
- `objects/`
- `path/`
- `io/`
- `cache/`
- `recovery/`
- `posix_bridge/`
- `syscall/`

Les autres (`dedup/`, `compress/`, `crypto/`, `snapshot/`, `relation/`, `quota/`, `gc/`, `observability/`, `audit/`, `numa/`, `export/`) sont importants dans la vision cible, mais relèvent en partie de couches avancées ou d’extensions.

## 3.2 Dépendances internes autorisées

Règle de DAG absolue :

`fs/exofs/` ne peut dépendre directement que de :

- `memory/`
- `scheduler/`
- `security/capability/`

Interdictions explicites :

- pas d’import direct `ipc/`
- pas d’import direct `process/`
- pas d’import direct `arch/`

### Implication code

Si ExoFS a besoin de services de plus haut niveau :
- il faut passer par **traits abstraits**
- ou par **injection au boot**
- ou via une interface VFS/block layer déjà exposée

C’est une contrainte de design réelle, pas simplement documentaire.

---

## 4. Structures de données critiques

## 4.1 Types cœur (`core/`)

Le document fixe des types structurants :

- `ObjectId = [u8; 32]`
- `BlobId = [u8; 32]`
- `EpochId = u64`
- `SnapshotId = u64`
- `DiskOffset = u64`
- `Extent { offset, len }`

Il y a aussi des constantes clés :

- `MAGIC = 0x45584F46`
- slots d’epochs fixes (`4KB`, `8KB`, slot C en fin de disque)
- `HEAP_START = 1MB`
- `EPOCH_MAX_OBJECTS = 500`

Cela impose :
- un format binaire déterministe
- des offsets fixes
- des limites de transaction epochale

## 4.2 Superblock

Le document corrige explicitement une erreur antérieure : **séparer disque et mémoire**.

### `ExoSuperblockDisk`
Structure on-disk :
- `#[repr(C, align(4096))]`
- types plain uniquement
- checksum Blake3 sur toute la structure sauf champ checksum
- champs de layout, compteurs persistés, flags de compatibilité, UUID, volume name

### `ExoSuperblockInMemory`
Wrapper RAM :
- contient `disk: ExoSuperblockDisk`
- compteurs live atomiques (`object_count`, `free_bytes`)
- `dirty: AtomicBool`

### Invariants

- vérifier **magic avant checksum**
- pas d’`AtomicU64` on-disk
- `root_inode()` du superblock VFS doit être fonctionnel

C’est clairement du **code concret imposé**.

## 4.3 EpochRecord

`EpochRecord` est spécifié quasi-octet par octet :

- `#[repr(C, packed)]`
- taille exacte `104 bytes`
- `magic`
- `version`
- `flags`
- `epoch_id`
- `timestamp`
- `root_oid`
- `root_offset`
- `prev_slot`
- `checksum[32]`

Avec :
- `const assert size_of == 104`

Ici, la spécification est suffisamment précise pour être traduite directement en structure Rust.

## 4.4 EpochRoot

`EpochRoot` est variable et chaînable :

- liste objets modifiés
- liste objets supprimés
- liste deltas de relations
- pointeur `next_page`
- checksum par page
- chaque page a son propre magic + checksum

### Invariant critique

On ne fait **jamais confiance** à `next_page` sans valider la page pointée.

## 4.5 LogicalObject

Le document décrit même le layout cache-line du `LogicalObject` :
- `#[repr(C, align(64))]`
- hot path en première ligne de cache
- compteurs atomiques (`link_count`, `epoch_last`, `ref_count`)
- méta et références physiques dans les lignes suivantes

Même si ce niveau de détail peut être assoupli au début, l’intention est claire :
- objet optimisé lecture/écriture noyau
- séparation hot/cold data
- identité stable et compteurs atomiques

## 4.6 PhysicalBlob

Là aussi, séparation imposée :
- `PhysicalBlobDisk`
- `PhysicalBlobInMemory`

Le `ref_count` doit être en mémoire, atomique, avec protection contre underflow.

## 4.7 PathIndex

Répertoire ExoFS = `PathIndex`, un objet spécial.

On-disk :
- tableau trié d’entrées
- hash, object_id, name_len, kind

En mémoire :
- radix tree pour lookup

Règles :
- hash = `SipHash-2-4` avec clé secrète de montage
- collisions confirmées par comparaison nom complet
- split au-delà de `8192` entrées
- split atomique dans un **seul** EpochRoot

---

## 5. Pipeline d’écriture

Le document décrit plusieurs pipelines distincts mais cohérents.

## 5.1 Pipeline d’écriture logique

Chemin fonctionnel :

1. validation d’accès (`verify_cap`)
2. résolution de l’objet cible (`ObjectId`)
3. écriture dans page cache / buffer
4. marquage dirty
5. writeback asynchrone
6. sérialisation objets/blobs
7. écriture des payloads
8. génération de l’EpochRoot
9. écriture de l’EpochRecord
10. flush barrières NVMe
11. l’epoch devient visible/persistée

## 5.2 Pipeline blob standard

Pour `blob_writer.rs`, ordre imposé :

1. données brutes
2. calcul `BlobId = Blake3(brut)`
3. compression
4. chiffrement éventuel
5. écriture disque

Règle critique :
- **jamais hash sur données compressées**
- **jamais compression après chiffrement**

## 5.3 Pipeline secret

Pour `ObjectKind::Secret` :

1. données brutes
2. `Blake3(BlobId)`
3. compression
4. chiffrement
5. écriture disque

Le `BlobId` ne doit jamais être exposé.

## 5.4 Commit epochal : protocole de persistance

Ordre strict :

1. `write(payload)`
2. `nvme_flush()`
3. `write(EpochRoot)`
4. `nvme_flush()`
5. `write(EpochRecord slot)`
6. `nvme_flush()`

### Propriétés

- crash après phase 1 : orphelins, ignorés
- crash après phase 2 : root non référencé, ignoré
- crash après phase 3 : epoch valide

### Conséquence implémentation

Le code doit avoir :
- un **wrapper de barrières** testable/mockable
- un `EPOCH_COMMIT_LOCK`
- un ordre d’écriture strict
- aucune “optimisation” qui fusionne ou inverse ces étapes

---

## 6. Pipeline de lecture

## 6.1 Lecture objet

Pour lire un L-Obj :

1. localiser l’objet
2. lire `ObjectHeader`
3. vérifier `magic`
4. vérifier `checksum`
5. seulement ensuite lire le payload / métadonnées / extents

Invariants :
- jamais accéder au payload avant validation d’en-tête
- tout parseur on-disk doit commencer par `magic`

## 6.2 Lecture blob

1. lire blob physique
2. si chiffré : déchiffrer
3. si compressé : décompresser
4. vérifier checksum / contenu

Le document mentionne aussi :
- `compression_reader.rs`: décompression après vérification checksum
- `checksum_reader.rs`: erreur `Corrupt` si Blake3 invalide

Il y a ici une légère tension documentaire entre l’ordre “read blob” et les modules spécialisés, mais la ligne constante reste :
- le système doit distinguer clairement checksum logique du contenu, header de compression, et pipeline inverse.

## 6.3 Lecture de répertoire / path lookup

1. canonicalisation éventuelle
2. parsing composant par composant
3. itératif, jamais récursif
4. lookup dans `PathIndex`
5. collision hash → comparaison nom complet
6. usage de caches (`path_cache`, `dentry cache`)
7. gestion symlink limitée à profondeur 40

---

## 7. Persistance et layout disque

## 7.1 Layout fixe

Le layout disque est fortement structuré :

- offset `0` : superblock primaire
- `4KB` : epoch slot A
- `8KB` : epoch slot B
- `12KB` : superblock miroir
- `1MB` : début du heap général
- `size - 8KB` : epoch slot C
- `size - 4KB` : superblock miroir final

### Implications

- les calculs d’offset doivent utiliser `checked_add`
- les zones critiques ont des positions fixes
- les slots d’epoch sont redondants
- les miroirs de superblock servent à la validation croisée

## 7.2 Heap

Le stockage principal semble être :
- append-only pour objets/blobs
- avec métadonnées buddy / free map / coalescing

Cela suggère une dualité :
- append pour simplicité et recovery
- free map pour réutilisation / GC / gestion d’espace

## 7.3 Écriture fiable

Règles imposées :
- vérifier `bytes_written == expected`
- jamais ignorer un write partiel
- checksum streaming
- validation de chaque header avant parse

---

## 8. Recovery et fsck

## 8.1 Recovery au boot

Séquence boot imposée :

1. lire superblock
2. vérifier `magic`
3. vérifier checksum
4. vérifier version
5. vérifier miroirs
6. scanner slots A/B/C
7. prendre `max(epoch_id)` parmi slots valides
8. vérifier l’EpochRoot pointé, y compris pages chaînées

### Résultat

Le recovery nominal est conçu pour être **O(1)** côté sélection d’état actif, grâce aux slots.

## 8.2 fsck en 4 phases

Le document prévoit :

1. vérifier superblock + miroirs + flags
2. scanner heap, vérifier tous les headers
3. reconstruire graphe `L-Obj -> P-Blob -> extents`
4. détecter orphelins non atteints depuis racines

Réparations prévues :
- orphelins vers `lost+found`
- troncatures si nécessaire

## 8.3 Interaction recovery / GC

Le GC ne doit pas détruire :
- blobs référencés
- blobs encore dans fenêtre de protection de 2 epochs
- blobs avec `EPOCH_PINNED`

---

## 9. Sécurité

## 9.1 Modèle Zero Trust

Règle absolue :
- tout accès à un objet passe par `verify_cap(cap, object_id, rights)`

Le cœur ExoFS ne doit **jamais réimplémenter** la validation des capabilities.

## 9.2 Revocation

Le document impose :
- vérification O(1) par comparaison de génération
- révocation = incrément atomique du compteur de génération

Cela impacte les structures de métadonnées et l’intégration avec `security/capability`.

## 9.3 Confidentialité

- `BlobId` non exposé sans droit `INSPECT_CONTENT`
- pour `Secret`, `BlobId` jamais exposé
- `SYS_EXOFS_GET_CONTENT_HASH` doit être audité

## 9.4 Crypto

Contraintes concrètes :
- `HKDF` pour dérivation de clés
- `XChaCha20-Poly1305`
- nonce unique par objet
- jamais réutiliser nonce + clé
- crypto shredding par oubli de clé d’objet

Pour une première implémentation ExoFS, cela peut être partiellement différé, mais le **pipeline** et l’API doivent être pensés pour ne pas casser ce modèle plus tard.

---

## 10. Quotas

Le document introduit un modèle de quotas :
- **liés aux capabilities**
- pas aux UID
- enforcement avant allocation
- quotas par namespace possibles

### Ce qui relève du code concret

- un `quota_tracker`
- un point de contrôle **avant toute allocation**
- retour `ENOSPC` si dépassement
- audit des dépassements

### Ce qui ressemble davantage à une politique/spécification

- stratégie de reporting
- articulation fine avec containers / namespaces
- détails d’administration des quotas

---

## 11. VFS et compatibilité POSIX

## 11.1 Pont VFS minimal côté kernel

Le document est très clair : le POSIX complet n’est pas dans ExoFS kernel, mais il faut un **pont VFS Ring 0**.

Composants minimums :
- `inode_emulation.rs`
- `vfs_compat.rs`
- `mmap.rs`
- `fcntl_lock.rs`

## 11.2 Traits à implémenter concrètement

### `VfsSuperblock`
Doit fournir au moins :
- `root_inode()`
- `statfs()`
- `sync_fs(wait)`
- `alloc_inode()`

### `InodeOps`
Au moins :
- `lookup`
- `create`

### `FileOps`
Au moins :
- `read`
- `write`
- `fsync`

## 11.3 Règles de comportement

- `root_inode()` ne doit pas retourner `NotSupported`
- `write()` alimente page cache et marque dirty
- `fsync()` force commit epochal
- le writeback thread fait les commits périodiques
- on n’altère pas les traits VFS existants : on les implémente

### Conséquence pratique

Les milestones du document montrent bien l’ordre réel :
1. superblock / root inode
2. compat VFS open/read/write
3. init globale et boot

C’est très utile pour prioriser le code.

---

## 12. Cache, concurrence, verrous

## 12.1 Hiérarchie stricte des locks

Ordre imposé :
1. memory locks
2. wait queue locks
3. page table locks
4. inode locks
5. cache LRU
6. `PathIndex` lock
7. `EPOCH_COMMIT_LOCK`

### Invariants

- jamais prendre un lock de niveau inférieur après un supérieur
- relâcher inode lock avant sleep / I/O
- jamais dormir avec un spinlock
- GC ne doit jamais prendre `EPOCH_COMMIT_LOCK`

## 12.2 ObjectTable comme source unique

Règle `CACHE-01/CACHE-02` :
- ne pas créer des instances concurrentes d’un même `LogicalObject`
- tout accès passe par `ObjectTable::get()`

C’est une contrainte architecturale importante : il faut une table canonique d’objets en mémoire.

## 12.3 Caches attendus

- object cache
- blob cache
- path cache
- extent cache
- metadata cache
- block cache

Pour une première implémentation, tous ne sont pas forcément nécessaires, mais :
- object cache
- path cache
- block/page cache integration

semblent structurants.

---

## 13. Garbage collection

## 13.1 Modèle

GC tricolore :
- racines = EpochRoots valides
- traverse aussi les relations
- sweep des blobs avec `ref_count = 0` depuis au moins 2 epochs

## 13.2 Invariants critiques

- GC toujours en background
- pas dans chemin critique d’écriture
- file grise heap-allocated, jamais récursive
- `try_reserve` obligatoire
- délai minimum de 2 epochs avant suppression réelle
- ne jamais collecter si `EPOCH_PINNED`

## 13.3 Atomicité création blob

Séquence imposée :
1. allocation
2. `ref_count.store(1)`
3. barrière
4. insertion en table

Sinon race avec GC.

---

## 14. Erreurs silencieuses et leur traduction en exigences de code

Cette section est particulièrement précieuse car elle distingue les zones où “ça compile” de celles où “ça corrompt”.

## 14.1 Arithmétique

- Tous les calculs d’offset disque doivent utiliser `checked_add`
- Sinon risque d’écraser le superblock

## 14.2 Writes partiels

- Tous les writes disque doivent vérifier la taille réellement écrite

## 14.3 Refcount

- pas de `fetch_sub` aveugle à zéro
- underflow = bug kernel, doit paniquer

## 14.4 Split de répertoire

- split atomique, un seul epoch

## 14.5 Récursion

- interdite dans GC, symlinks, walkers
- utiliser algorithmes itératifs + stockage heap

## 14.6 Hash

- hash avant compression/chiffrement

## 14.7 Chaînage d’epochs

- vérifier chaque page chaînée avant lecture

## 14.8 Deadlock

- GC et commit séparés via queue différée

Ces points sont de vraies **règles d’implémentation**, pas seulement des recommandations.

---

## 15. Ce qui relève de la spécification vs du code concret

## 15.1 Clairement spécification / vision cible

Ces éléments décrivent surtout la cible complète, parfois au-delà du minimum nécessaire immédiat :

- `dedup/` avancée avec CDC, MinHash, near-dedup
- `compress/` avec benchmark runtime et auto-tuning
- `crypto/` complet avec TPM sealing, rotation, audit riche
- `snapshot/` complet avec diff, streaming, restore
- `relation/` complet avec Tarjan itératif et requêtes riches
- `numa/`
- `observability/` détaillée
- `audit/` complet “jamais de perte d’événement”
- `export/EXOAR`
- `NFS` côté Ring 1
- schémas de validation `Code` / `Config`

Ce sont des directions d’architecture, parfois très prescriptives, mais pas forcément le MVP absolu pour “faire fonctionner ExoFS”.

## 15.2 Clairement code concret obligatoire

Ces éléments doivent se traduire directement en code pour qu’ExoFS soit conforme et bootable :

- séparation `Disk` / `InMemory` pour superblock et blobs
- `root_inode()` fonctionnel
- `VfsSuperblock`, `InodeOps`, `FileOps`
- pipeline commit 3 barrières
- recovery slots A/B/C
- `EpochRecord` taille 104
- `magic` vérifié avant checksum avant payload
- `BlobId` calculé sur brut avant compression
- `verify_cap()` avant accès
- `checked_add()` pour offsets
- `copy_from_user()` dans syscalls
- pas de `std`, pas de locks std
- allocations fallibles
- itératif plutôt que récursif
- ObjectTable comme source unique
- `PathIndex` avec hash keyed + confirmation par nom

## 15.3 Zone intermédiaire : code à préparer même si implémentation partielle

- quota hooks avant allocation
- `EPOCH_PINNED`
- writeback thread
- cache shrinker
- syscall range 500-518
- support secrets/chiffrement via interfaces, même si backend minimal au départ
- snapshot hooks au niveau epoch, même sans interface complète

---

## 16. Dépendances internes et interfaces à prévoir

## 16.1 Dépendances directes admises

### `memory/`
Utilisations attendues :
- frames
- DMA
- page cache / page table
- shrinker
- flags de frame (`EPOCH_PINNED`, `EXOFS_PINNED`)

### `scheduler/`
Utilisations attendues :
- `SpinLock`
- `RwLock`
- `WaitQueue`
- timers
- threads background (GC, writeback)

### `security/capability/`
Utilisations attendues :
- `verify_cap`
- `CapToken`
- `Rights`

## 16.2 Interfaces externes minimales à brancher

- VFS superblock/inode/file ops
- syscall table
- block device abstraction déjà disponible en pratique
- mount root au boot
- potentielle intégration avec fd table et page cache existants

---

## 17. Lecture orientée implémentation : noyau minimal à construire

Le document fournit implicitement un **MVP bootable ExoFS**.

## 17.1 Noyau minimal probable

1. `core/`
   - types, erreurs, constantes, flags, versions
2. `storage/`
   - layout
   - superblock disk/in-memory
   - read/verify
   - object/blob read-write basiques
3. `epoch/`
   - `EpochRecord`
   - slots
   - recovery
   - commit lock
   - commit 3 flush
4. `objects/`
   - `LogicalObject`
   - `PhysicalRef`
   - `ObjectHeader`
   - loader/builder minimal
5. `path/`
   - `PathIndex`
   - resolver itératif
   - composants, canonicalisation, symlink limité
6. `cache/`
   - object/path cache minimal
7. `io/`
   - read/write buffered minimal
   - fsync -> commit
8. `posix_bridge/`
   - `inode_emulation`
   - `vfs_compat`
   - `mmap` minimal sûr
9. `syscall/`
   - au moins le sous-ensemble réellement routé/nécessaire
10. `recovery/`
   - boot recovery minimal
11. `mod.rs`
   - `exofs_init()`
   - `exofs_register_fs()`

## 17.2 Éléments pouvant être stubés proprement au début

À condition de ne pas violer les invariants :
- dédup avancée
- compression multi-algo avancée
- crypto sophistiquée
- snapshot complet
- relations riches
- quotas détaillés
- observabilité/audit complets
- export/import

Mais les hooks/structures doivent rester compatibles avec la conception du document.

---

## 18. Checklist finale des composants ExoFS à construire dans le code

## A. Fondations `core/`
- [ ] Définir `ObjectId`, `BlobId`, `EpochId`, `DiskOffset`, `Extent`
- [ ] Définir `ObjectKind`, `ObjectClass`, `Rights`, `Flags`, `Version`
- [ ] Définir `ExofsError` et conversion vers `FsError`
- [ ] Ajouter helpers `ct_eq()`, générations d’IDs, constantes de layout
- [ ] Poser règles `no_std`, fallible allocation, `checked_add`

## B. Stockage / format disque
- [ ] Implémenter `storage/layout.rs` avec offsets fixes
- [ ] Implémenter `ExoSuperblockDisk` + `ExoSuperblockInMemory`
- [ ] Implémenter `read_and_verify()` : magic d’abord, checksum ensuite
- [ ] Implémenter miroirs de superblock et cross-validation
- [ ] Implémenter `ObjectHeader` et validation systématique
- [ ] Implémenter read/write objet avec vérification stricte des tailles écrites
- [ ] Implémenter pipeline blob : hash brut -> compression -> chiffrement -> disque
- [ ] Utiliser `checked_add()` partout sur offsets/tailles disque

## C. Epochs / atomicité / persistance
- [ ] Définir `EpochRecord` exact 104 bytes avec `const assert`
- [ ] Définir `EpochRoot` et chaînage multi-pages
- [ ] Implémenter slots A/B/C fixes
- [ ] Implémenter `EPOCH_COMMIT_LOCK`
- [ ] Implémenter commit à 3 barrières NVMe dans l’ordre strict
- [ ] Implémenter delta tracking des objets modifiés
- [ ] Implémenter sélection de l’epoch actif par `max(epoch_id)` valide
- [ ] Vérifier chaque page chaînée via magic + checksum

## D. Modèle d’objets
- [ ] Implémenter `LogicalObject`
- [ ] Implémenter `PhysicalBlobDisk` / `PhysicalBlobInMemory`
- [ ] Implémenter `PhysicalRef` (`Unique`, `Shared`, `Inline`)
- [ ] Implémenter `ObjectMeta`
- [ ] Implémenter `ObjectBuilder` validant les invariants
- [ ] Implémenter `ObjectLoader`
- [ ] Introduire une `ObjectTable` canonique comme source de vérité

## E. Répertoires / chemins
- [ ] Implémenter `PathIndex` on-disk + structure in-memory
- [ ] Implémenter hash keyed `SipHash-2-4`
- [ ] Gérer collisions par comparaison byte-à-byte du nom
- [ ] Implémenter `resolve_path()` itératif
- [ ] Utiliser buffers per-CPU pour `PATH_MAX`
- [ ] Implémenter canonicalisation (`.` / `..`)
- [ ] Implémenter symlink itératif avec profondeur max 40
- [ ] Implémenter split atomique `PathIndex` dans un seul epoch
- [ ] Prévoir rename atomique

## F. I/O / cache
- [ ] Implémenter chemin lecture : objet -> extents -> cache -> disque
- [ ] Implémenter chemin écriture : page cache dirty -> writeback -> commit epoch
- [ ] Faire `write()` sans commit direct
- [ ] Faire `fsync()` avec commit immédiat
- [ ] Mettre en place object/path cache minimal
- [ ] Prévoir shrinker mémoire
- [ ] Intégrer writeback thread périodique

## G. VFS / pont kernel
- [ ] Implémenter `VfsSuperblock` pour ExoFS
- [ ] Rendre `root_inode()` pleinement fonctionnel
- [ ] Implémenter `InodeOps::lookup/create`
- [ ] Implémenter `FileOps::read/write/fsync`
- [ ] Implémenter `inode_emulation` (`ObjectId -> ino_t`)
- [ ] Brancher `vfs_compat.rs`
- [ ] Prévoir `mmap` sûr avec promotion `Class1 -> Class2`
- [ ] Prévoir `fcntl` locks si requis par le VFS existant

## H. Recovery / intégrité
- [ ] Implémenter recovery boot : superblock -> slots -> epoch root
- [ ] Implémenter `slot_recovery` A/B/C
- [ ] Implémenter replay minimal de l’epoch actif si nécessaire
- [ ] Implémenter fsck phase 1–4 au moins en squelette structuré
- [ ] Implémenter réparations minimales sûres (`lost+found`, truncate)

## I. Sécurité
- [ ] Appeler `verify_cap()` avant tout accès objet
- [ ] Ne jamais exposer `BlobId` sans `INSPECT_CONTENT`
- [ ] Ne jamais exposer `BlobId` d’un `Secret`
- [ ] Implémenter `copy_from_user()` dans tous les syscalls
- [ ] Prévoir modèle de génération/révocation compatible capabilities
- [ ] Préparer pipeline crypto conforme pour `Secret`

## J. GC / lifecycle stockage
- [ ] Implémenter refcount blob sûr avec panic sur underflow
- [ ] Implémenter queue de suppression différée
- [ ] Respecter délai minimum de 2 epochs
- [ ] Ne jamais laisser le GC prendre `EPOCH_COMMIT_LOCK`
- [ ] Implémenter parcours itératif des racines et relations
- [ ] Respecter `EPOCH_PINNED`

## K. Quotas
- [ ] Définir policy de quota liée aux capabilities
- [ ] Vérifier quota avant allocation
- [ ] Retourner erreur type ENOSPC / équivalent si dépassement
- [ ] Prévoir tracking par capability et namespace

## L. Syscalls ExoFS
- [ ] Enregistrer la plage 500–518
- [ ] Implémenter validation commune userspace
- [ ] Implémenter au minimum resolve/open/read/write/create/delete/stat/set_meta/commit
- [ ] Auditer `GET_CONTENT_HASH`
- [ ] Conserver séparation mécanisme kernel / politique Ring 1

## M. Initialisation / boot
- [ ] Implémenter `exofs_init()`
- [ ] Lire et vérifier superblock + miroirs
- [ ] Récupérer l’epoch actif
- [ ] Initialiser caches
- [ ] Lancer threads background writeback + GC
- [ ] Enregistrer shrinker mémoire
- [ ] Enregistrer syscalls ExoFS
- [ ] Enregistrer et monter ExoFS dans le VFS

## N. Modifications externes minimales à prévoir
- [ ] Ajouter `FrameFlags::EPOCH_PINNED` et `EXOFS_PINNED`
- [ ] Ajouter rights `INSPECT_CONTENT`, `SNAPSHOT_CREATE`, `RELATION_CREATE`, `GC_TRIGGER`
- [ ] Ajouter numéros de syscalls 500–518
- [ ] Router syscalls vers `fs/exofs/syscall/`
- [ ] Déclarer `pub mod exofs;` dans `fs/mod.rs`
- [ ] Brancher `exofs_register_fs()`
- [ ] Compléter les stubs VFS existants nécessaires (`open/read/write/close`)
- [ ] Ajuster pools/structures scheduler/process minimales si le document l’exige

## Conclusion

`ExoFS_Reference_Complete_v3.md` n’est pas un simple document d’intention : c’est une **spécification exécutable** de l’architecture ExoFS kernel. Pour construire ExoFS dans le code, les priorités imposées par le document sont nettes :

1. **superblock + layout + recovery**
2. **epochs + commit atomique 3 barrières**
3. **objets logiques / blobs physiques**
4. **PathIndex et résolution de chemins**
5. **pont VFS fonctionnel**
6. **I/O buffered + fsync + writeback**
7. **sécurité capability-first**
8. **GC/quota/crypto/snapshots en extensions structurées**

Le point le plus important pour l’implémentation réelle est que le document fixe surtout des **invariants de sûreté** : ordre des writes, validation on-disk, absence de recursion, séparation Disk/InMemory, vérification capability, et hiérarchie des locks. Ce sont eux qui doivent guider toute construction d’ExoFS dans `kernel/src/fs/exofs/`.