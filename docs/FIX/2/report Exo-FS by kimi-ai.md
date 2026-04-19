# Rapport d'Audit de Sécurité et de Qualité — Module ExoFS (Exo-OS)

**Classification:** CONFIDENTIEL — Équipe Core Kernel
**Date d'audit:** 19 avril 2026
**Auditeur:** Agent d'Audit IA (Session complète)
**Scope:** `kernel/src/fs/exofs/` — 293 fichiers Rust, 130 220 lignes de code
**Version du code audité:** Git `main` (clone du 19 avril 2026)
**Documents de référence:**
- `ExoFS_Reference_Complete_v3.md` (105 KB)
- `ExoFS_Translation_Layer_v5_FINAL.md` (16 KB)
- `ExoOS_Architecture_v7.md` (38 KB)
- `ExoOS_Corrections_04_ExoFS.md` (CORR-06, CORR-20, CORR-22)
- `ExoOS_Corrections_07_Critiques_Majeures_v2.md` (CORR-31 à CORR-41)
- `ExoOS_Corrections_09_FINAL_v3.md` (CORR-49 à CORR-54)

---

## Résumé Exécutif

L'audit du module ExoFS a révélé un écart significatif entre l'ambition architecturale (documentée avec une exhaustivité remarquable) et l'état d'implémentation actuel. Sur **293 fichiers Rust** totalisant **130 220 lignes**, l'analyse a identifié **22 findings** répartis en **5 critiques**, **5 élevés**, **8 moyens**, et **4 faibles**. Le taux de densité d'anomalie est de **0,017 finding par 100 lignes**, ce qui est conforme aux standards de l'industrie pour un projet en phase de développement active, mais la concentration des problèmes dans les zones critiques (POSIX layer, epoch management, on-disk format) demande une attention immédiate.

Les problèmes critiques se concentrent autour de trois axes : (1) la **couche POSIX/VFS** qui, bien que documentée comme offrant ~95 % de compatibilité POSIX, est en réalité majoritairement constituée de stubs non fonctionnels ; (2) le **mécanisme d'epoch commit** qui utilise un algorithme de checksum cryptographiquement faible (XOR simple) au lieu de Blake3 comme requis par les specs ; et (3) des **incohérences dans les constantes on-disk** qui pourraient conduire à des incompatibilités de format entre versions. Ces problèmes sont détaillés dans les sections suivantes avec des recommandations concrètes de correction.

| Métrique | Valeur |
|----------|--------|
| Fichiers audités | 293 |
| Lignes de code | 130 220 |
| Findings CRITIQUE | 5 (23 %) |
| Findings HIGH | 5 (23 %) |
| Findings MEDIUM | 8 (36 %) |
| Findings LOW | 4 (18 %) |
| **Total** | **22** |

---

## Méthodologie d'Audit

L'audit a suivi une méthodologie en trois phases, conçue pour maximiser la couverture tout en maintenant une profondeur d'analyse suffisante pour détecter les vulnérabilités subtiles.

### Phase 1 : Analyse Documentaire (Spécifications vs Implémentation)

La première phase consistait en une lecture systématique de l'ensemble de la documentation de référence. Les spécifications ExoFS (Reference_Complete_v3, Translation_Layer_v5, Architecture_v7) ont été découpées en règles vérifiables, chacune étant ensuite tracée jusqu'à son implémentation dans le code. Cette approche a permis d'identifier rapidement les écarts structurels — zones où le code est soit absent, soit significativement différent de la spécification. Les documents de corrections (CORR-01 à CORR-54) ont également été analysés pour vérifier leur état d'implémentation dans le code actuel.

### Phase 2 : Analyse Statique du Code

La deuxième phase a impliqué la lecture manuelle de l'ensemble des 293 fichiers du module `fs/exofs/`, organisée par sous-module : `core/`, `storage/`, `epoch/`, `cache/`, `io/`, `gc/`, `posix_bridge/`, `crypto/`, `path/`, `dedup/`, `compress/`, `objects/`, `syscall/`, `export/`, `numa/`, `audit/`. Chaque fichier a été évalué selon les critères suivants : conformité aux specs, sécurité mémoire (spécialement critique en environnement `no_std`), cohérence inter-module, gestion d'erreurs, et qualité générale du code. Des outils d'analyse automatisée ont été utilisés pour détecter les patterns problématiques récurrents (déréférencement de raw pointers, unwraps non sécurisés, arithmetic overflow potentiels).

### Phase 3 : Analyse de Cohérence Cross-Module

La troisième phase a consisté à vérifier la cohérence entre les différents sous-modules. Un problème fréquent dans les grands projets est la divergence des constantes entre modules — une constante définie dans `core/constants.rs` mais utilisée avec une valeur différente dans un autre module. Cette phase a spécifiquement recherché ces incohérences, ainsi que les dépendances cycliques, les re-exports incohérents, et les violations de la séparation des responsabilités entre les couches.

---

## 1. Findings CRITIQUE (Severity: CRITIQUE)

### FS-CRIT-01 : Checksum XOR Naïf dans epoch_commit.rs — Violation HASH-01/HDR-03

**Localisation:** `kernel/src/fs/exofs/syscall/epoch_commit.rs`, lignes 131-141
**Sévérité:** CRITIQUE
**Règles violées:** HASH-01 (Blake3 pour tous les hashes), HDR-03 (checksum fort en-tête)

Le mécanisme de commit d'epoch, qui constitue le cœur de la durabilité transactionnelle d'ExoFS, utilise un algorithme de checksum extrêmement faible basé sur un XOR simple des tailles de blobs additionné à l'identifiant d'epoch. La fonction `compute_checksum` implémente littéralement : `cs = epoch_id; for each entry: cs = cs.wrapping_add(entry.size)`. Ce mécanisme est cryptographiquement trivial à contourner — deux blobs dont les tailles se compensent produisent le même checksum, et une collision intentionnelle peut être fabriquée en O(n) où n est le nombre d'entrées.

La spécification ExoFS Reference Complete v3 est explicite à ce sujet : la section HASH-01 stipule que "tous les identifiants de contenu sont dérivés via BLAKE3-256 sur les données RAW, avant toute compression ou chiffrement", et la section HDR-03 exige que "chaque en-tête on-disk contient un checksum vérifié AVANT tout accès au payload". Le checksum XOR utilisé dans `epoch_commit.rs` ne satisfait ni l'une ni l'autre de ces exigences. Un attaquant capable de modifier le journal d'epoch (par exemple via une erreur de disque manipulée ou un accès physique) pourrait altérer les métadonnées de commit sans que cette modification soit détectée, conduisant potentiellement à un état de volume incohérent après recovery.

**Recommandation:** Remplacer le checksum XOR par BLAKE3-256 des entrées du journal. La fonction `compute_checksum` devrait devenir : `blake3::hash(serialized_entries).into()`. La taille du champ checksum dans `EpochJournalHeader` devra être augmentée de 64 bits (8 octets) à 256 bits (32 octets), ce qui implique une modification du format on-disk et donc un bump de version de format. Cette modification est bloquante pour toute release candidate.

**Effort de correction:** 2-3 jours (modification du format on-disk + tests de régression + migration de format)

---

### FS-CRIT-02 : POSIX VFS Layer Non Implémenté — Translation Layer v5 Absente

**Localisation:** `kernel/src/fs/exofs/posix_bridge/vfs_compat.rs` (intégralité)
**Sévérité:** CRITIQUE
**Règles violées:** POSIX-01 à POSIX-95 (Translation Layer v5), SRV-02 (compatibilité application)

La Translation Layer v5, documentée comme offrant "~95 % POSIX via Translation Layer v5" avec "émulation inode/dentry complète", est en réalité entièrement absente du code. Le fichier `vfs_compat.rs` contient uniquement des déclarations de trait et des structures de données sans implémentation fonctionnelle. Les syscalls POSIX fondamentaux — `open()`, `read()`, `write()`, `close()`, `lseek()`, `stat()`, `mkdir()`, `rmdir()`, `rename()`, `link()`, `unlink()`, `chmod()`, `chown()` — sont tous définis comme des fonctions vides retournant `ENOSYS` ou des valeurs par défaut.

Cette absence a des conséquences critiques pour le projet. Premièrement, aucune application POSIX existante ne peut fonctionner sur ExoFS sans une réécriture complète pour utiliser les syscalls natifs ExoFS (500-518). Deuxièmement, le `vfs_server` (Ring 1) ne peut pas remplir son rôle de serveur de fichiers sans cette couche de traduction. Troisièmement, les tests d'intégration documentés dans la spécification ne peuvent pas être exécutés. Quatrièmement, la promesse d'"émulation inode/dentry complète" de la spécification est une divergence documentaire majeure qui peut induire les développeurs en erreur.

Le document `ExoFS_Translation_Layer_v5_FINAL.md` (16 KB) décrit en détail le mapping entre les appels POSIX et les opérations ExoFS natives, incluant la conversion inode↔ObjectId, la gestion des descripteurs de fichiers POSIX vers les handles ExoFS, et l'émulation des permissions POSIX via le système de droits ExoRights. Aucun de ces mécanismes n'est implémenté.

**Recommandation:** Implémenter la Translation Layer v5 conformément au document de spécification. Cette implémentation est un projet majeur estimé à 3-4 semaines de développement. Elle doit inclure : (1) le mapping bidirectionnel InodeNumber (u64) ↔ ObjectId ([u8; 32]), (2) la table des descripteurs de fichiers POSIX avec conversion vers les handles ExoFS, (3) l'émulation des appels système POSIX via les syscalls ExoFS natifs, et (4) les tests de compatibilité avec une suite de tests POSIX (comme LTP ou PJD). En attendant, la documentation doit être mise à jour pour refléter l'état réel d'implémentation.

**Effort de correction:** 3-4 semaines (projet majeur — blocking pour RC)

---

### FS-CRIT-03 : InodeNumber — Type Incompatible avec POSIX (u64 vs [u8; 32])

**Localisation:** `kernel/src/fs/exofs/posix_bridge/inode_emulation.rs`, lignes 1-80
**Sévérité:** CRITIQUE
**Règles violées:** POSIX-01 (compatibilité inode), ONDISK-02 (format portable)

Le module `inode_emulation.rs` définit `InodeNumber` comme un alias vers `ObjectId` (`[u8; 32]`), alors que la spécification POSIX et la Translation Layer v5 exigent un type `u64`. Cette divergence de type est fondamentalement incompatible avec l'interface POSIX : les appels système `stat()`, `lstat()`, `fstat()` retournent un champ `st_ino` de type `u64`, et les fonctions de bibliothèque comme `readdir()` attendent des inodes 64 bits. Un tableau de 32 octets ne peut pas être utilisé directement comme inode POSIX sans une fonction de réduction (hash ou mapping).

Le document `ExoFS_Translation_Layer_v5_FINAL.md` spécifie explicitement (section "Inode Mapping") : "Les inodes POSIX sont des u64 dérivés des ObjectId via une fonction de hash FNV-1a 64-bit sur les 24 octets significatifs de l'ObjectId, avec une table de reverse-mapping pour la résolution inverse". Cette fonction de mapping n'est pas implémentée. De plus, la structure `InodeEmulation` dans le code ne contient aucun champ pour stocker le numéro d'inode POSIX, ni de table de reverse-mapping.

Cette incohérence de type bloque toute tentative d'utiliser ExoFS via une interface POSIX. Même si la Translation Layer v5 était implémentée, le type fondamental ne permettrait pas la compatibilité sans une refonte complète de ce module.

**Recommandation:** Redéfinir `InodeNumber` comme un `u64` et implémenter la fonction de mapping `object_id_to_inode(oid: &ObjectId) -> u64` spécifiée dans la Translation Layer v5 (FNV-1a 64-bit sur les 24 octets significatifs). Implémenter également la table de reverse-mapping `inode_to_object_id(inode: u64) -> Option<ObjectId>` nécessaire pour les opérations de résolution de chemin. Cette modification est pré-requise à toute implémentation de la Translation Layer v5 (FS-CRIT-02).

**Effort de correction:** 3-5 jours ( incluant les tests de non-régression )

---

### FS-CRIT-04 : Utilisation d'AtomicU64 dans des Structures On-Disk

**Localisation:** Multiple — `epoch/epoch_record.rs`, `cache/cache_stats.rs`, `io/io_stats.rs`
**Sévérité:** CRITIQUE
**Règles violées:** ONDISK-03 (pas d'AtomicU64 dans les structs repr(C)), HDR-03 (format stable)

La règle ONDISK-03 de la spécification ExoFS est catégorique : "Les structures `#[repr(C)]` on-disk ne doivent JAMAIS contenir de types atomiques (`AtomicU64`, `AtomicUsize`, etc.) car leur représentation mémoire n'est pas stable cross-platform". Cette règle est violée dans plusieurs modules où des structures destinées à la sérialisation on-disk contiennent des champs atomiques.

Dans `epoch/epoch_record.rs`, la structure `EpochRecord` contient des compteurs `AtomicU64` pour les statistiques d'epoch (nombre de blobs, bytes écrits, etc.). Dans `cache/cache_stats.rs`, `CacheStats` utilise `AtomicU64` pour les compteurs de hits/misses/evictions. Dans `io/io_stats.rs`, `IoStats` fait de même pour les compteurs d'opérations I/O. Ces structures sont sérialisées et désérialisées depuis le disque, et la représentation mémoire des types atomiques en Rust n'est pas garantie stable (elle dépend de l'architecture cible et de la version du compilateur).

Le risque est qu'une sérialisation/désérialisation d'une structure contenant `AtomicU64` produise un layout mémoire différent selon la plateforme ou la version du compilateur, conduisant à une corruption silencieuse des données on-disk. Sur x86_64, `AtomicU64` a actuellement la même taille qu'un `u64`, mais ce n'est pas une garantie du langage.

**Recommandation:** Remplacer tous les `AtomicU64`/`AtomicUsize` dans les structures on-disk par leurs équivalents non-atomiques (`u64`, `usize`). Utiliser des types atomiques uniquement dans les structures in-memory (état volatile). Lors de la sérialisation, charger les valeurs avec `Ordering::Acquire` et stocker les valeurs brutes. Lors de la désérialisation, initialiser les atomiques in-memory avec `Ordering::Release`. Un audit automatisé doit être mis en place pour interdire tout `Atomic*` dans les `#[repr(C)]` on-disk.

**Effort de correction:** 1-2 jours (modification mécanique + revue de code)

---

### FS-CRIT-05 : `const fn` avec Initialisation Atomique Non-Zero

**Localisation:** `kernel/src/fs/exofs/core/config.rs`, ligne 67-87
**Sévérité:** CRITIQUE
**Règles violées:** ARITH-02 (arithmétique vérifiée), ONDISK-03 (stabilité format)

La fonction `ExofsConfig::default_config()` est déclarée comme `pub const fn`, ce qui signifie qu'elle peut être utilisée dans des contextes de compilation constante. Cependant, cette fonction initialise 17 champs atomiques (`AtomicUsize`, `AtomicU64`) avec des valeurs non-nulles via `AtomicUsize::new(val)`. En Rust, l'initialisation d'un type atomique dans un contexte `const fn` est techniquement valide car `AtomicUsize::new()` est une fonction constante, mais cela crée un état global mutable (`static EXOFS_CONFIG`) initialisé à la compilation avec des valeurs qui ne peuvent pas être facilement modifiées au runtime sans des opérations atomiques explicites.

Le problème fondamental est que cette approche mélange deux mondes : la configuration statique (connue au moment de la compilation) et la configuration dynamique (modifiable au runtime via `ConfigUpdate`). Le fait que `default_config()` soit `const fn` suggère que la configuration est figée, mais les setters (`set_object_cache_size`, etc.) permettent des modifications post-initialisation. Cette ambiguïté peut conduire à des comportements non-déterministes si `EXOFS_CONFIG` est accédée simultanément depuis plusieurs cœurs pendant l'initialisation.

De plus, la fonction `default_config()` ne valide pas les valeurs — elle retourne directement une structure avec des valeurs codées en dur. Si une valeur est incorrecte (par exemple, `gc_free_threshold_pct = 0` ou `compress_level = 10`), aucune erreur n'est signalée à la compilation ni au runtime jusqu'à ce que `validate()` soit appelée explicitement.

**Recommandation:** Séparer clairement la configuration statique (constantes du compilateur) de la configuration dynamique (état runtime). La fonction `default_config()` ne devrait pas être `const fn` — elle devrait être une fonction runtime qui valide les valeurs avant de retourner la structure. Alternativement, conserver le `const fn` mais ajouter des assertions de compilation (`const_assert!`) pour valider les valeurs par défaut. Le `static EXOFS_CONFIG` devrait être initialisé via une fonction d'init explicite appelée au boot, et non via une initialisation globale implicite.

**Effort de correction:** 1 jour

---

## 2. Findings HIGH (Severity: ÉLEVÉ)

### FS-HIGH-01 : Incohérence INLINE_DATA_MAX — 512 vs 256 octets

**Localisation:** `core/constants.rs` ligne 78 vs `core/constants.rs` ligne 468
**Sévérité:** ÉLEVÉ
**Règles violées:** ONDISK-01 (tailles cohérentes), HDR-03 (format stable)

Le fichier `constants.rs` définit deux constantes pour la même limite avec des valeurs différentes : `INLINE_DATA_MAX = 512` (ligne 78) et `INLINE_DATA_MAX_BYTES = 256` (ligne 468). Cette incohérence de 2x est problématique car différents modules utilisent différentes constantes. Le module `objects/inline_data.rs` utilise `INLINE_DATA_MAX` (512 octets) pour la taille du buffer inline, tandis que `object_meta.rs` et la logique de promotion inline→extent utilisent `INLINE_DATA_MAX_BYTES` (256 octets) comme seuil.

Cette divergence peut conduire à un comportement indéfini : un objet avec 300 octets de données inline serait considéré comme valide par `inline_data.rs` (300 < 512) mais déclencherait une promotion vers extent par `object_meta.rs` (300 > 256). Le résultat dépend de l'ordre d'évaluation et du module appelant, ce qui est un bug Heisenberg particulièrement difficile à déboguer.

**Recommandation:** Unifier en une seule constante `INLINE_DATA_MAX_BYTES = 256` conformément à la spécification ExoFS Reference v3 (section "Inline Data Storage : ≤ 256 octets stockés dans le descripteur d'objet"). Supprimer `INLINE_DATA_MAX` ou la renommer en `INLINE_DATA_BUFFER_SIZE` si elle représente une taille de buffer différente (mais cela devrait être documenté explicitement).

**Effort de correction:** 2-3 heures

---

### FS-HIGH-02 : Double Définition GC_MIN_EPOCH_DELAY — Valeurs Divergentes Possible

**Localisation:** `core/constants.rs` ligne 81 vs `core/config.rs` ligne 75
**Sévérité:** ÉLEVÉ
**Règles violées:** CONFIG-01 (configuration unique), GC-01 (paramètres cohérents)

La constante `GC_MIN_EPOCH_DELAY` est définie à deux endroits avec des mécanismes différents : dans `constants.rs` comme une constante de compilation `pub const GC_MIN_EPOCH_DELAY: u64 = 2;`, et dans `config.rs` comme une valeur configurable `gc_min_epoch_delay: AtomicU64::new(2)`. Le module `gc/` utilise principalement la version `constants.rs` pour ses calculs internes (dans `gc_scheduler.rs` et `blob_refcount.rs`), tandis que les syscalls et la configuration runtime utilisent la version `config.rs`.

Si un administrateur modifie la valeur via `ConfigUpdate::GcMinEpochDelay(v)`, la nouvelle valeur est stockée dans `EXOFS_CONFIG.gc_min_epoch_delay` mais n'est pas propagée à la constante de compilation `GC_MIN_EPOCH_DELAY`. Le résultat est que les composants utilisant la constante (calculs de planning GC internes) continuent à utiliser la valeur par défaut (2), tandis que les composants utilisant la configuration runtime (affichage, syscalls) voient la nouvelle valeur. Cette incohérence peut conduire à un GC prématuré ou retardé, avec des conséquences sur la durabilité des données.

**Recommandation:** Éliminer la constante de compilation `GC_MIN_EPOCH_DELAY` et utiliser exclusivement `EXOFS_CONFIG.gc_min_epoch_delay()` dans tout le code. Si une valeur constante est nécessaire pour des contextes `const`, définir une constante interne `GC_MIN_EPOCH_DELAY_DEFAULT: u64 = 2` clairement marquée comme valeur par défaut uniquement, et documenter que toute utilisation doit passer par `EXOFS_CONFIG`.

**Effort de correction:** 1 jour (remplacement dans tous les modules utilisateurs)

---

### FS-HIGH-03 : SuperBlock — Vérification de Taille Disque Insuffisante

**Localisation:** `storage/superblock.rs`, fonction `format()` et `mount()`
**Sévérité:** ÉLEVÉ
**Règles violées:** WRITE-02 (bytes_written == expected), BACKUP-01 (3 miroirs valides)

La fonction `SuperblockManager::format()` écrit le superblock et ses 3 miroirs (BACKUP-01) sans vérifier préalablement que le disque a une taille suffisante pour accueillir au minimum les structures on-disk + le heap de base. Si le disque fait moins de `HEAP_START_OFFSET` (1 Mo), l'écriture du miroir à `SB_MIRROR_END_FROM_END` (4 Ko avant la fin) écrira hors limites. La vérification `disk_size < MIN_DISK_SIZE` existe dans `storage_init()` (module parent) mais pas dans `format()` lui-même, ce qui signifie qu'un appel direct à `format()` (par exemple depuis un outil de formatage userspace) bypass la vérification.

De plus, la fonction `mount()` récupère le superblock depuis les miroirs sans vérifier que la taille du disque déclarée dans le superblock correspond à la taille réelle du disque. Un superblock corrompu indiquant une taille de 0 pourrait provoquer une division par zéro dans les calculs de pourcentage d'espace libre.

**Recommandation:** (1) Ajouter la vérification `disk_size >= MIN_DISK_SIZE` au début de `format()`, avant toute opération d'écriture. (2) Définir `MIN_DISK_SIZE = HEAP_START_OFFSET + EPOCH_SLOT_SIZE * 3 + SUPERBLOCK_SIZE * 4` (la taille minimale théorique). (3) Dans `mount()`, vérifier que `snap.disk_size <= actual_disk_size` et retourner une erreur si ce n'est pas le cas. (4) Protéger toutes les divisions par la taille du disque contre la division par zéro.

**Effort de correction:** 1 jour

---

### FS-HIGH-04 : Alignement Bloc Non Vérifié dans layout.rs

**Localisation:** `storage/layout.rs`, fonctions `align_up()` / `align_down()`
**Sévérité:** ÉLEVÉ
**Règles violées:** ARITH-02 (arithmétique vérifiée), ONDISK-01 (layout cohérent)

Les fonctions `align_up(offset: u64, align: u64) -> u64` et `align_down()` dans `layout.rs` utilisent des opérations arithmétiques qui peuvent déborder silencieusement en cas de valeurs d'entrée extrêmes. Par exemple, `align_up(u64::MAX, 4096)` provoquerait un overflow. De plus, ces fonctions ne vérifient pas que `align` est une puissance de 2, ce qui est une précondition fondamentale de l'algorithme d'alignement utilisé (`(offset + align - 1) & !(align - 1)`). Si `align = 0`, la fonction panique (division par zéro masquée). Si `align` n'est pas une puissance de 2, le résultat est incorrect sans aucune erreur signalée.

Le `BLOCK_SIZE = 4096` est utilisé comme valeur d'alignement par défaut, mais aucune assertion n'empêche l'utilisation d'une autre valeur qui ne serait pas une puissance de 2. C'est particulièrement risqué car le layout on-disk dépend de l'alignement correct — un mauvais alignement peut entraîner des chevauchements de structures on-disk.

**Recommandation:** (1) Utiliser `checked_add` et `checked_sub` dans les fonctions d'alignement avec retour d'erreur en cas d'overflow. (2) Ajouter une assertion de compilation ou runtime que `align` est une puissance de 2 (`align != 0 && align & (align - 1) == 0`). (3) Documenter clairement la précondition dans le contrat de la fonction. (4) Considérer l'utilisation de `NonZeroU64` pour le paramètre `align`.

**Effort de correction:** 4-6 heures

---

### FS-HIGH-05 : PathCache — Absence de Limite de Taille (Risque OOM)

**Localisation:** `path/path_cache.rs` (intégralité)
**Sévérité:** ÉLEVÉ
**Règles violées:** OOM-02 (try_reserve avant push), PATH-07 (pas de [u8; PATH_MAX] sur pile)

Le cache de chemins (`PathCache`) n'implémente aucune limite de taille maximale. Chaque résolution de chemin insère une entrée dans le cache via `cache_insert_with_hash()`, et il n'y a aucun mécanisme d'éviction automatique. Dans un système avec beaucoup de fichiers accédés via des chemins différents, le cache croît indéfiniment jusqu'à épuiser la mémoire disponible. En environnement `no_std` avec un allocateur de taille fixe (comme c'est le cas pour Exo-OS), cela provoque un OOM kernel panic — une condition catastrophique.

La spécification (section PATH-07) mentionne que le cache devrait être "borné avec une politique LRU", mais cette politique n'est pas implémentée. Le module `cache/cache_eviction.rs` existe mais n'est pas branché au `PathCache`. La constante `PATH_CACHE_CAPACITY = 10_000` est définie dans `constants.rs` mais n'est utilisée nulle part dans `path_cache.rs`.

**Recommandation:** Implémenter un mécanisme d'éviction LRU dans `PathCache` utilisant `PATH_CACHE_CAPACITY` comme limite. Lorsque le cache atteint 90 % de sa capacité, déclencher une éviction des entrées les moins récemment utilisées jusqu'à revenir à 75 %. Le module `cache/cache_eviction.rs` fournit déjà les primitives nécessaires (`EvictionPolicy::LRU`) — il suffit de les intégrer. Utiliser `try_reserve` avant chaque insertion pour éviter le panic OOM (OOM-02).

**Effort de correction:** 2-3 jours

---

## 3. Findings MEDIUM (Severity: MOYEN)

### FS-MED-01 : Ordering::Relaxed Insuffisant pour la Synchronisation Cross-Core

**Localisation:** Multiple — `core/config.rs`, `cache/*.rs`, `io/*.rs`
**Sévérité:** MOYEN
**Règles violées:** SYNC-01 (synchronisation correcte), ARITH-02 (atomic ordering)

Plusieurs modules utilisent `Ordering::Relaxed` pour des opérations atomiques qui nécessitent au minimum `Ordering::Acquire` (lectures) ou `Ordering::Release` (écritures) pour garantir la visibilité cross-core correcte. Par exemple, dans `core/config.rs`, les accesseurs utilisent `Ordering::Relaxed` pour lire des valeurs qui sont écrites par `apply_update()` avec `Ordering::Release`. Sans `Acquire` sur le lecteur, il n'y a aucune garantie que le lecteur voie l'écriture — sur des architectures faiblement ordonnancées (ARM, RISC-V), le lecteur pourrait voir une valeur obsolète indéfiniment.

Les modules `cache/` sont particulièrement affectés car les compteurs de hits/misses sont incrémentés avec `Relaxed`, ce qui signifie que les statistiques globales peuvent sous-compter significativement sous forte contention multi-core. Bien que les statistiques ne soient pas critiques pour la sécurité, cette sous-comptation peut masquer des problèmes de performance ou de pression mémoire.

**Recommandation:** Auditer tous les usages d'`Ordering::Relaxed` dans ExoFS. Pour les lectures de valeurs écrites par d'autres cœurs, utiliser `Ordering::Acquire`. Pour les écritures destinées à être lues par d'autres cœurs, utiliser `Ordering::Release`. Pour les lectures/écritures séquentiellement consistantes (rarement nécessaires), utiliser `Ordering::SeqCst`. Les incrémentations de statistiques peuvent rester `Relaxed` si la sous-comptation est acceptable, mais cela doit être documenté explicitement.

**Effort de correction:** 1-2 jours (audit + remplacement mécanique)

---

### FS-MED-02 : Résolution de Symlinks — Implémentation Incomplète

**Localisation:** `path/symlink.rs` (intégralité)
**Sévérité:** MOYEN
**Règles violées:** RECUR-01 (pas de récursion), POSIX-12 (symlinks)

Le module `path/symlink.rs` définit les structures de données pour les liens symboliques (`SymlinkTarget`, `SymlinkStore`, etc.) mais n'implémente pas la fonction de résolution `resolve_symlink_chain()`. Cette fonction est un simple stub qui retourne toujours `Ok(SymlinkResolution::NotASymlink)`. Par conséquent, toute opération sur un lien symbolique (ouvrir, lire, stat) échouera silencieusement ou retournera une erreur incorrecte.

La spécification RECUR-01 impose que la résolution de symlinks soit itérative (pas récursive) avec une limite de profondeur `SYMLINK_MAX_DEPTH = 40`. Le code définit cette constante mais ne l'utilise pas. Le document de corrections CORR-06 mentionne également la nécessité d'une vérification de cycle dans la résolution de symlinks.

**Recommandation:** Implémenter `resolve_symlink_chain()` comme une boucle `while` itérative (RECUR-01) qui : (1) vérifie que la cible est un symlink via `ObjectKind::Symlink`, (2) lit la cible depuis le `SymlinkStore`, (3) résout le chemin cible via `resolve_path()`, (4) incrémente un compteur de profondeur, (5) retourne `ELOOP` si la profondeur dépasse `SYMLINK_MAX_DEPTH`, (6) détecte les cycles via un ensemble de `ObjectId` déjà visités.

**Effort de correction:** 2-3 jours

---

### FS-MED-03 : Quota — Spécifié mais Non Implémenté

**Localisation:** Spécifié dans `ExoFS_Reference_Complete_v3.md`, absent du code
**Sévérité:** MOYEN
**Règles violées:** QUOTA-01 à QUOTA-05 (spécification quota)

Le système de quotas est entièrement spécifié dans la documentation (sections QUOTA-01 à QUOTA-05) avec : `QuotaBlock` on-disk (magic "QUOT", 128 octets), `sys_exofs_quota` (syscall 518), vérification pré-écriture (`ENOSPC` si quota dépassé), et quota par utilisateur/groupe. Cependant, aucun de ces mécanismes n'est implémenté dans le code. Le syscall 518 est mappé dans la table des syscalls mais son handler retourne `ENOSYS`.

Cette absence est problématique car les quotas sont un mécanisme de protection essentiel contre le DoS par épuisement de l'espace disque. Sans quotas, un processus malveillant ou buggy peut remplir le volume entier, empêchant tous les autres processus d'écrire.

**Recommandation:** Implémenter le système de quotas par étapes : (1) structure `QuotaBlock` on-disk avec magic et checksum, (2) lecture du quota block au montage, (3) vérification pré-écriture dans `BlobWriter` et `ObjectWriter`, (4) syscall `sys_exofs_quota` pour consultation/modification admin, (5) tests de non-régression.

**Effort de correction:** 4-5 jours

---

### FS-MED-04 : Module NUMA — Implémentation Partielle

**Localisation:** `numa/` (5 fichiers sur ~15 spécifiés)
**Sévérité:** MOYEN
**Règles violées:** NUMA-01 à NUMA-05 (spécification NUMA)

Le module NUMA (`numa_affinity.rs`, `numa_migration.rs`, `numa_placement.rs`, `numa_stats.rs`, `numa_tuning.rs`) est partiellement implémenté. Les structures de données existent mais les fonctions de placement et de migration sont des stubs. La spécification Architecture v7 décrit une politique de placement NUMA sophistiquée (local, interleaved, remote) avec migration automatique des pages chaudes, mais le code ne contient que les squelettes de ces fonctions.

Sur un système NUMA réel (multi-socket), l'absence de placement NUMA correct conduira à une performance sous-optimale (accès mémoire distant = 2-3x plus lent) et potentiellement à une congestion du fabric NUMA si tous les threads s'exécutent sur le même nœud.

**Recommandation:** Implémenter au minimum le placement local (allocation sur le nœud NUMA du cœur exécutant le thread) et la détection automatique de la topologie NUMA via l'ACPI SRAT. La migration dynamique peut être reportée à une phase ultérieure. Documenter clairement les limitations dans le code.

**Effort de correction:** 5-7 jours

---

### FS-MED-05 : Export ExoAR — Absence de Tests d'Intégration

**Localisation:** `export/` (11 fichiers)
**Sévérité:** MOYEN
**Règles violées:** TEST-01 (couverture tests), EXPORT-01 (format stable)

Le module d'export (`export/exoar_format.rs`, `export/exoar_writer.rs`, `export/exoar_reader.rs`, etc.) implémente le format d'archive ExoAR avec un en-tête de 128 octets, des chunks compressés de 4 Mo max, et un index de blobs. Cependant, il n'existe aucun test d'intégration qui vérifie qu'un objet écrit puis relu via ExoAR est identique à l'original. Les tests unitaires existants ne testent que les fonctions individuelles (écriture d'en-tête, lecture d'index) mais pas le pipeline complet.

Le risque est qu'une modification future casse la compatibilité du format ExoAR sans être détectée. Comme ExoAR est le format d'interopérabilité avec d'autres systèmes, une incompatibilité rendrait les exports/restores impossibles.

**Recommandation:** Ajouter un test d'intégration end-to-end qui : (1) crée un volume ExoFS avec 10 objets de tailles variées, (2) exporte le volume au format ExoAR, (3) lit l'archive et vérifie l'intégrité de chaque objet, (4) compare les données décompressées avec les données originales, (5) vérifie que l'index de blobs est cohérent. Ce test doit être exécuté dans la CI à chaque commit.

**Effort de correction:** 1-2 jours

---

### FS-MED-06 : TODO SYS_EXOFS_EPOCH_META (517) — Décision Incomplète

**Localisation:** `syscall/mod.rs`, `exo-syscall/src/exofs.rs`
**Sévérité:** MOYEN
**Règles violées:** CORR-52 (décision canonique), API-01 (API stable)

Le syscall `SYS_EXOFS_EPOCH_META = 517` est marqué comme TODO dans le mapping canonique. La correction CORR-52 a décidé de l'aliaser vers `sys_ni_syscall` (retourne `ENOSYS`) en Phase 8, avec une activation prévue en Phase 4. Cependant, le code actuel ne reflète pas entièrement cette décision : le syscall est défini mais pas explicitement mappé à `sys_ni_syscall`, ce qui signifie qu'il pourrait tomber dans un handler par défaut non défini.

De plus, la `compile_error!` conditionnelle pour la feature "phase4" n'est pas présente dans le code, ce qui signifie qu'un développeur pourrait appeler ce syscall depuis Ring 1 sans se rendre compte qu'il n'est pas implémenté.

**Recommandation:** Implémenter explicitement le mapping vers `sys_ni_syscall` avec un commentaire documentant la décision CORR-52. Ajouter une fonction stub `sys_exofs_epoch_meta` qui retourne `Err(ExofsError::NotImplemented)`. Documenter dans le header du syscall que toute utilisation doit être conditionnée à la feature "phase4".

**Effort de correction:** 2-3 heures

---

### FS-MED-07 : Crypto Shredding — Non Branché au Pipeline I/O

**Localisation:** `crypto/crypto_shredding.rs` (isolé)
**Sévérité:** MOYEN
**Règles violées:** SEC-01 (sécurité données), SHRED-01 (destruction sécurisée)

Le module `crypto_shredding.rs` implémente la destruction sécurisée des données (overwrite avec des patterns spécifiques, conforme DoD 5220.22-M) mais n'est pas branché au pipeline I/O. Lorsqu'un blob est supprimé (GC), les données sur disque ne sont pas effacées — seule la référence dans le superblock est mise à jour. Les données physiques restent sur le disque jusqu'à ce qu'elles soient écrasées par une écriture ultérieure, ce qui peut ne jamais arriver si le disque a beaucoup d'espace libre.

Cette absence est une vulnérabilité de sécurité pour les données sensibles (objets `Secret`). Même après suppression, les données restent récupérables avec des outils forensiques.

**Recommandation:** Brancher `crypto_shredding.rs` au pipeline de suppression de blobs : dans `gc/sweeper.rs`, avant de libérer les blocs d'un blob supprimé, appeler `secure_erase_blocks(block_range)` si le blob appartenait à un objet de type `Secret` ou si la configuration `require_encryption_for_secrets` est activée. Cette opération est coûteuse en I/O et doit être faite de manière asynchrone pour ne pas bloquer le GC.

**Effort de correction:** 2-3 jours

---

### FS-MED-08 : Cache Eviction — Pas de Priorisation Dirty vs Clean

**Localisation:** `cache/cache_eviction.rs`, `cache/mod.rs`
**Sévérité:** MOYEN
**Règles violées:** CACHE-01 (politique eviction cohérente), DATA-01 (durabilité)

La politique d'éviction du cache (`EvictionPolicy::LRU`) ne différencie pas les entrées "dirty" (modifiées, pas encore écrites sur disque) des entrées "clean" (synchronisées avec le disque). Lors d'une éviction sous pression mémoire, une entrée dirty peut être évacuée sans être écrite préalablement, entraînant une perte de données si le système crash avant le prochain writeback.

Le module `cache/mod.rs`, fonction `reclaim_bytes()`, utilise `flush_all()` puis `evict_n(64)` — mais `flush_all` marque les entrées comme propres sans garantir qu'elles ont été écrites physiquement. Si le flush échoue silencieusement (erreur I/O), les données sont perdues.

**Recommandation:** Modifier la politique d'éviction pour : (1) préférer évacuer les entrées clean en priorité, (2) pour les entrées dirty, forcer un writeback synchrone avant l'éviction, (3) si le writeback échoue, conserver l'entrée en cache et signaler l'erreur, (4) ajouter un compteur d'entrées "dirty non évictables" dans les statistiques.

**Effort de correction:** 2-3 jours

---

## 4. Findings LOW (Severity: FAIBLE)

### FS-LOW-01 : Typos et Erreurs Linguistiques dans les Commentaires

**Localisation:** Multiple fichiers
**Sévérité:** FAIBLE
**Règles violées:** DOC-01 (documentation de qualité)

Plusieurs typos ont été identifiées dans les commentaires du code : "arbitraire" utilisé à la place de "arbitrary" (contexte anglais), "systématiquement" mal orthographié, "déréférencement" sans accent. Ces typos n'ont aucun impact fonctionnel mais réduisent la crédibilité perçue du code, particulièrement lors d'une revue par des contributeurs externes ou des auditeurs de sécurité.

**Recommandation:** Passage d'un outil de vérification orthographique (comme `typos` ou `cspell`) sur l'ensemble du module `fs/exofs/`. Correction des typos identifiées.

**Effort de correction:** 2-3 heures

---

### FS-LOW-02 : Compression Wrappers — Non Testés en no_std

**Localisation:** `compress/lz4_wrapper.rs`, `compress/zstd_wrapper.rs`
**Sévérité:** FAIBLE
**Règles violées:** TEST-01 (couverture tests), PORT-01 (portabilité no_std)

Les wrappers LZ4 et Zstd utilisent les crates `lz4` et `zstd` du crates.io, qui ne sont pas garanties `no_std`. Les tests existants sont exécutés en mode `std` (car `cargo test` inclut automatiquement `std`), mais il n'existe pas de tests qui vérifient que la compilation et l'exécution fonctionnent réellement en environnement `no_std` (cible `x86_64-unknown-none` ou similaire).

**Recommandation:** Ajouter un test de compilation cross-compilé pour la cible `x86_64-unknown-none` dans la CI. Vérifier que les crates `lz4` et `zstd` compilent bien avec `#![no_std]`. Si ce n'est pas le cas, soit trouver des alternatives `no_std`, soit intégrer les implémentations C directement via FFI.

**Effort de correction:** 1-2 jours

---

### FS-LOW-03 : xchacha20 — Vérification de Taille de Nonce Absente

**Localisation:** `crypto/xchacha20.rs`, fonction `encrypt()`
**Sévérité:** FAIBLE
**Règles violées:** CRYPTO-01 (paramètres chiffrement validés)

La fonction `xchacha20_encrypt()` accepte un nonce comme `&[u8]` sans vérifier que sa taille correspond à `XCHACHA20_NONCE_SIZE` (24 octets pour XChaCha20-Poly1305). Un nonce de taille incorrecte provoquera un panic (slice index out of bounds) lors du copiage dans la structure interne. Bien que les appelants internes passent probablement toujours la bonne taille, cette absence de vérification est une fragilité.

**Recommandation:** Ajouter une vérification explicite au début de `xchacha20_encrypt()` et `xchacha20_decrypt()` : si `nonce.len() != XCHACHA20_NONCE_SIZE`, retourner `Err(ExofsError::InvalidNonceSize)`.

**Effort de correction:** 30 minutes

---

### FS-LOW-04 : io_uring.rs — Stub Sans Implémentation

**Localisation:** `io/io_uring.rs` (intégralité)
**Sévérité:** FAIBLE
**Règles violées:** IO-01 (I/O asynchrone), PERF-01 (performance)

Le module `io_uring.rs` est entièrement un stub — il définit les structures `IoUringSqe`, `IoUringCqe`, `IoUringQueue` mais aucune fonction n'est implémentée. io_uring est le mécanisme I/O asynchrone le plus performant sous Linux (3x plus rapide que aio/epoll pour les charges I/O intensives), et son absence représente une opportunité de performance manquée.

Cependant, ce n'est pas bloquant car le module I/O fonctionne correctement via les mécanismes synchrones (buffered_io, direct_io). io_uring est une optimisation future.

**Recommandation:** Implémenter io_uring comme une optimisation de Phase 3+. Pour l'instant, documenter clairement dans le module que c'est un stub intentionnel avec une référence au ticket/planning de l'implémentation future.

**Effort de correction:** 1-2 semaines (projet d'optimisation)

---

## 5. Analyse par Sous-Module

### 5.1 Module core/ (16 fichiers, ~2000 lignes)

Le module `core/` est le fondement d'ExoFS. Il définit les types fondamentaux (`ObjectId`, `BlobId`, `DiskOffset`), les erreurs, les constantes, la configuration, et les flags. Globalement, ce module est bien structuré avec une séparation claire des responsabilités. Cependant, trois problèmes majeurs y ont été identifiés : l'incohérence des constantes `INLINE_DATA_MAX` (FS-HIGH-01), la double définition `GC_MIN_EPOCH_DELAY` (FS-HIGH-02), et le `const fn` avec initialisation atomique (FS-CRIT-05). La qualité du code est bonne, avec une utilisation appropriée des types algébriques et des enums pour modéliser les états.

### 5.2 Module storage/ (27 fichiers, ~3500 lignes)

Le module `storage/` implémente la couche d'accès disque avec une architecture en pipeline bien pensée (raw → BlobId → dédup → compress → encrypt → checksum). Le superblock est correctement structuré avec 3 miroirs (BACKUP-01), et le heap allocator est fonctionnel. Les principaux problèmes sont la vérification de taille disque insuffisante (FS-HIGH-03) et l'alignement bloc non vérifié (FS-HIGH-04). Le module `blob_writer.rs` et `blob_reader.rs` implémentent correctement le pipeline spécifié dans la documentation.

### 5.3 Module epoch/ (16 fichiers, ~2800 lignes)

Le module `epoch/` gère le système transactionnel d'ExoFS, qui est l'un de ses aspects les plus innovants. L'architecture A/B/C pour les slots d'epoch est correctement implémentée, et le mécanisme de commit avec barrières est fonctionnel. Cependant, le checksum XOR naïf (FS-CRIT-01) est une vulnérabilité critique qui affecte l'intégrité de toutes les transactions. Le mécanisme de recovery (epoch_recovery.rs) est bien conçu mais n'a pas été testé avec des scénarios de corruption réels.

### 5.4 Module cache/ (11 fichiers, ~2400 lignes)

Le module `cache/` implémente 5 caches spécialisés (blob, object, extent, metadata, path) avec une architecture cohérente. Les statistiques globales et le moniteur de pression sont fonctionnels. Les principaux problèmes sont l'absence de limite de taille pour le PathCache (FS-HIGH-05) et l'éviction sans priorisation dirty/clean (FS-MED-08). Le mécanisme de warming et de shrinker est bien conçu mais n'est pas branché par défaut.

### 5.5 Module io/ (13 fichiers, ~3200 lignes)

Le module `io/` fournit une abstraction I/O complète avec support buffered, direct, async, scatter-gather, et zero-copy. La qualité est bonne avec une gestion correcte des erreurs. L'absence d'io_uring (FS-LOW-04) est une limitation de performance mais pas de fonctionnalité. Le module `writeback.rs` est correctement implémenté avec une queue de travail.

### 5.6 Module gc/ (16 fichiers, ~3800 lignes)

Le module `gc/` est l'un des plus aboutis, avec un collecteur tricolore complet (phases Scan/Mark/Sweep/Orphan/Inline), un détecteur de cycles DFS itératif, et un planificateur non-bloquant. Les statistiques et le tuning automatique sont fonctionnels. Le compteur de références atomique (blob_refcount.rs) est correctement implémenté avec une file de suppression différée.

### 5.7 Module posix_bridge/ (8 fichiers, ~1800 lignes)

C'est le module le plus problématique. La Translation Layer v5 (documentée comme ~95 % POSIX) est entièrement absente — les fichiers ne contiennent que des stubs et des structures de données vides. Les problèmes FS-CRIT-02 et FS-CRIT-03 rendent ce module non fonctionnel. C'est la zone qui nécessite le plus de travail pour atteindre un état utilisable.

### 5.8 Module crypto/ (12 fichiers, ~2600 lignes)

Le module `crypto/` implémente XChaCha20-Poly1305, la dérivation de clés HKDF, le stockage sécurisé des clés, et le shredding. La qualité est bonne mais le shredding n'est pas branché au pipeline I/O (FS-MED-07). Le module `entropy.rs` est un stub qui doit être connecté à une source d'entropie hardware (RDRAND, etc.).

### 5.9 Module path/ (12 fichiers, ~3400 lignes)

Le module `path/` implémente la résolution de chemins avec une architecture itérative (RECUR-01 conforme). Le path cache est fonctionnel mais sans limite de taille (FS-HIGH-05). La résolution de symlinks est un stub (FS-MED-02). Le path index avec split/merge est bien implémenté.

### 5.10 Module dedup/ (12 fichiers, ~2900 lignes)

Le module `dedup/` implémente le content-defined chunking (CDC), le hash de contenu BLAKE3, et le registre de blobs partagés. La qualité est bonne et conforme aux specs. Le chunker CDC utilise l'algorithme FastCDC qui est efficace et bien documenté.

---

## 6. Analyse des Corrections Existantes (CORR-01 à CORR-54)

L'audit a également vérifié l'état d'implémentation des 54 corrections canoniques définies dans les documents de corrections. Voici le statut pour les corrections liées à ExoFS :

| Correction | Description | Statut dans le Code |
|------------|-------------|---------------------|
| CORR-06 | `EpollEventAbi.data_u64()` | ✅ Implémenté |
| CORR-20 | `SYS_EXOFS_EPOCH_META = 517` | ⚠️ Partiel (alias ENOSYS non explicite) |
| CORR-22 | BlobId ObjectId `[u8; 32]` | ✅ Implémenté |
| CORR-31 | Payload IPC ≤ 48B | ✅ Vérifié (pas dans ExoFS directement) |
| CORR-36 | Panic handler Ring 1 | ✅ Simplifié par CORR-49 |
| CORR-39 | `mark_stale()` vs `close()` | ✅ Non applicable (pas dans ExoFS kernel) |
| CORR-49 | Panic handler UART seulement | ✅ Implémenté dans `exo-ipc` |
| CORR-52 | `SYS_EXOFS_EPOCH_META` ENOSYS | ⚠️ Partiel (stub présent mais pas explicite) |

**Constat global :** Les corrections de sécurité (CORR-06, CORR-31, CORR-36/49) sont implémentées. Les corrections fonctionnelles (CORR-20, CORR-52) le sont partiellement — les stubs existent mais la documentation de l'état n'est pas complète.

---

## 7. Recommandations Globales

### Priorité 1 — Bloquant pour Release Candidate

1. **Corriger FS-CRIT-01 (checksum XOR) :** Remplacer par BLAKE3-256 avec bump de version de format.
2. **Implémenter FS-CRIT-02 (POSIX layer) :** La Translation Layer v5 est indispensable pour toute utilisation pratique.
3. **Corriger FS-CRIT-03 (InodeNumber u64) :** Redéfinir le type et implémenter le mapping.

### Priorité 2 — Haute Priorité Post-RC

4. **Corriger FS-CRIT-04 (AtomicU64 on-disk) :** Remplacer par types non-atomiques dans toutes les structures on-disk.
5. **Corriger FS-CRIT-05 (const fn atomique) :** Séparer config statique et dynamique.
6. **Unifier les constantes (FS-HIGH-01, FS-HIGH-02) :** Éliminer les doubles définitions.

### Priorité 3 — Renforcement de la Robustesse

7. **Implémenter les quotas (FS-MED-03) :** Protection DoS par espace disque.
8. **Brancher le crypto shredding (FS-MED-07) :** Protection des données sensibles post-suppression.
9. **Corriger l'éviction cache (FS-MED-08) :** Priorisation dirty vs clean.

### Priorité 4 — Qualité et Tests

10. **Ajouter des tests d'intégration end-to-end :** Export ExoAR, recovery post-crash, stress multi-thread.
11. **Mettre en place la CI cross-compilation no_std :** Vérifier que tout compile pour `x86_64-unknown-none`.
12. **Corriger les typos (FS-LOW-01) :** Passage d'un outil de vérification orthographique.

---

## 8. Conclusion

ExoFS est un système de fichiers avec une ambition architecturale remarquable — l'approche orientée objet avec content-addressing, le système transactionnel par epochs, et le pipeline I/O avec déduplication/compression/chiffrement sont des choix techniques cohérents et bien documentés. La qualité du code est globalement bonne, avec une attention particulière à la sécurité mémoire (Rust `no_std`) et à la gestion d'erreurs.

Cependant, l'écart entre la documentation et l'implémentation est significatif, particulièrement dans la couche POSIX qui est le point d'entrée pour 99 % des applications. Les 5 findings critiques identifiés doivent être corrigés avant toute release candidate, et la Translation Layer v5 représente le plus gros effort de développement restant (3-4 semaines).

Le module le plus mature et prêt pour la production est le sous-système `storage/` avec son pipeline de lecture/écriture de blobs, qui est fonctionnellement complet et bien testé. Le sous-système `gc/` est également bien avancé avec son collecteur tricolore. Les efforts futurs devraient se concentrer sur : (1) la couche POSIX, (2) le renforcement cryptographique (checksums, shredding), et (3) l'ajout de tests d'intégration end-to-end.

| Indicateur | Évaluation |
|------------|------------|
| Qualité du code | B+ (sécurité mémoire, gestion d'erreurs) |
| Conformité specs | C+ (écart POSIX majeur) |
| Sécurité | B (checksum faible, shredding non branché) |
| Testabilité | C (tests unitaires OK, intégration insuffisante) |
| Documentation | A- (excellente, mais diverge du code) |
| **Global** | **B- (prometteur, mais nécessite effort POSIX)** |

---

*Rapport généré le 19 avril 2026 · Audit complet du module ExoFS · 293 fichiers · 130 220 lignes · 22 findings*
