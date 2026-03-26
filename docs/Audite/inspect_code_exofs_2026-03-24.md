# Analyse code réel ExoFS — état fonctionnel

## Fichiers inspectés
- `kernel/src/fs/exofs/mod.rs`
- `kernel/src/fs/exofs/lib.rs`
- `kernel/src/fs/exofs/storage/virtio_adapter.rs`
- `kernel/src/fs/exofs/syscall/object_write.rs`
- `kernel/src/fs/exofs/recovery/boot_recovery.rs`
- `drivers/storage/virtio_blk/src/lib.rs`
- `kernel/src/fs/exofs/cache/blob_cache.rs`
- `kernel/src/fs/exofs/epoch/mod.rs`
- `kernel/src/fs/exofs/posix_bridge/mod.rs`
- `kernel/src/fs/exofs/posix_bridge/vfs_compat.rs`

## Verdict synthétique

### Estimation de maturité du **code réel ExoFS**
- **Global code ExoFS : 43%**
- **Validé : 25%**
- **Partiel : 18%**
- **Non prouvé / probablement non finalisé : 57%**

Ce pourcentage reflète **le code exécutable réellement visible**, pas la documentation ni l’intention d’architecture.

## Pourquoi 43% ?

### 1. Ce qui est réellement implémenté et crédible
#### a) Initialisation ExoFS branchée dans le kernel
- `kernel/src/lib.rs` appelle bien `crate::fs::exofs::exofs_init(0u64);`
- `kernel/src/fs/exofs/mod.rs` existe et orchestre :
  - init du disque global,
  - appel recovery,
  - init POSIX bridge,
  - création d’un thread GC.

**Conclusion** : ExoFS est bien câblé au boot du kernel.  
**Niveau** : réel, mais superficiel.

#### b) Cache mémoire fonctionnel
- `cache/blob_cache.rs` implémente un vrai cache mémoire :
  - map de blobs,
  - insert/get/invalidate,
  - dirty tracking,
  - stats,
  - éviction rudimentaire.
- `object_write.rs` écrit réellement dans ce cache.

**Conclusion** : il existe un **FS mémoire de type blob-cache**, avec sémantique de réécriture complète en RAM.

#### c) Une surface POSIX/VFS existe
- `posix_bridge/mod.rs` et `vfs_compat.rs` exposent lookup/open/read/write/mkdir/unlink/rename/readdir/truncate/symlink.
- Il y a une table de fds, une émulation d’inodes, des compteurs, des validations.

**Conclusion** : la surface API est large et le code n’est pas vide.  
Mais la plupart des opérations restent **émulées** ou **simplifiées**.

#### d) Quelques hooks de persistance existent
- `object_write.rs` tente d’écrire sur `GLOBAL_DISK`.
- `storage/virtio_adapter.rs` fournit un adaptateur de type `BlockDevice`.
- Il y a un flush appelé après écriture.

**Conclusion** : il existe bien un **hook code → block device**, donc ce n’est pas purement déclaratif.

---

### 2. Ce qui est seulement partiel ou naïf
#### a) Recovery boot non réellement exécuté
Le point clé :
- `exofs_init()` appelle `boot_recovery_sequence(disk_size_bytes)`.
- Mais `boot_recovery_sequence()` dans `recovery/boot_recovery.rs` est un stub pratique :
  - log start,
  - audit started,
  - log done,
  - audit completed,
  - `Ok(())`.

Le vrai moteur `BootRecovery::run(device, options)` existe, mais **n’est pas utilisé dans le chemin d’init visible**.

**Impact** :
- le recovery complet est codé en théorie,
- mais au boot réel, la séquence branchée ne fait pas de lecture disque ni de sélection de slot ni de replay réel.

**Conclusion** : recovery **présent dans l’arbre**, mais **non prouvé comme actif dans le boot réel**.

#### b) VirtIO “réel” très douteux
Dans `kernel/src/fs/exofs/storage/virtio_adapter.rs` :
- `init_global_disk()` initialise toujours :
  - base fixe `0x1000_0000`
  - capacité fixe `1024 * 1024 * 512`
- `total_blocks()` retourne une constante simulée `1024 * 1024`
- `flush()` retourne toujours `Ok(())`

Dans `drivers/storage/virtio_blk/src/lib.rs` :
- le driver stocke les blocs dans `internal_storage: Mutex<Vec<u8>>`
- commentaire explicite :
  - “mock disque QEMU”
  - “pour l’intégration initiale”
  - “nous mockons”
- imports `virtio_drivers::{..., VirtIOBlk}` présents, mais **non utilisés dans la logique réelle exposée au kernel**.

**Conclusion** :
- il y a un adaptateur d’API,
- mais le backend actuel visible est **un faux disque en RAM**, pas une persistance matérielle démontrée.

#### c) Écriture bloc naïve et non formatée
Dans `object_write.rs` :
- l’objet est d’abord entièrement reconstruit en RAM,
- puis écrit bloc par bloc sur disque via :
  - `base_lba = (blob_id.0[0] as u64) * 100`
- cette stratégie est clairement naïve :
  - collision probable entre objets,
  - mapping basé sur 1 octet,
  - pas de table d’allocation,
  - pas de métadonnées on-disk robustes,
  - pas de gestion multi-blocs sérieuse,
  - pas de journal transactionnel réel sur ce chemin.

Le code ignore aussi les erreurs d’écriture :
- `let _ = dev.write_block(...)`
- `let _ = dev.flush()`

Donc même si le backend tombait, l’appel peut réussir côté FS.

**Conclusion** : présence d’un chemin de persistance, mais **prototype fragile**, probablement non finalisé.

#### d) VFS/POSIX essentiellement émulateur
Exemples nets dans `vfs_compat.rs` :
- `vfs_read()` retourne un remplissage à zéro, commentaire :
  - “ZeroFill — un vrai impl lirait BLOB_CACHE ici.”
- `vfs_write()` :
  - avance l’offset,
  - met à jour la taille,
  - **n’écrit pas le contenu dans ExoFS**
- `vfs_readdir()` retourne seulement `"."` et `".."`.
- `vfs_lookup()` utilise un `hash_name(parent_ino, name)` synthétique.
- `rename` retire et recrée via hash, pas de vraie structure répertoire.
- `rmdir` dit explicitement que la vérification d’enfants est une simplification non implémentée.

**Conclusion** : la couche POSIX/VFS est **largement une façade fonctionnelle**, pas un FS POSIX complet.

---

### 3. Zones probablement non finalisées
#### a) `kernel/src/fs/exofs/lib.rs`
Ce fichier n’est pas une vraie bibliothèque active :
- c’est essentiellement un mémo documentaire sur des feature flags nightly.
- pas de logique réelle.

#### b) Superblock global
Dans `mod.rs` :
- `static mut EXOFS_SUPERBLOCK: Option<Arc<SuperblockInMemory>> = None;`
- visible mais non alimenté dans le flux inspecté.

**Conclusion** : structure annoncée, usage réel non démontré.

#### c) Epoch subsystem très riche mais pas prouvé dans le chemin critique
`epoch/mod.rs` ré-exporte énormément de composants :
- barriers,
- commit,
- recovery,
- root chain,
- writeback,
- checksum,
- snapshots, etc.

Mais dans les fichiers inspectés :
- le chemin d’écriture concret passe surtout par `BLOB_CACHE` + write_block naïf,
- pas par un protocole complet clairement visible de commit epoch durable sur le chemin standard.

**Conclusion** :
- beaucoup d’architecture existe,
- mais le raccord au datapath réel observable reste incomplet ou non prouvé.

#### d) Gestion d’erreurs insuffisante
Dans `object_write.rs` :
- erreurs disque ignorées,
- flush ignoré,
- pas de rollback,
- pas de validation d’atomicité.
Cela réduit fortement la confiance dans la “persistance”.

---

## Catégories détaillées

### Validé
#### 1. Intégration boot minimale — **70%**
- ExoFS est appelé au boot.
- Init globale présente.
- Pont POSIX initialisé.
- Thread GC créé.

Mais :
- pas de montage VFS global démontré ici,
- init recovery réelle absente.

#### 2. Stockage mémoire / cache blob — **75%**
- lecture/écriture cache crédibles,
- dirty tracking réel,
- logique exploitable.

Mais :
- principalement RAM,
- aucune preuve de cohérence durable.

#### 3. API syscall d’écriture d’objet — **60%**
- validation arguments,
- copy userspace,
- update cache,
- tentative de persistance.

Mais :
- persistance primitive,
- erreurs d’E/S avalées,
- forte simplification.

### Partiel
#### 4. Persistance bloc / VirtIO — **25%**
- hook présent,
- adaptateur présent,
- write_block/read_block disponibles.

Mais :
- backend visible = `Vec<u8>` en mémoire,
- flush factice,
- total_blocks simulé,
- adresse MMIO codée en dur,
- aucune découverte/probing matériel visible ici.

#### 5. Recovery / fsck / epoch durable — **30%**
- beaucoup de code structurel existe.
- vrai orchestrateur `BootRecovery::run` présent.

Mais :
- au boot réel, le code utilisé est un stub minimal,
- aucune lecture disque effective dans le chemin d’init observé.

#### 6. POSIX/VFS — **35%**
- grande surface API.
- tables et structures présentes.

Mais :
- `read` = zero-fill,
- `write` ne persiste pas les données,
- `readdir` symbolique,
- répertoires synthétiques,
- sémantique partielle.

### Non prouvé / non finalisé
#### 7. FS durable “grandeur nature” — **15%**
Non prouvé par le code inspecté :
- absence de vrai backend hardware démontré,
- mapping on-disk naïf,
- pas de métadonnées disque robustes visibles dans le chemin standard,
- pas de transaction complète visible de bout en bout.

#### 8. Persistance VirtIO réelle — **10%**
Le code visible contredit une affirmation de persistance matérielle pleinement opérationnelle :
- le “driver” exposé au kernel est un mock RAM.
- donc la persistance réelle sur disque virtio n’est **pas démontrée par ce code**.

---

## Signaux forts qui réduisent la note

1. **Commentaires explicites de mock**
   - `drivers/storage/virtio_blk/src/lib.rs` dit littéralement que l’intégration mocke le disque.

2. **`boot_recovery_sequence()` simplifié**
   - chemin réel d’init ne fait pas le recovery complet.

3. **`vfs_read()` zero-fill**
   - preuve directe que la couche VFS n’est pas branchée aux vraies données.

4. **`vfs_write()` sans écriture réelle**
   - seule la taille/position évolue.

5. **Mapping LBA basé sur `blob_id.0[0] * 100`**
   - typique d’un prototype.

6. **Erreurs disque ignorées**
   - impossible de qualifier cela de persistance fiable.

---

## Estimation finale recommandée au parent

### Pourcentage code réel
- **ExoFS code maturity réelle : 43%**

### Décomposition utile
- **Validé : 25%**
- **Partiel : 18%**
- **Non prouvé / non finalisé : 57%**

### Formulation courte recommandée
> Le code ExoFS est significativement avancé en surface API et en structures internes, avec un vrai cache mémoire, une init boot branchée et un début de pont bloc. En revanche, le fonctionnement réel observé reste majoritairement celui d’un prototype semi-fonctionnel : recovery réel non branché au boot, VFS encore émulé, persistance bloc naïve, et backend “VirtIO” visible qui reste en pratique un mock RAM. La documentation semble donc nettement plus optimiste que le code exécutable prouvé.

## Résumé en une phrase
**ExoFS ressemble davantage à un prototype avancé de FS mémoire avec hooks de persistance expérimentaux qu’à un système de fichiers durable réellement finalisé et prouvé de bout en bout.**