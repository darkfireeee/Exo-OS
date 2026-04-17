Voici la transcription fidèle du document en Markdown :



---



\# ExoFS — Référence Complète pour Copilot / IA



\## Règles absolues · Arborescences · Driver Ring 0/1 · Erreurs silencieuses



\*\*Exo-OS · fs/exofs/ · Kernel Ring 0 · no\_std · Rust · v2.0\*\*



⛔ \*\*CE DOCUMENT EST LA LOI.\*\* Toute règle ❌ INTERDIT est une violation critique pouvant corrompre le kernel, les données disque, ou créer une faille de sécurité. Aucune exception n'est tolérée.



---



\## 1. Contexte, Position et Décision Ring 0 / Ring 1



\### 1.1 Ce qu'est ExoFS



ExoFS est le système de fichiers natif d'Exo-OS. Il remplace ext4plus. Il tourne en Ring 0 (kernel, no\_std). Il implémente les traits VFS définis dans `fs/core/vfs.rs`. Il utilise un modèle d'objets typés + capabilities cryptographiques au lieu d'inodes POSIX classiques.



\### 1.2 DAG des dépendances — RÈGLE ABSOLUE



\*\*RÈGLE DAG-01 :\*\* `fs/exofs/` ne peut dépendre QUE de `memory/`, `scheduler/`, `security/capability/`. Tout autre import direct est une violation qui crée des dépendances circulaires.



```

fs/exofs/ dépend de :

├── memory/ → alloc\_frame(), DMA, CoW, NUMA

├── scheduler/ → SpinLock, RwLock, WaitQueue, timers

├── security/ → verify\_cap(), CapToken

│

├── INTERDIT : ipc/ (utiliser trait abstrait + injection au boot)

├── INTERDIT : process/ (lecture seule via trait, jamais import direct)

└── INTERDIT : arch/ (accès NVMe via block layer uniquement)

```



\### 1.3 Décision Driver — Ring 0 vs Ring 1



\*\*DÉCISION FONDAMENTALE :\*\* Le driver ExoFS est physiquement scindé. Règle : « Si le crash corrompt des données → Ring 0. Si le crash se gère par redémarrage → Ring 1 ».



| Ring 0 — fs/exofs/ (MÉCANISMES) | Ring 1 — servers/posix\_server/ (POLITIQUE) |

|--------------------------------|-------------------------------------------|

| ✅ Superblock, Epochs, Heap disque | ✅ Traduction path string → ObjectId |

| ✅ Page cache, writeback thread | ✅ stat() → struct stat POSIX |

| ✅ CapToken verification | ✅ readdir() → struct dirent |

| ✅ Commit Epoch (3 barrières NVMe) | ✅ Permission mapping rwx → CapToken |

| ✅ GC thread (tricolore) | ✅ errno mapping ExofsError → POSIX |

| ✅ PathIndex (structure kernel) | ✅ NFS v3/v4 server |

| ✅ Syscalls ExoFS 500-518 | ✅ exofs-mkfs, exofs-fsck (outils) |

| ✅ inode\_emulation (ObjectId→ino\_t VFS) | |

| ✅ mmap + CoW (page table kernel) | |

| ✅ fcntl locks (mécanisme kernel) | |

| ✅ Recovery au boot | |

| ❌ JAMAIS : toucher au disque directement | |

| ❌ JAMAIS : modifier CapTokens kernel | |

| ❌ JAMAIS : tenir un lock kernel pendant I/O réseau | |



⚠️ \*\*ERREUR SPEC Z-AI :\*\* `posix/` est placé entièrement dans Ring 0. \*\*INCORRECT.\*\* Scindé en `posix\_bridge/` (Ring 0, 5 fichiers) + `servers/posix\_server/` (Ring 1). Voir arborescence Section 2.



⚠️ \*\*ERREUR SPEC Z-AI :\*\* `AtomicU64` dans `ExoSuperblock` on-disk. \*\*INCORRECT.\*\* AtomicU64 = bit pattern non-déterministe = checksum Blake3 invalide à chaque boot. Correction : `ExoSuperblockDisk` (plain u64) + `ExoSuperblockInMemory` (AtomicU64). Voir Section 5.



---



\## 2. Arborescence Complète — Z-AI v1.0 Corrigée



\*\*Format :\*\* chaque fichier sur sa propre ligne, indentation `│ ├── └──`, commentaire `#` aligné. Créer tous les fichiers vides immédiatement (structure + `mod.rs`), puis implémenter le contenu dans l'ordre de la Phase 1.



\### 2.0 Racine du module



```

kernel/src/fs/exofs/

│

├── mod.rs                 # API publique : exofs\_init(), exofs\_register\_fs()

├── lib.rs                 # Feature flags : #!\[feature(allocator\_api)]

│

├── core/                  # ★ Types fondamentaux — ZÉRO dépendance externe

├── objects/               # Modèles L-Obj et P-Blob

├── path/                  # Résolution chemins, PathIndex

├── epoch/                 # Commits atomiques, recovery

├── storage/               # Stockage disque, heap, superblock

├── gc/                    # Garbage collection tricolore

├── dedup/                 # Déduplication content-aware

├── compress/              # Compression LZ4/Zstd inline

├── crypto/                # Chiffrement XChaCha20 pour Secrets

├── snapshot/              # Snapshots natifs (epoch-as-snapshot)

├── relation/              # Graphe de relations typées

├── quota/                 # Quotas capability-bound

├── syscall/               # Syscalls 500-518

├── posix\_bridge/          # ★ AJOUT : pont VFS Ring 0 (corrige posix/ Z-AI)

├── io/                    # Opérations I/O (read/write/zero-copy)

├── cache/                 # Caches (object/blob/path/extent)

├── recovery/              # Recovery boot + fsck 4 phases

├── export/                # Export/Import format EXOAR

├── numa/                  # NUMA awareness

├── observability/         # Métriques, tracing, health

├── audit/                 # Audit trail ring buffer

└── tests/                 # Tests unitaires, intégration, fuzz

```



\### 2.1 core/ — Types Fondamentaux (14 fichiers)



```

kernel/src/fs/exofs/core/

│

├── mod.rs                 # Re-exports pub de tous les types

├── types.rs               # ObjectId=\[u8;32], BlobId=\[u8;32], EpochId=u64

&nbsp;                         # SnapshotId=u64, DiskOffset=u64, Extent{offset,len}

├── constants.rs           # MAGIC=0x45584F46, SLOT\_A=4KB, SLOT\_B=8KB

&nbsp;                         # HEAP\_START=1MB, EPOCH\_MAX\_OBJECTS=500

├── error.rs               # ExofsError enum → impl From<ExofsError> for FsError

├── config.rs              # Configuration boot-time (tailles caches, seuils GC)

├── object\_id.rs           # new\_class1(), new\_class2(), ct\_eq() temps constant

├── blob\_id.rs             # Blake3 wrapper — JAMAIS sur données compressées

├── epoch\_id.rs            # Monotonic counter, comparison, wrapping check

├── object\_kind.rs         # #\[repr(u8)] enum ObjectKind

&nbsp;                         # { Blob=0, Code=1, Config=2, Secret=3,

&nbsp;                         # PathIndex=4, Relation=5 }

├── object\_class.rs        # ObjectClass::Class1 (immutable) vs Class2 (CoW)

&nbsp;                         # + promotion logic Class1→Class2

├── rights.rs              # INSPECT\_CONTENT=1<<10, SNAPSHOT\_CREATE=1<<11

&nbsp;                         # RELATION\_CREATE=1<<12, GC\_TRIGGER=1<<13

├── flags.rs               # ObjectFlags, ExtentFlags, EpochFlags bitfields

├── version.rs             # Format version, compatibility check

└── stats.rs               # Compteurs AtomicU64 — PAS dans structs on-disk

```



\### 2.2 objects/ — Modèles d'Objets (17 fichiers)



```

kernel/src/fs/exofs/objects/

│

├── mod.rs                 # Registry objects, re-exports

├── logical\_object.rs      # LogicalObject #\[repr(C, align(64))]

&nbsp;                         # Cache line 1 (64B, hot path) :

&nbsp;                         # id:ObjectId\[32], kind:ObjectKind\[1]

&nbsp;                         # class:ObjectClass\[1], flags:ObjectFlags\[2]

&nbsp;                         # link\_count:AtomicU32\[4], epoch\_last:AtomicU64\[8]

&nbsp;                         # ref\_count:AtomicU32\[4], \_pad:\[u8;12]

&nbsp;                         # Cache line 2+ : meta, physical\_ref, extents

├── physical\_blob.rs       # PhysicalBlob — SÉPARER disk / in-memory

&nbsp;                         # PhysicalBlobDisk #\[repr(C)] : plain u64

&nbsp;                         # PhysicalBlobInMemory : AtomicU32 ref\_count

├── physical\_ref.rs        # enum PhysicalRef {

&nbsp;                         # Unique { blob\_id: BlobId },

&nbsp;                         # Shared { blob\_id: BlobId, share\_idx: u32,

&nbsp;                         # is\_writer: bool },

&nbsp;                         # Inline { data: \[u8;512], len: u16,

&nbsp;                         # checksum: u32 } // CRC32

&nbsp;                         # }

├── object\_meta.rs         # ObjectMeta : timestamps, mime\_type:\[u8;64], owner\_cap

├── object\_kind/

│   ├── mod.rs

│   ├── blob.rs            # ObjectKind::Blob — données génériques

│   ├── code.rs            # ObjectKind::Code — validation ELF avant exec

│   ├── config.rs          # ObjectKind::Config — validation schéma

│   ├── secret.rs          # ObjectKind::Secret — BlobId JAMAIS exposé

│   ├── path\_index.rs      # ObjectKind::PathIndex — toujours Class2

│   └── relation.rs        # ObjectKind::Relation — lien typé entre objets

├── extent.rs              # Extent { offset:DiskOffset, len:u64 }

├── extent\_tree.rs         # B+ tree pour extents — read/insert/delete

├── inline\_data.rs         # Stockage inline < 512B dans le L-Obj

├── object\_builder.rs      # Builder pattern — valide les invariants à la création

├── object\_loader.rs       # Lazy loading depuis disque

&nbsp;                         # Vérifie ObjectHeader magic+checksum AVANT payload

└── object\_cache.rs        # LRU cache objets chauds

&nbsp;                         # PASSE PAR ObjectTable (règle CACHE-02)

```



\### 2.3 path/ — Résolution de Chemins (13 fichiers)



```

kernel/src/fs/exofs/path/

│

├── mod.rs                 # API résolution chemins

├── resolver.rs            # resolve\_path(path:\&\[u8]) → ObjectId

&nbsp;                         # Buffer per-CPU PATH\_BUFFERS — PAS \[u8;4096] stack

&nbsp;                         # Itératif, jamais récursif (règle RECUR-01)

├── path\_index.rs          # PathIndex — un par répertoire, toujours Class2

&nbsp;                         # On-disk : sorted array (hash, ObjectId, name\_len)

&nbsp;                         # In-memory: radix tree pour lookup O(log n)

&nbsp;                         #

&nbsp;                         # PathIndexEntry #\[repr(C, packed)] :

&nbsp;                         # hash:u64, object\_id:ObjectId\[32]

&nbsp;                         # name\_len:u16, kind:ObjectKind\[1]

&nbsp;                         #

&nbsp;                         # SplitInfo #\[repr(C)] :

&nbsp;                         # low\_child:ObjectId, high\_child:ObjectId

&nbsp;                         # threshold:u32

├── path\_index\_tree.rs     # Radix tree in-memory

├── path\_index\_split.rs    # Split atomique — UN SEUL EpochRoot (règle SPLIT-02)

├── path\_index\_merge.rs    # Merge après suppressions (seuil < 4096 entrées)

├── path\_component.rs      # Parsing composant : UTF-8, len≤255, pas '/'

├── symlink.rs             # Résolution symlink : MAX\_DEPTH=40, itératif

├── mount\_point.rs         # Intégration MOUNT\_TABLE du VFS existant

├── namespace.rs           # Path namespaces pour containers

├── canonicalize.rs        # Normalisation /../ et /./ — buffer in-place

├── path\_cache.rs          # Dentry cache LRU 10 000 entrées

&nbsp;                         # Clé : (parent\_oid, name\_hash)

└── path\_walker.rs         # Iterator-based walking — évite récursion

```



\### 2.4 epoch/ — Gestion des Epochs (17 fichiers)



```

kernel/src/fs/exofs/epoch/

│

├── mod.rs                 # Epoch management API

├── epoch\_id.rs            # EpochId : monotonic, wrapping check, comparison

├── epoch\_record.rs        # EpochRecord #\[repr(C, packed)] — EXACTEMENT 104 bytes

&nbsp;                         # magic:u32\[4] = 0x45584F46

&nbsp;                         # version:u16\[2]

&nbsp;                         # flags:u16\[2]

&nbsp;                         # epoch\_id:EpochId\[8] monotone croissant

&nbsp;                         # timestamp:u64\[8] TSC au commit

&nbsp;                         # root\_oid:ObjectId\[32] ObjectId de l'EpochRoot

&nbsp;                         # root\_offset:u64\[8] offset disque EpochRoot

&nbsp;                         # prev\_slot:u64\[8] offset slot précédent

&nbsp;                         # checksum:\[u8;32] Blake3(tout ce qui précède)

&nbsp;                         # const \_: () = assert!(size\_of::<EpochRecord>()==104)

├── epoch\_root.rs          # EpochRoot — variable, chainable, multi-pages

&nbsp;                         # magic:u32 = 0x45504F43 ("EPOC")

&nbsp;                         # epoch\_id:EpochId

&nbsp;                         # modified\_objects: liste (ObjectId, DiskOffset)

&nbsp;                         # deleted\_objects: liste ObjectId

&nbsp;                         # new\_relations: liste RelationDelta

&nbsp;                         # next\_page:Option<DiskOffset> inclus dans checksum

&nbsp;                         # checksum:\[u8;32] Blake3 de cette page seule

&nbsp;                         # ★ CHAQUE page chainée a son propre magic+checksum

├── epoch\_root\_chain.rs    # Chainement multi-pages — next\_page inclus dans checksum

├── epoch\_slots.rs         # Slots A/B/C aux offsets FIXES

&nbsp;                         # Slot A : offset 4KB

&nbsp;                         # Slot B : offset 8KB

&nbsp;                         # Slot C : offset disk\_size - 4MB

&nbsp;                         # FrameFlags::EPOCH\_PINNED sur slot inactif

├── epoch\_commit.rs        # Protocole 3 barrières NVMe OBLIGATOIRE

&nbsp;                         # Phase 1 : write(payload) → nvme\_flush()

&nbsp;                         # Phase 2 : write(EpochRoot) → nvme\_flush()

&nbsp;                         # Phase 3 : write(EpochRecord→slot) → nvme\_flush()

├── epoch\_commit\_lock.rs   # static EPOCH\_COMMIT\_LOCK: SpinLock<()>

&nbsp;                         # UN SEUL commit à la fois — jamais par GC

├── epoch\_recovery.rs      # max(epoch\_id) parmi slots magic+checksum valides

&nbsp;                         # Vérifie EpochRoot pointé (magic+checksum aussi)

├── epoch\_gc.rs            # Interface GC → DeferredDeleteQueue

&nbsp;                         # JAMAIS EPOCH\_COMMIT\_LOCK depuis le GC (règle DEAD-01)

├── epoch\_barriers.rs      # nvme\_flush() wrappé — mockable en tests

├── epoch\_checksum.rs      # Blake3 streaming EpochRecord + EpochRoot pages

├── epoch\_writeback.rs     # Writeback thread : timer EXOFS\_WRITEBACK, group commit

├── epoch\_snapshot.rs      # Snapshot = marquer un Epoch comme permanent (~1% coût)

├── epoch\_delta.rs         # Delta tracking — objets modifiés dans l'Epoch courant

├── epoch\_pin.rs           # EPOCH\_PINNED : set/clear avec vérif ref\_count

└── epoch\_stats.rs         # Histogramme latence commit, throughput

```



\### 2.5 storage/ — Stockage Disque (22 fichiers)



```

kernel/src/fs/exofs/storage/

│

├── mod.rs                 # Storage layer API

├── layout.rs              # Offsets FIXES — fn sector\_for\_offset()

&nbsp;                         # avec checked\_add() obligatoire (règle ARITH-02)

&nbsp;                         #

&nbsp;                         # Offset 0 : ExoSuperblock primaire (4KB)

&nbsp;                         # Offset 4KB : EpochSlot A

&nbsp;                         # Offset 8KB : EpochSlot B

&nbsp;                         # Offset 12KB : ExoSuperblock miroir

&nbsp;                         # Offset 1MB : Heap général (blobs, objets)

&nbsp;                         # Offset size-8KB: EpochSlot C

&nbsp;                         # Offset size-4KB: ExoSuperblock miroir final

├── superblock.rs          # ★ CORRECTION Z-AI : séparer disk / in-memory

&nbsp;                         #

&nbsp;                         # ExoSuperblockDisk #\[repr(C, align(4096))]

&nbsp;                         # TYPES PLAIN UNIQUEMENT (pas AtomicU64) :

&nbsp;                         # magic:u32, version\_major:u16, version\_minor:u16

&nbsp;                         # incompat\_flags:u64, compat\_flags:u64

&nbsp;                         # disk\_size\_bytes:u64, heap\_start:u64

&nbsp;                         # heap\_end:u64, slot\_a\_offset:u64

&nbsp;                         # slot\_b\_offset:u64, slot\_c\_offset:u64

&nbsp;                         # created\_at:u64, uuid:\[u8;16]

&nbsp;                         # volume\_name:\[u8;64], block\_size:u32

&nbsp;                         # object\_count:u64, ← plain u64, pas AtomicU64

&nbsp;                         # blob\_count:u64, free\_bytes:u64

&nbsp;                         # epoch\_current:u64

&nbsp;                         # checksum:\[u8;32] Blake3(tout ce qui précède)

&nbsp;                         #

&nbsp;                         # ExoSuperblockInMemory :

&nbsp;                         # disk: ExoSuperblockDisk

&nbsp;                         # object\_count: AtomicU64 ← compteur live

&nbsp;                         # free\_bytes: AtomicU64

&nbsp;                         # dirty: AtomicBool

&nbsp;                         #

&nbsp;                         # IMPLÉMENTE VfsSuperblock → root\_inode() FONCTIONNEL

&nbsp;                         # read\_and\_verify() : magic EN PREMIER, puis checksum

├── superblock\_backup.rs   # Miroirs offset 12KB + size-4KB — cross-validation

├── heap.rs                # Heap allocator : append-only objets, buddy metadata

├── heap\_allocator.rs      # Buddy implementation

&nbsp;                         # checked\_add() pour tous calculs (règle ARITH-02)

├── heap\_free\_map.rs       # Bitmap blocs libres — atomic updates

├── heap\_coalesce.rs       # Coalescing blocs libres adjacents

├── object\_writer.rs       # Write L-Obj : ObjectHeader + payload

&nbsp;                         # Vérifie bytes\_written == expected (règle WRITE-02)

├── object\_reader.rs       # Read L-Obj : vérifie ObjectHeader magic+checksum

&nbsp;                         # AVANT d'accéder au payload (règle HDR-03)

├── blob\_writer.rs         # Write P-Blob : BlobId calculé AVANT compression

&nbsp;                         # Pipeline : données → Blake3(BlobId)

&nbsp;                         # → compression → chiffrement → disque

├── blob\_reader.rs         # Read P-Blob : déchiffrement → décompression

├── extent\_writer.rs       # Write extent trees — atomique dans même Epoch

├── extent\_reader.rs       # Read extent trees

├── checksum\_writer.rs     # Blake3 streaming sur contenu brut (avant compression)

├── checksum\_reader.rs     # Vérification Blake3 — Err(Corrupt) si invalide

├── compression\_writer.rs  # LZ4/Zstd APRÈS calcul BlobId (règle HASH-02)

├── compression\_reader.rs  # Décompression APRÈS vérification checksum

├── compression\_choice.rs  # Auto-sélection : text→Zstd, media→None, data→Lz4

├── dedup\_writer.rs        # Lookup BlobId avant écriture — réutilise si trouvé

├── dedup\_reader.rs        # Lecture via BlobId partagé

├── block\_allocator.rs     # Politique d'allocation — Root Reserve protégée

├── block\_cache.rs         # Cache blocs 4KB — LRU avec shrinker

├── io\_batch.rs            # Batched I/O — regroupe writes en une Bio

└── storage\_stats.rs       # Stats I/O : latences, throughput, erreurs

```



\### 2.6 gc/ — Garbage Collection (16 fichiers)



```

kernel/src/fs/exofs/gc/

│

├── mod.rs                 # GC API

├── gc\_state.rs            # State machine : Idle→Scanning→Marking→Sweeping→Idle

├── gc\_thread.rs           # Background GC thread

&nbsp;                         # JAMAIS EPOCH\_COMMIT\_LOCK (règle DEAD-01)

├── gc\_scheduler.rs        # Déclenchement : espace libre < 20% OU timer 60s

├── tricolor.rs            # Algorithme tri-color : Blanc/Gris/Noir

├── marker.rs              # Mark phase — itératif avec grey\_queue heap

&nbsp;                         # grey\_queue: Vec<ObjectId> sur le heap (règle RECUR-04)

├── sweeper.rs             # Sweep phase — blobs ref\_count=0 depuis > 2 Epochs

├── reference\_tracker.rs   # Comptage références cross-Epoch

├── epoch\_scanner.rs       # Scan Epochs — racines GC = EpochRoots valides

├── relation\_walker.rs     # Parcours graphe relations — itératif BFS/DFS

├── cycle\_detector.rs      # Détection cycles — Tarjan itératif

├── orphan\_collector.rs    # Collecte orphelins (inaccessibles depuis racines)

├── blob\_gc.rs             # P-Blob GC : supprime si ref\_count=0 ET délai ≥ 2 Epochs

├── blob\_refcount.rs       # ref\_count avec PANIC sur underflow (règle REFCNT-01)

├── inline\_gc.rs           # GC données inline (< 512B dans L-Obj)

├── gc\_metrics.rs          # Métriques : objets collectés, durée phases

└── gc\_tuning.rs           # Auto-tuning : seuils selon charge système

```



\### 2.7 dedup/ — Déduplication (13 fichiers)



```

kernel/src/fs/exofs/dedup/

│

├── mod.rs                 # Dedup API

├── content\_hash.rs        # Blake3 sur contenu brut — AVANT compression

├── chunking.rs            # Dispatch : fixe ou CDC selon taille/type

├── chunker\_fixed.rs       # Fixed-size chunks 4KB/8KB (fichiers structurés)

├── chunker\_cdc.rs         # Content-Defined Chunking (rolling hash Rabin)

├── chunk\_cache.rs         # Cache chunks récents — évite rehash

├── blob\_registry.rs       # Registry BlobId → locations disque — kernel-only

├── blob\_sharing.rs        # Tracking partage : quels L-Objs partagent quel P-Blob

├── dedup\_stats.rs         # Ratio dédup, économies disque, CPU utilisé

├── dedup\_policy.rs        # Policy : always / size-threshold(>4KB) / off

├── chunk\_index.rs         # Index hash→BlobId — BTreeMap kernel-safe

├── chunk\_fingerprint.rs   # Fingerprinting rapide (early reject avant lookup)

├── similarity\_detect.rs   # Near-dedup : MinHash pour similarité

└── dedup\_api.rs           # API userspace : SYS\_EXOFS\_GET\_CONTENT\_HASH (audité)

```



\### 2.8 compress/ — Compression (10 fichiers)



```

kernel/src/fs/exofs/compress/

│

├── mod.rs                 # Compression API

├── algorithm.rs           # #\[repr(u8)] enum CompressionAlgo

&nbsp;                         # { None=0, Lz4=1, Zstd=2, ZstdMax=3 }

├── lz4\_wrapper.rs         # LZ4 bindings no\_std

&nbsp;                         # Vérifie output\_size après compression

├── zstd\_wrapper.rs        # Zstd bindings — niveau configurable 1-22

├── compress\_writer.rs     # Compression streaming — APRÈS calcul BlobId

├── decompress\_reader.rs   # Décompression — APRÈS vérification checksum Blake3

├── compress\_stats.rs      # Ratio compression, CPU time, algo utilisé

├── compress\_choice.rs     # Auto-sélection par MIME type

├── compress\_threshold.rs  # Taille minimum : pas de compression si < 512B

├── compress\_header.rs     # Header 8B : algo+original\_size — inclus dans checksum

└── compress\_benchmark.rs  # Benchmark runtime pour calibrer seuils

```



\### 2.9 crypto/ — Chiffrement Secrets (12 fichiers)



```

kernel/src/fs/exofs/crypto/

│

├── mod.rs                 # Crypto API ExoFS

├── key\_derivation.rs      # HKDF : MasterKey → VolumeKey → ObjectKey

├── master\_key.rs          # Clé maître : TPM-sealed ou Argon2

├── volume\_key.rs          # Clé par volume — dérivée au montage

├── object\_key.rs          # Clé par L-Obj Secret : HKDF(volume\_key, object\_id)

├── xchacha20.rs           # XChaCha20-Poly1305

&nbsp;                         # nonce unique par objet — JAMAIS réutilisé

├── secret\_writer.rs       # Pipeline : données→Blake3(BlobId)→compress→chiffrer

├── secret\_reader.rs       # Pipeline inverse : déchiffrer→décompress→vérifier

├── crypto\_shredding.rs    # Suppression sécurisée : oublier ObjectKey

├── key\_rotation.rs        # Rotation VolumeKey sans rechiffrement des données

├── key\_storage.rs         # Stockage : TPM/sealed en priorité, sinon chiffré PIN

├── entropy.rs             # Source entropie : RDRAND + TSC pour nonces

└── crypto\_audit.rs        # Audit toutes opérations crypto (ring buffer SEC-09)

```



\### 2.10 snapshot/ — Snapshots (12 fichiers)



```

kernel/src/fs/exofs/snapshot/

│

├── mod.rs                 # Snapshot API

├── snapshot.rs            # struct Snapshot { id, epoch\_id, name, created\_at }

├── snapshot\_create.rs     # mark\_epoch\_as\_snapshot() — coût O(1), un seul flag

├── snapshot\_list.rs       # Liste snapshots — depuis EpochRoot flags

├── snapshot\_mount.rs      # Monte snapshot en read-only via VFS

├── snapshot\_delete.rs     # Supprime snapshot → déclenche GC blobs exclusifs

├── snapshot\_protect.rs    # Protège snapshot de la suppression (TTL)

├── snapshot\_quota.rs      # Quota espace snapshots

├── snapshot\_diff.rs       # Diff entre 2 snapshots — compare EpochRoots

├── snapshot\_restore.rs    # Restauration depuis snapshot (nouveau Epoch)

├── snapshot\_streaming.rs  # Export incrémental streaming

└── snapshot\_gc.rs         # GC snapshot-aware : préserve blobs référencés

```



\### 2.11 relation/ — Graphe de Relations (11 fichiers)



```

kernel/src/fs/exofs/relation/

│

├── mod.rs                 # Relation API

├── relation.rs            # struct Relation { id, source, target, kind, epoch }

├── relation\_type.rs       # enum RelationType

&nbsp;                         # { DependsOn, DerivedFrom, Symlink,

&nbsp;                         # HardLink, Custom(u32) }

├── relation\_graph.rs      # Graphe in-memory — itératif (règle RECUR-01)

├── relation\_index.rs      # Index par source / par target

├── relation\_walker.rs     # BFS/DFS itératif — stack sur heap (règle RECUR-04)

├── relation\_query.rs      # API requête : find\_by\_source(), find\_by\_target()

├── relation\_batch.rs      # Batch insert/delete dans un seul EpochRoot

├── relation\_gc.rs         # Participation au GC tricolore

├── relation\_cycle.rs      # Tarjan itératif — vérifie avant insertion

└── relation\_storage.rs    # Persistance relations sur disque

```



\### 2.12 quota/ — Quotas (6 fichiers)



```

kernel/src/fs/exofs/quota/

│

├── mod.rs                 # Quota API

├── quota\_policy.rs        # Quota lié à la capability — pas à l'UID

├── quota\_tracker.rs       # Usage tracking par capability

├── quota\_enforcement.rs   # ENOSPC si dépassé — vérifié AVANT toute allocation

├── quota\_report.rs        # Rapports usage

├── quota\_namespace.rs     # Quotas par namespace (containers)

└── quota\_audit.rs         # Audit dépassements quota

```



\### 2.13 syscall/ — Syscalls ExoFS 500-518 (20 fichiers)



```

kernel/src/fs/exofs/syscall/

│

├── mod.rs                 # register\_exofs\_syscalls() → table syscall kernel

├── path\_resolve.rs        # SYS\_EXOFS\_PATH\_RESOLVE (500)

&nbsp;                         # Buffer per-CPU PATH\_BUFFERS (règle PATH-07)

&nbsp;                         # copy\_from\_user() obligatoire (règle SYS-01)

├── object\_open.rs         # SYS\_EXOFS\_OBJECT\_OPEN (501)

├── object\_read.rs         # SYS\_EXOFS\_OBJECT\_READ (502)

├── object\_write.rs        # SYS\_EXOFS\_OBJECT\_WRITE (503)

├── object\_create.rs       # SYS\_EXOFS\_OBJECT\_CREATE (504)

├── object\_delete.rs       # SYS\_EXOFS\_OBJECT\_DELETE (505)

├── object\_stat.rs         # SYS\_EXOFS\_OBJECT\_STAT (506)

├── object\_set\_meta.rs     # SYS\_EXOFS\_OBJECT\_SET\_META (507)

├── get\_content\_hash.rs    # SYS\_EXOFS\_GET\_CONTENT\_HASH(508) — audité SEC-09

├── snapshot\_create.rs     # SYS\_EXOFS\_SNAPSHOT\_CREATE (509)

├── snapshot\_list.rs       # SYS\_EXOFS\_SNAPSHOT\_LIST (510)

├── snapshot\_mount.rs      # SYS\_EXOFS\_SNAPSHOT\_MOUNT (511)

├── relation\_create.rs     # SYS\_EXOFS\_RELATION\_CREATE (512)

├── relation\_query.rs      # SYS\_EXOFS\_RELATION\_QUERY (513)

├── gc\_trigger.rs          # SYS\_EXOFS\_GC\_TRIGGER (514)

├── quota\_query.rs         # SYS\_EXOFS\_QUOTA\_QUERY (515)

├── export\_object.rs       # SYS\_EXOFS\_EXPORT\_OBJECT (516)

├── import\_object.rs       # SYS\_EXOFS\_IMPORT\_OBJECT (517)

├── epoch\_commit.rs        # SYS\_EXOFS\_EPOCH\_COMMIT (518)

└── validation.rs          # copy\_from\_user() helpers, bounds checks

&nbsp;                         # Utilisé par TOUS les autres syscalls

```



\### 2.14 posix\_bridge/ — Pont VFS Ring 0 (★ Correction Z-AI, 5 fichiers)



★ AJOUT non présent dans Z-AI. Remplace `posix/` Ring 0. Contient UNIQUEMENT les mécanismes kernel qui touchent directement la page table ou le VFS existant.



```

kernel/src/fs/exofs/posix\_bridge/

│

├── mod.rs                 # Re-exports, enregistrement dans VFS

├── inode\_emulation.rs     # ObjectId → ino\_t : mapping stable pour VFS existant

&nbsp;                         # Le VFS fs/core/vfs.rs en a besoin directement

├── vfs\_compat.rs          # Adapte ExofsInodeOps/FileOps → traits VfsSuperblock

&nbsp;                         # ★ MILESTONE 1 : root\_inode() fonctionnel ici

&nbsp;                         # ★ MILESTONE 2 : open/read/write fonctionnels ici

├── mmap.rs                # mmap : promotion Class1→Class2 si MAP\_SHARED|PROT\_WRITE

&nbsp;                         # Touche à la page table → Ring 0 obligatoire

└── fcntl\_lock.rs          # fcntl locks : mécanisme kernel, granularité byte-range

```



\### 2.15 io/ — Opérations I/O (13 fichiers)



```

kernel/src/fs/exofs/io/

│

├── mod.rs                 # I/O API

├── reader.rs              # Read path : L-Obj→extent\_tree→page\_cache→disque

├── writer.rs              # Write path : page\_cache dirty → commit Epoch (async)

&nbsp;                         # NE fait PAS le commit directement (writeback thread)

├── zero\_copy.rs           # True zero-copy : DMA → PageTable Ring 3 direct

├── direct\_io.rs           # O\_DIRECT : bypasse page cache, write synchrone

├── buffered\_io.rs         # Buffered I/O standard via page\_cache existant

├── async\_io.rs            # Async I/O via callbacks bio\_completion

├── io\_uring.rs            # io\_uring support — submission queue kernel-side

├── scatter\_gather.rs      # Scatter-gather pour gros fichiers

├── prefetch.rs            # Préchargement prédictif

├── readahead.rs           # Readahead adaptatif selon pattern d'accès

├── writeback.rs           # Intégration writeback thread existant

├── io\_batch.rs            # Regroupement I/Os en Bio unique

└── io\_stats.rs            # Statistiques I/O par objet et par type

```



\### 2.16 cache/ — Caches (12 fichiers)



```

kernel/src/fs/exofs/cache/

│

├── mod.rs                 # Cache coordination

├── object\_cache.rs        # LogicalObject cache

&nbsp;                         # PASSE PAR ObjectTable — jamais bypass (règle CACHE-02)

├── blob\_cache.rs          # PhysicalBlob cache — LRU avec shrinker

├── path\_cache.rs          # Résolution chemins — LRU 10 000 entrées

├── extent\_cache.rs        # Extent trees — hot path lecture

├── metadata\_cache.rs      # Metadata (ObjectMeta) cache

├── cache\_policy.rs        # Politiques : LRU / LFU / ARC

├── cache\_eviction.rs      # Logique éviction

├── cache\_pressure.rs      # Réaction à la pression mémoire

├── cache\_stats.rs         # Hit/miss ratios par cache

├── cache\_warming.rs       # Préchauffage cache au boot

└── cache\_shrinker.rs      # Callback memory pressure

&nbsp;                         # Libère dans l'ordre : blob→path→object

```



\### 2.17 recovery/ — Recovery et fsck (13 fichiers)



```

kernel/src/fs/exofs/recovery/

│

├── mod.rs                 # Recovery API

├── boot\_recovery.rs       # Séquence boot : magic→checksum→max(epoch)→verify\_root

├── slot\_recovery.rs       # Sélection slot A/B/C — vérifie les 3, prend max valide

├── epoch\_replay.rs        # Rejoue l'Epoch actif si nécessaire

├── fsck.rs                # Full check — orchestre les 4 phases

├── fsck\_phase1.rs         # Phase 1 : Superblock + miroirs + feature flags

├── fsck\_phase2.rs         # Phase 2 : Heap scan — tous ObjectHeaders magic+checksum

├── fsck\_phase3.rs         # Phase 3 : Reconstruction graphe L-Obj→P-Blob→extents

├── fsck\_phase4.rs         # Phase 4 : Détection orphelins (non-atteints racines)

├── fsck\_repair.rs         # Réparations : orphelins→lost+found, tronqués→truncate

├── checkpoint.rs          # Points de reprise recovery

├── recovery\_log.rs        # Journal recovery

└── recovery\_audit.rs      # Audit opérations recovery

```



\### 2.18 export/ — Export/Import (9 fichiers)



```

kernel/src/fs/exofs/export/

│

├── mod.rs                 # Export/Import API

├── exoar\_format.rs        # Format EXOAR : magic, versioning, chunks

├── exoar\_writer.rs        # Write archive EXOAR

├── exoar\_reader.rs        # Read archive EXOAR — vérif magic+checksum à l'entrée

├── tar\_compat.rs          # Compatibilité TAR (lecture uniquement)

├── stream\_export.rs       # Export streaming (pipe, réseau)

├── stream\_import.rs       # Import streaming

├── incremental\_export.rs  # Export incrémental depuis un Epoch de référence

├── metadata\_export.rs     # Export métadonnées seules

└── export\_audit.rs        # Audit toutes opérations export (données sensibles)

```



\### 2.19 numa/ — NUMA Awareness (6 fichiers)



```

kernel/src/fs/exofs/numa/

│

├── mod.rs

├── numa\_placement.rs      # Placement objets selon NUMA node du process owner

├── numa\_migration.rs      # Migration entre nodes (background, non-urgent)

├── numa\_affinity.rs       # Tracking affinité process

├── numa\_stats.rs          # Statistiques NUMA : local vs remote hits

└── numa\_tuning.rs         # Auto-tuning seuils migration

```



\### 2.20 observability/ — Observabilité (10 fichiers)



```

kernel/src/fs/exofs/observability/

│

├── mod.rs

├── metrics.rs             # Compteurs performance (AtomicU64)

├── tracing.rs             # Tracing opérations (ring buffer non-bloquant)

├── health\_check.rs        # Monitoring santé : espace libre, GC lag, commit latency

├── alert.rs               # Génération alertes (seuils configurables)

├── perf\_counters.rs       # Compteurs hardware perf (PMU)

├── latency\_histogram.rs   # Distribution latences par opération

├── throughput\_tracker.rs  # Débit lecture/écriture

├── space\_tracker.rs       # Suivi espace : heap, blobs, metadata

└── debug\_interface.rs     # Interface debug/sysrq

```



\### 2.21 audit/ — Audit Trail (8 fichiers)



```

kernel/src/fs/exofs/audit/

│

├── mod.rs

├── audit\_log.rs           # Ring buffer non-bloquant — jamais de perte d'événement

├── audit\_entry.rs         # struct AuditEntry { ts, op, actor\_cap, object\_id, result }

├── audit\_writer.rs        # Write entrées audit — lock-free

├── audit\_reader.rs        # Read entrées audit (userspace via syscall)

├── audit\_rotation.rs      # Rotation log (taille max configurable)

├── audit\_filter.rs        # Filtrage par opération, acteur, objet

└── audit\_export.rs        # Export audit vers EXOAR

```



\### 2.22 tests/ — Tests (~20 fichiers)



```

kernel/src/fs/exofs/tests/

│

├── mod.rs                 # Framework de test kernel

├── unit/

│   ├── object\_id\_test.rs  # Tests ObjectId : class1, class2, ct\_eq

│   ├── blob\_id\_test.rs    # Tests BlobId : Blake3 avant compression

│   ├── epoch\_test.rs      # Tests EpochRecord : taille 104B, checksum

│   ├── path\_index\_test.rs # Tests PathIndex : lookup, split atomique

│   ├── dedup\_test.rs      # Tests dédup : même contenu = même BlobId

│   ├── compress\_test.rs   # Tests compression : BlobId stable

│   ├── crypto\_test.rs     # Tests chiffrement : nonce unique

│   └── gc\_test.rs         # Tests GC : tricolore, cycles, underflow panic

├── integration/

│   ├── create\_read\_test.rs    # Create→Read cycle complet

│   ├── epoch\_commit\_test.rs   # Commit avec 3 barrières NVMe

│   ├── snapshot\_test.rs       # Create snapshot + mount read-only

│   ├── recovery\_test.rs       # Simulation crash → recovery

│   └── stress\_test.rs         # Stress test concurrent

└── fuzz/

&nbsp;   ├── path\_resolve\_fuzz.rs   # Fuzzing résolution chemins

&nbsp;   ├── epoch\_parse\_fuzz.rs    # Fuzzing parsing EpochRecord

&nbsp;   └── object\_parse\_fuzz.rs   # Fuzzing parsing ObjectHeader

```



\### 2.23 Ring 1 — servers/posix\_server/ (★ Correction Z-AI)



Tout ce qui était dans `posix/` Ring 0 chez Z-AI et qui relève de la POLITIQUE migre ici. Ce serveur redémarre sans kernel panic si il crashe.



```

servers/posix\_server/src/

│

├── main.rs                # Point d'entrée Ring 1 — restart automatique si crash

├── path/

│   ├── mod.rs

│   ├── parser.rs          # Parsing path POSIX : /a/b/../c → composants

│   ├── resolver.rs        # Appelle SYS\_EXOFS\_PATH\_RESOLVE → ObjectId

│   └── cache.rs           # Cache côté Ring 1 (évite syscalls redondants)

├── ops/

│   ├── mod.rs

│   ├── open.rs            # open() → SYS\_EXOFS\_OBJECT\_OPEN

│   ├── read.rs            # read() → SYS\_EXOFS\_OBJECT\_READ

│   ├── write.rs           # write() → SYS\_EXOFS\_OBJECT\_WRITE

│   ├── stat.rs            # stat() → ObjectMeta → struct stat POSIX

│   ├── readdir.rs         # readdir() → PathIndex entries → struct dirent

│   ├── rename.rs          # rename() → SYS\_EXOFS\_RENAME (atomique Epoch)

│   ├── link.rs            # link() hard links → Relation HardLink

│   ├── symlink.rs         # symlink() → Relation Symlink

│   ├── xattr.rs           # getxattr/setxattr → ObjectMeta extended

│   └── acl.rs             # ACL emulation → Rights mapping

├── compat/

│   ├── mod.rs

│   ├── permission.rs      # chmod/chown → Rights bitfield

│   ├── errno.rs           # ExofsError → errno POSIX

│   └── flags.rs           # O\_RDONLY, O\_CREAT, O\_TRUNC → ExoFS flags

└── nfs/

&nbsp;   ├── mod.rs

&nbsp;   ├── v3.rs              # NFSv3 server — politique réseau pure Ring 1

&nbsp;   └── v4.rs              # NFSv4 server

```



---



\## 3. Règles Fondamentales Rust no\_std



\### 3.1 Imports autorisés



| Sév. | ID | Règle |

|------|-----|-------|

| ✅ | NO-STD-01 | `use core::...` pour primitives (`core::sync::atomic`, `core::mem`, `core::ptr`, `core::fmt`) |

| ✅ | NO-STD-02 | `use alloc::...` pour collections (`alloc::vec::Vec`, `alloc::sync::Arc`, `alloc::string::String`) |

| ✅ | NO-STD-03 | Locks : `use crate::scheduler::sync::{spinlock::SpinLock, rwlock::RwLock}` UNIQUEMENT |

| ❌ | NO-STD-04 | INTERDIT : `use std::...` — std n'existe pas en no\_std kernel |

| ❌ | NO-STD-05 | INTERDIT : `std::sync::Mutex`, `std::sync::RwLock`, `std::thread` |

| ❌ | NO-STD-06 | INTERDIT : `println!`, `eprintln!`, `print!` — utiliser `log\_kernel!()` kernel |

| ❌ | NO-STD-07 | INTERDIT : `std::collections::HashMap` — utiliser BTreeMap ou hash table shardée |



\### 3.2 OOM Safety — Allocations



\*\*RÈGLE OOM-01 :\*\* TOUT code qui alloue dans le kernel DOIT utiliser les variantes fallible (`try\_`). Un panic en OOM est une panne kernel totale.



| Sév. | ID | Règle |

|------|-----|-------|

| ❌ | OOM-01 | INTERDIT : `Vec::push(x)` sans `try\_reserve` — peut paniquer en OOM |

| ✅ | OOM-02 | OBLIGATOIRE : `vec.try\_reserve(1).map\_err(\\|\_\\| ExofsError::NoMemory)?;` puis `vec.push(x)` |

| ❌ | OOM-03 | INTERDIT : `Vec::with\_capacity(n)` sans vérification — utiliser `try\_with\_capacity(n)?` |

| ❌ | OOM-04 | INTERDIT : `alloc::vec!\[a,b,c]` en hot path kernel — créer manuellement avec `try\_reserve` |

| ✅ | OOM-05 | AUTORISÉ : `alloc::vec!\[]` pour Vec vides (pas d'allocation initiale) |

| ⚠️ | OOM-06 | ATTENTION : `Box::new(x)` en hot path — préférer types stack-allocated ou pools |



```rust

// ❌ MAUVAIS — panic en OOM

fn add\_object(\&mut self, obj: LogicalObject) {

&nbsp;   self.objects.push(obj); // INTERDIT

}



// ✅ BON — fallible, obligatoire

fn add\_object(\&mut self, obj: LogicalObject) -> Result<(), ExofsError> {

&nbsp;   self.objects.try\_reserve(1).map\_err(|\_| ExofsError::NoMemory)?;

&nbsp;   self.objects.push(obj); // safe après try\_reserve

&nbsp;   Ok(())

}

```



\### 3.3 Unsafe



| Sév. | ID | Règle |

|------|-----|-------|

| ✅ | UNSAFE-01 | Tout bloc `unsafe{}` DOIT avoir `// SAFETY: <raison précise>` immédiatement au-dessus |

| ❌ | UNSAFE-02 | INTERDIT : `unsafe{}` sans commentaire SAFETY — rejet systématique en review |

| ✅ | UNSAFE-03 | `copy\_from\_user()` OBLIGATOIRE pour tout pointeur venant de Ring 1 / userspace |

| ❌ | UNSAFE-04 | INTERDIT : déréférencer un pointeur userspace sans `copy\_from\_user()` — exploit garanti |



\### 3.4 Structs on-disk — format physique



| Sév. | ID | Règle |

|------|-----|-------|

| ✅ | ONDISK-01 | Structures écrites sur disque : `#\[repr(C)]` ou `#\[repr(C, packed)]` — layout déterministe |

| ✅ | ONDISK-02 | Types plain uniquement : `u32`, `u64`, `\[u8; N]` — PAS `AtomicU64`, PAS `Vec`, PAS `Arc` |

| ❌ | ONDISK-03 | INTERDIT : `AtomicU64` dans struct on-disk — bit pattern non-déterministe → checksum invalide |

| ❌ | ONDISK-04 | INTERDIT : `Vec<T>` dans struct on-disk — pas de layout fixe |

| ✅ | ONDISK-05 | SÉPARER `XyzDisk` (types plain, on-disk) de `XyzInMemory` (`AtomicU64`, en RAM) |

| ✅ | ONDISK-06 | const assert obligatoire : `const \_: () = assert!(size\_of::<EpochRecord>() == 104)` |



---



\## 4. Modèle d'Objets ExoFS — L-Obj / P-Blob



\### 4.1 Séparation Logique / Physique



\*\*Principe fondamental :\*\* L-Obj = identité stable visible par les applications. P-Blob = contenu physique content-addressed. Cette séparation est NON NÉGOCIABLE et est la base de la déduplication, du CoW, et des snapshots.



| Concept | Description |

|---------|-------------|

| \*\*LogicalObject (L-Obj)\*\* | Identité stable. ObjectId Classe 1 = Blake3(contenu\\|\\|owner\_cap). Classe 2 = compteur u64 monotone. Possède : owner\_cap, generation, droits, lien vers P-Blob. |

| \*\*PhysicalBlob (P-Blob)\*\* | Contenu physique. BlobId = Blake3(contenu brut non-compressé). Partagé entre L-Objs (dédup). `ref\_count:AtomicU32`. |

| \*\*PhysicalRef\*\* | Enum dans L-Obj : `Unique{blob\_id}`, `Shared{blob\_id, share\_idx}`, `Inline{data:\[u8;512], len:u16}` pour petits fichiers. |

| \*\*ObjectHeader\*\* | Header universel 64 bytes — magic `0x4F424A45` + Blake3 checksum. Tout objet disque commence par lui. |



| Sév. | ID | Règle |

|------|-----|-------|

| ✅ | LOBJ-01 | `SYS\_EXOFS\_PATH\_RESOLVE` retourne TOUJOURS l'ObjectId du L-Obj — jamais le BlobId |

| ❌ | LOBJ-02 | INTERDIT : exposer BlobId hors kernel sans `Rights::INSPECT\_CONTENT` |

| ❌ | LOBJ-03 | INTERDIT : `ObjectKind::Secret` → BlobId jamais exposé même avec `INSPECT\_CONTENT` |

| ❌ | LOBJ-04 | INTERDIT : mmap writable direct sur objet Class1 immuable (→ promouvoir Class2 d'abord) |

| ✅ | LOBJ-05 | Comparaison ObjectId en temps constant `ct\_eq()` — résistance timing attacks |

| ✅ | LOBJ-06 | ObjectId Class1 = Blake3(blob\_id \\|\\| owner\_cap) — calculé UNE SEULE FOIS à la création |

| ❌ | LOBJ-07 | INTERDIT : modifier ObjectId Class2 après création — il est stable à vie |



---



\## 5. Système d'Epochs — Atomicité et Recovery



\### 5.1 Protocole de commit — 3 barrières NVMe



\*\*RÈGLE EPOCH-01 (CRITIQUE) :\*\* L'ordre des écritures et barrières est INVIOLABLE. Inverser cet ordre = corruption garantie au prochain reboot.



```

Phase 1 — Écrire les données objet (payload)

&nbsp;   write(payload\_data → heap\_offset)

&nbsp;   nvme\_flush() ← BARRIÈRE 1 — OBLIGATOIRE



Phase 2 — Écrire l'EpochRoot

&nbsp;   write(EpochRoot → epoch\_root\_zone)

&nbsp;   nvme\_flush() ← BARRIÈRE 2 — OBLIGATOIRE



Phase 3 — Écrire l'EpochRecord dans le slot

&nbsp;   write(EpochRecord → slot\_A ou slot\_B ou slot\_C)

&nbsp;   nvme\_flush() ← BARRIÈRE 3 — OBLIGATOIRE

```



\- Crash entre Phase 1 et 2 → données orphelines, ignorées au recovery

\- Crash entre Phase 2 et 3 → EpochRoot sans EpochRecord, ignoré

\- Phase 3 complète → Epoch valide, recovery O(1) possible



| Sév. | ID | Règle |

|------|-----|-------|

| ❌ | EPOCH-01 | INTERDIT : écrire EpochRecord AVANT les données — recovery pointe vers inexistant |

| ❌ | EPOCH-02 | INTERDIT : omettre une barrière NVMe — reordering disque = corruption silencieuse |

| ✅ | EPOCH-03 | `EPOCH\_COMMIT\_LOCK` : SpinLock obligatoire — un seul commit à la fois |

| ❌ | EPOCH-04 | INTERDIT : GC thread demande `EPOCH\_COMMIT\_LOCK` — deadlock avec writeback (DEAD-01) |

| ✅ | EPOCH-05 | EpochRoot ≤ 500 objets par Epoch — commit anticipé si dépassé |

| ✅ | EPOCH-06 | Recovery = max(epoch\_id) parmi slots avec magic+checksum valides |

| ✅ | EPOCH-07 | Chaque page EpochRoot chainée vérifie son propre magic `0x45504F43` + checksum |

| ❌ | EPOCH-08 | INTERDIT : faire confiance à `next\_page` sans vérifier magic+checksum de la page |

| ✅ | EPOCH-09 | `EPOCH\_PINNED` sur frames du slot inactif — libéré uniquement au commit suivant |

| ❌ | EPOCH-10 | INTERDIT : libérer une frame `FrameFlags::EPOCH\_PINNED` — use-after-free garanti |



---



\## 6. PathIndex — Répertoires



| Sév. | ID | Règle |

|------|-----|-------|

| ✅ | PATH-01 | SipHash-2-4 avec `mount\_secret\_key:\[u8;16]` (aléatoire au montage) — anti Hash-DoS |

| ❌ | PATH-02 | INTERDIT : hash non-keyed pour PathIndex — vulnérable DoS |

| ✅ | PATH-03 | Collision de hash → comparer le nom COMPLET byte-à-byte pour confirmation |

| ✅ | PATH-04 | Split automatique si > 8192 entrées — SplitOp atomique dans UN SEUL EpochRoot |

| ❌ | PATH-05 | INTERDIT : split PathIndex en 2 Epochs séparés — crash mid-split = répertoire mort |

| ❌ | PATH-06 | INTERDIT : tenir PathIndex lock pendant une opération I/O bloquante |

| ✅ | PATH-07 | Buffer per-CPU pour PATH\_MAX : `static PATH\_BUFFERS: PerCpu<\[u8;4096]>` |

| ❌ | PATH-08 | INTERDIT : `let buf = \[0u8; 4096]` sur la stack kernel — stack overflow silencieux |

| ✅ | PATH-09 | `rename()` atomique dans le même EpochRoot — via `SYS\_EXOFS\_RENAME` dédié |

| ❌ | PATH-10 | INTERDIT : `rename()` = `unlink()` + `link()` séparés — non-atomique |



---



\## 7. Sécurité — Capabilities et Zero Trust



| Sév. | ID | Règle |

|------|-----|-------|

| ✅ | SEC-01 | TOUT accès à un objet passe par `verify\_cap(cap, object\_id, rights)` — sans exception |

| ❌ | SEC-02 | INTERDIT : accéder à un objet sans vérifier la capability — violation Zero Trust |

| ✅ | SEC-03 | Vérification O(1) : `token.generation == table\[object\_id].generation` |

| ✅ | SEC-04 | Révocation = increment atomique de generation — tous tokens existants invalides |

| ❌ | SEC-05 | INTERDIT : réimplémenter `verify()` hors de `security/capability/` — duplication interdite |

| ✅ | SEC-06 | `Rights::INSPECT\_CONTENT` requis pour `SYS\_EXOFS\_GET\_CONTENT\_HASH` — audité |

| ❌ | SEC-07 | INTERDIT : exposer BlobId d'un `ObjectKind::Secret` — même avec `INSPECT\_CONTENT` |

| ✅ | SEC-08 | Délégation capability : droits\_délégués ⊆ droits\_délégateur — PROP-3 prouvée Coq |

| ✅ | CRYPTO-01 | Pipeline crypto obligatoire : données→Blake3(BlobId)→compression→chiffrement→disque |

| ❌ | CRYPTO-02 | INTERDIT : compresser après chiffrement — ciphertext incompressible |

| ❌ | CRYPTO-03 | INTERDIT : réutiliser un nonce avec la même clé — violation cryptographique totale |

| ✅ | CRYPTO-04 | Crypto-shredding : oublier l'ObjectKey = suppression sécurisée sans effacement physique |



---



\## 8. Locks — Hiérarchie et Deadlock Prevention



\*\*RÈGLE LOCK-01 (CRITIQUE) :\*\* Toujours acquérir les locks dans l'ordre croissant de niveau. Inverser = deadlock garanti.



\*\*Ordre strict (du PLUS BAS au PLUS ÉLEVÉ) :\*\*



1\. \*\*Niveau 1 :\*\* `memory/` SpinLocks (buddy, frame descriptor)

2\. \*\*Niveau 2 :\*\* `scheduler/` WaitQueue SpinLocks

3\. \*\*Niveau 3 :\*\* `memory/` PageTable Locks

4\. \*\*Niveau 4 :\*\* `fs/` Inode RwLock

5\. \*\*Niveau 5 :\*\* `fs/exofs/` dentry\_cache LRU Lock

6\. \*\*Niveau 6 :\*\* `fs/exofs/` PathIndex RwLock

7\. \*\*Niveau 7 :\*\* `fs/exofs/` EPOCH\_COMMIT\_LOCK ← le plus élevé de fs/



\*\*JAMAIS :\*\* tenir lock Niveau N et demander lock Niveau < N



| Sév. | ID | Règle |

|------|-----|-------|

| ✅ | LOCK-01 | Acquérir les locks dans l'ordre croissant de niveau — toujours |

| ❌ | LOCK-02 | INTERDIT : tenir PathIndex lock (N6) et demander Inode lock (N4) — deadlock |

| ✅ | LOCK-03 | Relâcher lock inode AVANT de dormir ou attendre I/O (release-before-sleep) |

| ❌ | LOCK-04 | INTERDIT : tenir un SpinLock pendant `sleep()` ou `wait()` — non-préemptif |

| ❌ | LOCK-05 | INTERDIT : tenir `EPOCH\_COMMIT\_LOCK` pendant I/O disque direct |

| ✅ | LOCK-06 | GC communique avec writeback via `DeferredDeleteQueue` lock-free — jamais `EPOCH\_COMMIT\_LOCK` |



---



\## 9. Garbage Collection — Règles Critiques



| Sév. | ID | Règle |

|------|-----|-------|

| ✅ | GC-01 | `DeferredDeleteQueue` : délai minimum 2 Epochs avant suppression réelle |

| ✅ | GC-02 | GC tricolore traverse les Relations — sinon cycles orphelins jamais collectés |

| ✅ | GC-03 | File grise bornée : `MAX\_GC\_GREY\_QUEUE = 1 000 000` — si dépassé, reporter |

| ✅ | GC-04 | `try\_reserve()` obligatoire pour la file grise — si OOM : Err et reporter |

| ❌ | GC-05 | INTERDIT : GC bloquant dans le chemin critique d'écriture — toujours background |

| ✅ | GC-06 | Racines GC = EpochRoots des slots A/B/C valides |

| ❌ | GC-07 | INTERDIT : collecter P-Blob avec `EPOCH\_PINNED` actif sur ses frames |

| ✅ | GC-08 | Création P-Blob atomique : alloc + `ref\_count.store(1)` + insert(ObjectTable) — indivisible |

| ❌ | GC-09 | INTERDIT : créer P-Blob sans `ref\_count=1` immédiat — GC peut le détruire avant usage |



---



\## 10. Erreurs Silencieuses — 10 Catégories Critiques



Ces erreurs ne provoquent PAS de crash immédiat. Elles corrompent silencieusement ou créent des fuites permanentes. \*\*Les plus dangereuses du module.\*\*



| ID | Cause | Conséquence | Correction obligatoire |

|----|-------|-------------|------------------------|

| \*\*ARITH-01\*\* | offset + len sans `checked\_add()` | Overflow u64 → écriture offset 0 → superblock écrasé | `checked\_add(len).ok\_or(ExofsError::OffsetOverflow)?` pour TOUT calcul d'adresse disque |

| \*\*WRITE-01\*\* | Ignorer `bytes\_written` retourné | Fichier tronqué sans erreur visible | `assert!(bytes\_written == data.len())` sinon `Err(PartialWrite)` |

| \*\*REFCNT-01\*\* | `fetch\_sub(1)` sur `ref\_count=0` | Wraps à `u32::MAX` → blob jamais collecté → fuite disque permanente | `compare\_exchange` avec vérification `current>0`, panic si underflow (bug kernel) |

| \*\*SPLIT-01\*\* | Split en 2 Epochs séparés | Crash mid-split → `split\_marker` sans enfants → répertoire inaccessible | Les 2 enfants + mise à jour parent = UN SEUL EpochRoot |

| \*\*CACHE-01\*\* | Créer LogicalObject sans ObjectTable | 2 instances du même objet en RAM → corruption état | `ObjectTable` = SEULE source de vérité — tout accès via `ObjectTable::get()` |

| \*\*RECUR-01\*\* | Récursion sur stack kernel (GC, symlinks) | Stack overflow corrompt mémoire voisine silencieusement | Toujours itératif + stack explicite allouée sur le heap |

| \*\*HASH-01\*\* | `Blake3(données\_compressées)` | BlobIds différents pour mêmes données → déduplication à 0% | BlobId = Blake3(contenu brut NON-compressé, NON-chiffré) — TOUJOURS |

| \*\*RACE-01\*\* | GC voit `ref\_count=0` pendant création | Blob valide détruit → use-after-free | `store(ref\_count=1)` → barrier → insert(ObjectTable) — séquence atomique |

| \*\*CHAIN-01\*\* | Lire `next\_page` sans vérifier magic | Lecture heap arbitraire → interprété comme liste objets → corruption totale | Chaque page chainée : vérifier magic `0x45504F43` + checksum AVANT lecture |

| \*\*DEAD-01\*\* | GC demande `EPOCH\_COMMIT\_LOCK` | Writeback tient `EPOCH\_COMMIT\_LOCK`, attend ObjectTable. GC tient ObjectTable, attend `EPOCH\_COMMIT\_LOCK` → kernel figé | GC → `DeferredDeleteQueue` uniquement. Jamais `EPOCH\_COMMIT\_LOCK` depuis le GC |



---



\## 11. Interface VFS et Syscalls



\### 11.1 Traits à implémenter



```rust

// storage/superblock.rs :

impl VfsSuperblock for ExofsVfsSuperblock {

&nbsp;   fn root\_inode(\&self) -> FsResult<InodeRef> { /\* OBLIGATOIRE — débloque path\_lookup \*/ }

&nbsp;   fn statfs(\&self) -> FsResult<FsStats> { /\* statistiques \*/ }

&nbsp;   fn sync\_fs(\&self, wait: bool) -> FsResult<()> { /\* flush + commit Epoch \*/ }

&nbsp;   fn alloc\_inode(\&self) -> FsResult<InodeRef> { /\* créer L-Obj, wrapper Inode VFS \*/ }

}



// posix\_bridge/vfs\_compat.rs :

impl InodeOps for ExofsInodeOps {

&nbsp;   fn lookup(\&self, dir: \&InodeRef, name: \&\[u8]) -> FsResult<DentryRef> { /\* PathIndex \*/ }

&nbsp;   fn create(\&self, dir: \&InodeRef, name: \&\[u8], ...) -> FsResult<InodeRef> { ... }

}



impl FileOps for ExofsFileOps {

&nbsp;   fn read (\&self, fh: \&FileHandle, buf: \&mut \[u8], off: u64) -> FsResult<usize> { ... }

&nbsp;   fn write(\&self, fh: \&FileHandle, buf: \&\[u8], off: u64) -> FsResult<usize> { ... }

&nbsp;   fn fsync(\&self, fh: \&FileHandle, datasync: bool) -> FsResult<()> { /\* commit Epoch \*/ }

}

```



| Sév. | ID | Règle |

|------|-----|-------|

| ❌ | VFS-01 | INTERDIT : retourner `Err(FsError::NotSupported)` dans `root\_inode()` — kernel ne peut pas booter |

| ✅ | VFS-02 | `FileOps::fsync()` déclenche `SYS\_EXOFS\_EPOCH\_COMMIT` — commit dur immédiat |

| ✅ | VFS-03 | `FileOps::write()` écrit dans page\_cache + marque dirty — pas de commit direct |

| ✅ | VFS-04 | Le writeback thread flush page\_cache → commit Epoch périodique |

| ❌ | VFS-05 | INTERDIT : modifier les traits `VfsSuperblock`/`InodeOps`/`FileOps` existants — uniquement les implémenter |



---



\## 12. Modifications Requises dans les Autres Modules



Modifications MINIMALES pour la Phase 1. Ne rien ajouter de plus pour éviter la sur-ingénierie.



| Fichier existant | Modification requise |

|------------------|---------------------|

| `memory/physical/frame/descriptor.rs` | Ajouter dans `FrameFlags` :<br>`EPOCH\_PINNED = 1 << 5` (blocs slot Epoch inactif)<br>`EXOFS\_PINNED = 1 << 6` (pages ExoFS non-évictables) |

| `security/capability/rights.rs` | Ajouter dans `Rights` bitflags :<br>`INSPECT\_CONTENT = 1 << 10`<br>`SNAPSHOT\_CREATE = 1 << 11`<br>`RELATION\_CREATE = 1 << 12`<br>`GC\_TRIGGER = 1 << 13` |

| `syscall/numbers.rs` | `SYS\_EXOFS\_PATH\_RESOLVE = 500`<br>`SYS\_EXOFS\_OBJECT\_OPEN = 501`<br>`SYS\_EXOFS\_OBJECT\_READ = 502`<br>`SYS\_EXOFS\_OBJECT\_WRITE = 503`<br>`SYS\_EXOFS\_OBJECT\_CREATE = 504`<br>`SYS\_EXOFS\_OBJECT\_DELETE = 505`<br>`SYS\_EXOFS\_OBJECT\_STAT = 506`<br>`SYS\_EXOFS\_OBJECT\_SET\_META = 507`<br>`SYS\_EXOFS\_GET\_CONTENT\_HASH= 508`<br>`SYS\_EXOFS\_EPOCH\_COMMIT = 518` |

| `syscall/table.rs` (ou `dispatch.rs`) | Router syscalls 500-518 vers `fs/exofs/syscall/` |

| `fs/mod.rs` | Ajouter : `pub mod exofs;`<br>Remplacer : `ext4\_register\_fs()` → `exofs::exofs\_register\_fs()`<br>Ajouter : `static RT\_HINT\_PROVIDER: AtomicPtr<()>` (injection RT bypass)<br>Le scheduler injecte `fn()→bool` au boot via ce pointeur |

| `fs/core/vfs.rs` | Compléter les stubs qui retournent `NotSupported` :<br>`fd\_read()`, `open()`, `close()`, `write()`<br>Ces fonctions délèguent via `FdTable` → `ExofsFileOps` |

| `scheduler/sync/wait\_queue.rs` | Augmenter `EMERGENCY\_POOL\_SIZE` à 96<br>(+16 pour GC thread + writeback thread ExoFS) |

| `process/core/tcb.rs` | Ajouter : `exofs\_dirty\_objects: Vec<ObjectId>`<br>Tracking objets dirty par thread |



---



\## 13. Séquence de Boot ExoFS



```rust

// fs/exofs/mod.rs — exofs\_init()

pub fn exofs\_init() -> Result<(), ExofsError> {



&nbsp;   // Étape 1 : Lecture superblock

&nbsp;   let frame = crate::memory::physical::allocator::alloc\_frame(KERNEL)?;

&nbsp;   let sb = storage::superblock::read\_and\_verify(ROOT\_DEV, frame.phys\_addr())?;

&nbsp;   sb.verify\_magic()?;         // 0x45584F46 EN PREMIER — si invalide : STOP

&nbsp;   sb.verify\_checksum()?;      // Blake3 du superblock complet

&nbsp;   sb.verify\_version()?;       // version compatible avec ce kernel

&nbsp;   sb.verify\_mirrors()?;       // cross-validation avec les 2 miroirs



&nbsp;   // Étape 2 : Recovery slots A/B/C

&nbsp;   let epoch\_id = epoch::recovery::recover\_active\_epoch(ROOT\_DEV)?;

&nbsp;   // → max(epoch\_id) parmi slots avec magic+checksum valides

&nbsp;   // → vérifie l'EpochRoot pointé (magic+checksum de chaque page chainée)



&nbsp;   // Étape 3 : Initialisation caches

&nbsp;   cache::object\_cache::init(OBJECT\_CACHE\_SIZE)?;

&nbsp;   cache::blob\_cache::init(BLOB\_CACHE\_SIZE)?;

&nbsp;   cache::path\_cache::init(PATH\_CACHE\_SIZE)?;



&nbsp;   // Étape 4 : Threads background

&nbsp;   epoch::epoch\_writeback::start\_thread()?;  // commit Epoch périodique (1ms)

&nbsp;   gc::gc\_thread::start\_thread()?;           // GC tricolore background



&nbsp;   // Étape 5 : Shrinker memory pressure

&nbsp;   crate::memory::utils::shrinker::register(cache::cache\_shrinker::exofs\_shrink)?;



&nbsp;   // Étape 6 : Syscalls 500-518

&nbsp;   syscall::register\_exofs\_syscalls()?;



&nbsp;   // Étape 7 : Mount root

&nbsp;   exofs\_register\_fs();

&nbsp;   vfs\_mount("exofs", ROOT\_DEV, "/", MountFlags::default(), "")?;

&nbsp;   // ← MILESTONE 3 : premier boot ISO



&nbsp;   Ok(())

}

```



---



\## 14. Milestones Phase 1 — Ordre d'Implémentation



| Milestone | Ce qui est débloqué |

|-----------|---------------------|

| \*\*M1 — superblock.rs\*\*<br>(`storage/superblock.rs`) | `root\_inode()` retourne Ok(). `path\_lookup()` peut traverser l'arbre. Le VFS peut monter ExoFS. C'est le déblocage de TOUT le reste. |

| \*\*M2 — vfs\_compat.rs\*\*<br>(`posix\_bridge/vfs\_compat.rs`) | `InodeOps` + `FileOps` complets. `open()`, `read()`, `write()`, `close()` fonctionnels. Les syscalls POSIX passent. |

| \*\*M3 — mod.rs\*\*<br>(racine `exofs/`) | `exofs\_init()` complet. Kernel boote sur ExoFS. Premier ISO bootable. Phase 2 peut commencer. |



\*\*Ordre de création des fichiers :\*\* `core/` (types) → `storage/` (layout) → `epoch/record` → `superblock` ★M1 → `epoch/slots+recovery` → `objects/` → `path/` → `io/` → `epoch/commit` → `vfs\_compat` ★M2 → `syscall/` → `mod.rs` ★M3



---



\## 15. Checklist de Vérification — Passe 2 Copilot



Après chaque génération Copilot : vérifier systématiquement chaque point avant d'accepter le code.



| Sév. | ID | Règle |

|------|-----|-------|

| ✅ | V-01 | Tous les imports : `use core::...` ou `use alloc::...` ou `crate::...` — aucun `use std::` |

| ✅ | V-02 | Toutes les allocations Vec : `try\_reserve(1)?` avant `push()` |

| ✅ | V-03 | Tous les blocs `unsafe{}` : commentaire `// SAFETY:` présent |

| ✅ | V-04 | Structs on-disk : `#\[repr(C)]` + types plain (u32, u64, `\[u8;N]`) — pas AtomicU64 |

| ✅ | V-05 | const assert sur toutes les tailles critiques : `size\_of::<EpochRecord>() == 104` |

| ✅ | V-06 | Locks : `crate::scheduler::sync::` uniquement — pas `std::sync::` |

| ✅ | V-07 | DAG respecté : pas d'import `scheduler/`, `ipc/`, `process/` direct |

| ✅ | V-08 | `verify\_cap()` appelé AVANT tout accès objet |

| ✅ | V-09 | BlobId jamais exposé sans `INSPECT\_CONTENT` — Secret : jamais |

| ✅ | V-10 | `copy\_from\_user()` pour tout pointeur Ring 1 → Ring 0 |

| ✅ | V-11 | Buffer per-CPU pour PATH\_MAX — pas de `\[u8;4096]` sur stack kernel |

| ✅ | V-12 | 3 barrières NVMe dans le bon ordre : data→flush→root→flush→record→flush |

| ✅ | V-13 | magic `0x45584F46` vérifié EN PREMIER dans tout parsing on-disk |

| ✅ | V-14 | Recovery : max(epoch\_id) parmi slots checksum valides |

| ✅ | V-15 | Pas de récursion sur stack kernel — itératif + stack heap |

| ✅ | V-16 | BlobId = Blake3(données AVANT compression) — jamais après |

| ✅ | V-17 | ref\_count P-Blob : panic si underflow (checked, jamais `fetch\_sub` aveugle) |

| ✅ | V-18 | Ordre locks respecté : memory < scheduler < inode < PathIndex < EPOCH\_COMMIT |

| ✅ | V-19 | GC ne demande jamais `EPOCH\_COMMIT\_LOCK` — `DeferredDeleteQueue` uniquement |

| ✅ | V-20 | `checked\_add()` pour TOUS les calculs d'offsets disque |

| ✅ | V-21 | `bytes\_written` vérifié == expected\_size après chaque write disque |

| ✅ | V-22 | Chaque page EpochRoot chainée : magic + checksum vérifiés AVANT lecture |

| ✅ | V-23 | PathIndex split = UN SEUL EpochRoot (SPLIT-02) |



---



\## 16. Templates de Prompts Copilot



\### 16.1 Prompt Génération — À coller EN TÊTE



```

=== RÈGLES OBLIGATOIRES Exo-OS ExoFS ===



Contexte : Kernel Exo-OS, module fs/exofs/, Ring 0, Rust no\_std.



RÈGLES ABSOLUES (violations = corruption kernel ou faille sécurité) :

1\. no\_std uniquement : use core::... / use alloc::... / crate::...

&nbsp;  INTERDIT : use std::...

2\. Toute allocation Vec → try\_reserve(1)? AVANT push()

3\. Tout unsafe{} → // SAFETY: <raison> obligatoire

4\. Structs on-disk → #\[repr(C)] + types plain + const assert taille

5\. Locks → crate::scheduler::sync::spinlock::SpinLock UNIQUEMENT

6\. DAG : fs/exofs/ n'importe PAS scheduler/ / ipc/ / process/ directement

7\. 3 barrières NVMe dans le commit Epoch (ordre data→root→record)

8\. Vérifier magic EN PREMIER dans tout parsing on-disk

9\. copy\_from\_user() obligatoire pour pointeurs userspace

10\. Buffer per-CPU pour PATH\_MAX — jamais \[u8;4096] sur stack kernel

11\. BlobId = Blake3(données AVANT compression) — jamais après

12\. ref\_count P-Blob : panic si underflow (jamais fetch\_sub aveugle)

13\. GC n'acquiert jamais EPOCH\_COMMIT\_LOCK — DeferredDeleteQueue uniquement

14\. checked\_add() pour TOUS calculs d'offsets disque

15\. PathIndex split = UN SEUL EpochRoot atomique



=== TYPES DÉJÀ DÉFINIS (ne pas redéfinir) ===

\[COLLER ICI le contenu des fichiers déjà générés du bloc courant]



=== DEMANDE ===

\[DÉCRIRE le fichier à générer avec son interface attendue]

```



\### 16.2 Prompt Vérification — Passe 2



```

=== VÉRIFICATION CODE ExoFS — PASSE 2 ===



Analyse le code suivant. Vérifie UNIQUEMENT ces points :



1\. IMPORTS : aucun use std::... ?

2\. OOM-SAFETY : Vec::push() précédé de try\_reserve(1)? ?

3\. UNSAFE : commentaire // SAFETY: sur chaque bloc unsafe ?

4\. ON-DISK : structs #\[repr(C)] + plain + const assert taille ?

5\. LOCKS : crate::scheduler::sync:: uniquement ?

6\. DAG : pas d'import scheduler/ ipc/ process/ direct ?

7\. SÉCURITÉ : verify\_cap() avant accès objet ?

8\. EPOCH : 3 barrières NVMe dans le bon ordre ?

9\. MAGIC : vérification EN PREMIER ?

10\. STACK : pas de \[u8;4096] sur stack kernel ?

11\. BLOBID : Blake3 sur données AVANT compression ?

12\. REFCOUNT : underflow P-Blob protégé (panic) ?

13\. DEADLOCK : GC n'acquiert pas EPOCH\_COMMIT\_LOCK ?

14\. ARITH : checked\_add() pour offsets disque ?

15\. SPLIT : PathIndex split atomique (1 EpochRoot) ?



Format de réponse :

VIOLATION \[règle] ligne N : <description> → CORRECTION : <code corrigé>

OK si aucune violation détectée.



=== CODE À VÉRIFIER ===

\[...]}

```



---

