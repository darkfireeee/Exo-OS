ExoFS — Référence Complète pour Copilot / IA
Règles absolues · Arborescences · Driver Ring 0/1 · Erreurs silencieuses
Exo-OS · fs/exofs/ · Kernel Ring 0 · no_std · Rust · v2.0
⛔  CE DOCUMENT EST LA LOI. Toute règle ❌ INTERDIT est une violation critique pouvant corrompre le kernel, les données disque, ou créer une faille de sécurité. Aucune exception n'est tolérée.

1. Contexte, Position et Décision Ring 0 / Ring 1
1.1 Ce qu'est ExoFS
ExoFS est le système de fichiers natif d'Exo-OS. Il remplace ext4plus. Il tourne en Ring 0 (kernel, no_std). Il implémente les traits VFS définis dans fs/core/vfs.rs. Il utilise un modèle d'objets typés + capabilities cryptographiques au lieu d'inodes POSIX classiques.

1.2 DAG des dépendances — RÈGLE ABSOLUE
RÈGLE DAG-01 : fs/exofs/ ne peut dépendre QUE de memory/, scheduler/, security/capability/. Tout autre import direct est une violation qui crée des dépendances circulaires.

fs/exofs/  dépend de :
    ├── memory/         →  alloc_frame(), DMA, CoW, NUMA
    ├── scheduler/      →  SpinLock, RwLock, WaitQueue, timers
    ├── security/       →  verify_cap(), CapToken
    │
    ├── INTERDIT : ipc/       (utiliser trait abstrait + injection au boot)
    ├── INTERDIT : process/   (lecture seule via trait, jamais import direct)
    └── INTERDIT : arch/      (accès NVMe via block layer uniquement)

1.3 Décision Driver — Ring 0 vs Ring 1
DÉCISION FONDAMENTALE : Le driver ExoFS est physiquement scindé. Règle : « Si le crash corrompt des données → Ring 0. Si le crash se gère par redémarrage → Ring 1 ».

Ring 0 — fs/exofs/  (MÉCANISMES)	Ring 1 — servers/posix_server/  (POLITIQUE)
✅ Superblock, Epochs, Heap disque ✅ Page cache, writeback thread ✅ CapToken verification ✅ Commit Epoch (3 barrières NVMe) ✅ GC thread (tricolore) ✅ PathIndex (structure kernel) ✅ Syscalls ExoFS 500-518 ✅ inode_emulation (ObjectId→ino_t VFS) ✅ mmap + CoW (page table kernel) ✅ fcntl locks (mécanisme kernel) ✅ Recovery au boot	✅ Traduction path string → ObjectId ✅ stat() → struct stat POSIX ✅ readdir() → struct dirent ✅ Permission mapping rwx → CapToken ✅ errno mapping ExofsError → POSIX ✅ NFS v3/v4 server ✅ exofs-mkfs, exofs-fsck (outils) ❌ JAMAIS : toucher au disque directement ❌ JAMAIS : modifier CapTokens kernel ❌ JAMAIS : tenir un lock kernel pendant I/O réseau

⚠️  ERREUR SPEC Z-AI : posix/ est placé entièrement dans Ring 0. INCORRECT. Scindé en posix_bridge/ (Ring 0, 5 fichiers) + servers/posix_server/ (Ring 1). Voir arborescence Section 2.

⚠️  ERREUR SPEC Z-AI : AtomicU64 dans ExoSuperblock on-disk. INCORRECT. AtomicU64 = bit pattern non-déterministe = checksum Blake3 invalide à chaque boot. Correction : ExoSuperblockDisk (plain u64) + ExoSuperblockInMemory (AtomicU64). Voir Section 5.

2. Arborescence Complète — Z-AI v1.0 Corrigée
Format : chaque fichier sur sa propre ligne, indentation │ ├── └──, commentaire # aligné. Créer tous les fichiers vides immédiatement (structure + mod.rs), puis implémenter le contenu dans l'ordre de la Phase 1.

2.0 Racine du module
kernel/src/fs/exofs/
│
├── mod.rs                     # API publique : exofs_init(), exofs_register_fs()
├── lib.rs                     # Feature flags : #![feature(allocator_api)]
│
├── core/                      # ★ Types fondamentaux — ZÉRO dépendance externe
├── objects/                   # Modèles L-Obj et P-Blob
├── path/                      # Résolution chemins, PathIndex
├── epoch/                     # Commits atomiques, recovery
├── storage/                   # Stockage disque, heap, superblock
├── gc/                        # Garbage collection tricolore
├── dedup/                     # Déduplication content-aware
├── compress/                  # Compression LZ4/Zstd inline
├── crypto/                    # Chiffrement XChaCha20 pour Secrets
├── snapshot/                  # Snapshots natifs (epoch-as-snapshot)
├── relation/                  # Graphe de relations typées
├── quota/                     # Quotas capability-bound
├── syscall/                   # Syscalls 500-518
├── posix_bridge/              # ★ AJOUT : pont VFS Ring 0 (corrige posix/ Z-AI)
├── io/                        # Opérations I/O (read/write/zero-copy)
├── cache/                     # Caches (object/blob/path/extent)
├── recovery/                  # Recovery boot + fsck 4 phases
├── export/                    # Export/Import format EXOAR
├── numa/                      # NUMA awareness
├── observability/             # Métriques, tracing, health
├── audit/                     # Audit trail ring buffer
└── tests/                     # Tests unitaires, intégration, fuzz

2.1 core/ — Types Fondamentaux  (14 fichiers)
kernel/src/fs/exofs/core/
│
├── mod.rs                     # Re-exports pub de tous les types
├── types.rs                   # ObjectId=[u8;32], BlobId=[u8;32], EpochId=u64
│                              #   SnapshotId=u64, DiskOffset=u64, Extent{offset,len}
├── constants.rs               # MAGIC=0x45584F46, SLOT_A=4KB, SLOT_B=8KB
│                              #   HEAP_START=1MB, EPOCH_MAX_OBJECTS=500
├── error.rs                   # ExofsError enum → impl From<ExofsError> for FsError
├── config.rs                  # Configuration boot-time (tailles caches, seuils GC)
├── object_id.rs               # new_class1(), new_class2(), ct_eq() temps constant
├── blob_id.rs                 # Blake3 wrapper — JAMAIS sur données compressées
├── epoch_id.rs                # Monotonic counter, comparison, wrapping check
├── object_kind.rs             # #[repr(u8)] enum ObjectKind
│                              #   { Blob=0, Code=1, Config=2, Secret=3,
│                              #     PathIndex=4, Relation=5 }
├── object_class.rs            # ObjectClass::Class1 (immutable) vs Class2 (CoW)
│                              #   + promotion logic Class1→Class2
├── rights.rs                  # INSPECT_CONTENT=1<<10, SNAPSHOT_CREATE=1<<11
│                              #   RELATION_CREATE=1<<12, GC_TRIGGER=1<<13
├── flags.rs                   # ObjectFlags, ExtentFlags, EpochFlags bitfields
├── version.rs                 # Format version, compatibility check
└── stats.rs                   # Compteurs AtomicU64 — PAS dans structs on-disk

2.2 objects/ — Modèles d'Objets  (17 fichiers)
kernel/src/fs/exofs/objects/
│
├── mod.rs                     # Registry objects, re-exports
├── logical_object.rs          # LogicalObject #[repr(C, align(64))]
│                              #   Cache line 1 (64B, hot path) :
│                              #     id:ObjectId[32], kind:ObjectKind[1]
│                              #     class:ObjectClass[1], flags:ObjectFlags[2]
│                              #     link_count:AtomicU32[4], epoch_last:AtomicU64[8]
│                              #     ref_count:AtomicU32[4], _pad:[u8;12]
│                              #   Cache line 2+ : meta, physical_ref, extents
├── physical_blob.rs           # PhysicalBlob — SÉPARER disk / in-memory
│                              #   PhysicalBlobDisk #[repr(C)] : plain u64
│                              #   PhysicalBlobInMemory : AtomicU32 ref_count
├── physical_ref.rs            # enum PhysicalRef {
│                              #   Unique  { blob_id: BlobId },
│                              #   Shared  { blob_id: BlobId, share_idx: u32,
│                              #             is_writer: bool },
│                              #   Inline  { data: [u8;512], len: u16,
│                              #             checksum: u32 }   // CRC32
│                              # }
├── object_meta.rs             # ObjectMeta : timestamps, mime_type:[u8;64], owner_cap
├── object_kind/
│   ├── mod.rs
│   ├── blob.rs                # ObjectKind::Blob — données génériques
│   ├── code.rs                # ObjectKind::Code — validation ELF avant exec
│   ├── config.rs              # ObjectKind::Config — validation schéma
│   ├── secret.rs              # ObjectKind::Secret — BlobId JAMAIS exposé
│   ├── path_index.rs          # ObjectKind::PathIndex — toujours Class2
│   └── relation.rs            # ObjectKind::Relation — lien typé entre objets
├── extent.rs                  # Extent { offset:DiskOffset, len:u64 }
├── extent_tree.rs             # B+ tree pour extents — read/insert/delete
├── inline_data.rs             # Stockage inline < 512B dans le L-Obj
├── object_builder.rs          # Builder pattern — valide les invariants à la création
├── object_loader.rs           # Lazy loading depuis disque
│                              #   Vérifie ObjectHeader magic+checksum AVANT payload
└── object_cache.rs            # LRU cache objets chauds
                               #   PASSE PAR ObjectTable (règle CACHE-02)

2.3 path/ — Résolution de Chemins  (13 fichiers)
kernel/src/fs/exofs/path/
│
├── mod.rs                     # API résolution chemins
├── resolver.rs                # resolve_path(path:&[u8]) → ObjectId
│                              #   Buffer per-CPU PATH_BUFFERS — PAS [u8;4096] stack
│                              #   Itératif, jamais récursif (règle RECUR-01)
├── path_index.rs              # PathIndex — un par répertoire, toujours Class2
│                              #   On-disk  : sorted array (hash, ObjectId, name_len)
│                              #   In-memory: radix tree pour lookup O(log n)
│                              #
│                              #   PathIndexEntry #[repr(C, packed)] :
│                              #     hash:u64, object_id:ObjectId[32]
│                              #     name_len:u16, kind:ObjectKind[1]
│                              #
│                              #   SplitInfo #[repr(C)] :
│                              #     low_child:ObjectId, high_child:ObjectId
│                              #     threshold:u32
├── path_index_tree.rs         # Radix tree in-memory
├── path_index_split.rs        # Split atomique — UN SEUL EpochRoot (règle SPLIT-02)
├── path_index_merge.rs        # Merge après suppressions (seuil < 4096 entrées)
├── path_component.rs          # Parsing composant : UTF-8, len≤255, pas '/'
├── symlink.rs                 # Résolution symlink : MAX_DEPTH=40, itératif
├── mount_point.rs             # Intégration MOUNT_TABLE du VFS existant
├── namespace.rs               # Path namespaces pour containers
├── canonicalize.rs            # Normalisation /../ et /./ — buffer in-place
├── path_cache.rs              # Dentry cache LRU 10 000 entrées
│                              #   Clé : (parent_oid, name_hash)
└── path_walker.rs             # Iterator-based walking — évite récursion

2.4 epoch/ — Gestion des Epochs  (17 fichiers)
kernel/src/fs/exofs/epoch/
│
├── mod.rs                     # Epoch management API
├── epoch_id.rs                # EpochId : monotonic, wrapping check, comparison
├── epoch_record.rs            # EpochRecord #[repr(C, packed)] — EXACTEMENT 104 bytes
│                              #   magic:u32[4]          = 0x45584F46
│                              #   version:u16[2]
│                              #   flags:u16[2]
│                              #   epoch_id:EpochId[8]   monotone croissant
│                              #   timestamp:u64[8]      TSC au commit
│                              #   root_oid:ObjectId[32] ObjectId de l'EpochRoot
│                              #   root_offset:u64[8]    offset disque EpochRoot
│                              #   prev_slot:u64[8]      offset slot précédent
│                              #   checksum:[u8;32]      Blake3(tout ce qui précède)
│                              #   const _: () = assert!(size_of::<EpochRecord>()==104)
├── epoch_root.rs              # EpochRoot — variable, chainable, multi-pages
│                              #   magic:u32             = 0x45504F43 ("EPOC")
│                              #   epoch_id:EpochId
│                              #   modified_objects: liste (ObjectId, DiskOffset)
│                              #   deleted_objects:  liste ObjectId
│                              #   new_relations:    liste RelationDelta
│                              #   next_page:Option<DiskOffset> inclus dans checksum
│                              #   checksum:[u8;32]  Blake3 de cette page seule
│                              #   ★ CHAQUE page chainée a son propre magic+checksum
├── epoch_root_chain.rs        # Chainement multi-pages — next_page inclus dans checksum
├── epoch_slots.rs             # Slots A/B/C aux offsets FIXES
│                              #   Slot A : offset 4KB
│                              #   Slot B : offset 8KB
│                              #   Slot C : offset disk_size - 4MB
│                              #   FrameFlags::EPOCH_PINNED sur slot inactif
├── epoch_commit.rs            # Protocole 3 barrières NVMe OBLIGATOIRE
│                              #   Phase 1 : write(payload)   → nvme_flush()
│                              #   Phase 2 : write(EpochRoot) → nvme_flush()
│                              #   Phase 3 : write(EpochRecord→slot) → nvme_flush()
├── epoch_commit_lock.rs       # static EPOCH_COMMIT_LOCK: SpinLock<()>
│                              #   UN SEUL commit à la fois — jamais par GC
├── epoch_recovery.rs          # max(epoch_id) parmi slots magic+checksum valides
│                              #   Vérifie EpochRoot pointé (magic+checksum aussi)
├── epoch_gc.rs                # Interface GC → DeferredDeleteQueue
│                              #   JAMAIS EPOCH_COMMIT_LOCK depuis le GC (règle DEAD-01)
├── epoch_barriers.rs          # nvme_flush() wrappé — mockable en tests
├── epoch_checksum.rs          # Blake3 streaming EpochRecord + EpochRoot pages
├── epoch_writeback.rs         # Writeback thread : timer EXOFS_WRITEBACK, group commit
├── epoch_snapshot.rs          # Snapshot = marquer un Epoch comme permanent (~1% coût)
├── epoch_delta.rs             # Delta tracking — objets modifiés dans l'Epoch courant
├── epoch_pin.rs               # EPOCH_PINNED : set/clear avec vérif ref_count
└── epoch_stats.rs             # Histogramme latence commit, throughput

2.5 storage/ — Stockage Disque  (22 fichiers)
kernel/src/fs/exofs/storage/
│
├── mod.rs                     # Storage layer API
├── layout.rs                  # Offsets FIXES — fn sector_for_offset()
│                              #   avec checked_add() obligatoire (règle ARITH-02)
│                              #
│                              #   Offset 0       : ExoSuperblock primaire (4KB)
│                              #   Offset 4KB     : EpochSlot A
│                              #   Offset 8KB     : EpochSlot B
│                              #   Offset 12KB    : ExoSuperblock miroir
│                              #   Offset 1MB     : Heap général (blobs, objets)
│                              #   Offset size-8KB: EpochSlot C
│                              #   Offset size-4KB: ExoSuperblock miroir final
├── superblock.rs              # ★ CORRECTION Z-AI : séparer disk / in-memory
│                              #
│                              #   ExoSuperblockDisk #[repr(C, align(4096))]
│                              #     TYPES PLAIN UNIQUEMENT (pas AtomicU64) :
│                              #     magic:u32, version_major:u16, version_minor:u16
│                              #     incompat_flags:u64, compat_flags:u64
│                              #     disk_size_bytes:u64, heap_start:u64
│                              #     heap_end:u64, slot_a_offset:u64
│                              #     slot_b_offset:u64, slot_c_offset:u64
│                              #     created_at:u64, uuid:[u8;16]
│                              #     volume_name:[u8;64], block_size:u32
│                              #     object_count:u64,  ← plain u64, pas AtomicU64
│                              #     blob_count:u64, free_bytes:u64
│                              #     epoch_current:u64
│                              #     checksum:[u8;32]   Blake3(tout ce qui précède)
│                              #
│                              #   ExoSuperblockInMemory :
│                              #     disk: ExoSuperblockDisk
│                              #     object_count: AtomicU64  ← compteur live
│                              #     free_bytes:   AtomicU64
│                              #     dirty:        AtomicBool
│                              #
│                              #   IMPLÉMENTE VfsSuperblock → root_inode() FONCTIONNEL
│                              #   read_and_verify() : magic EN PREMIER, puis checksum
├── superblock_backup.rs       # Miroirs offset 12KB + size-4KB — cross-validation
├── heap.rs                    # Heap allocator : append-only objets, buddy metadata
├── heap_allocator.rs          # Buddy implementation
│                              #   checked_add() pour tous calculs (règle ARITH-02)
├── heap_free_map.rs           # Bitmap blocs libres — atomic updates
├── heap_coalesce.rs           # Coalescing blocs libres adjacents
├── object_writer.rs           # Write L-Obj : ObjectHeader + payload
│                              #   Vérifie bytes_written == expected (règle WRITE-02)
├── object_reader.rs           # Read L-Obj : vérifie ObjectHeader magic+checksum
│                              #   AVANT d'accéder au payload (règle HDR-03)
├── blob_writer.rs             # Write P-Blob : BlobId calculé AVANT compression
│                              #   Pipeline : données → Blake3(BlobId)
│                              #            → compression → chiffrement → disque
├── blob_reader.rs             # Read P-Blob : déchiffrement → décompression
├── extent_writer.rs           # Write extent trees — atomique dans même Epoch
├── extent_reader.rs           # Read extent trees
├── checksum_writer.rs         # Blake3 streaming sur contenu brut (avant compression)
├── checksum_reader.rs         # Vérification Blake3 — Err(Corrupt) si invalide
├── compression_writer.rs      # LZ4/Zstd APRÈS calcul BlobId (règle HASH-02)
├── compression_reader.rs      # Décompression APRÈS vérification checksum
├── compression_choice.rs      # Auto-sélection : text→Zstd, media→None, data→Lz4
├── dedup_writer.rs            # Lookup BlobId avant écriture — réutilise si trouvé
├── dedup_reader.rs            # Lecture via BlobId partagé
├── block_allocator.rs         # Politique d'allocation — Root Reserve protégée
├── block_cache.rs             # Cache blocs 4KB — LRU avec shrinker
├── io_batch.rs                # Batched I/O — regroupe writes en une Bio
└── storage_stats.rs           # Stats I/O : latences, throughput, erreurs

2.6 gc/ — Garbage Collection  (16 fichiers)
kernel/src/fs/exofs/gc/
│
├── mod.rs                     # GC API
├── gc_state.rs                # State machine : Idle→Scanning→Marking→Sweeping→Idle
├── gc_thread.rs               # Background GC thread
│                              #   JAMAIS EPOCH_COMMIT_LOCK (règle DEAD-01)
├── gc_scheduler.rs            # Déclenchement : espace libre < 20% OU timer 60s
├── tricolor.rs                # Algorithme tri-color : Blanc/Gris/Noir
├── marker.rs                  # Mark phase — itératif avec grey_queue heap
│                              #   grey_queue: Vec<ObjectId> sur le heap (règle RECUR-04)
├── sweeper.rs                 # Sweep phase — blobs ref_count=0 depuis > 2 Epochs
├── reference_tracker.rs       # Comptage références cross-Epoch
├── epoch_scanner.rs           # Scan Epochs — racines GC = EpochRoots valides
├── relation_walker.rs         # Parcours graphe relations — itératif BFS/DFS
├── cycle_detector.rs          # Détection cycles — Tarjan itératif
├── orphan_collector.rs        # Collecte orphelins (inaccessibles depuis racines)
├── blob_gc.rs                 # P-Blob GC : supprime si ref_count=0 ET délai ≥ 2 Epochs
├── blob_refcount.rs           # ref_count avec PANIC sur underflow (règle REFCNT-01)
├── inline_gc.rs               # GC données inline (< 512B dans L-Obj)
├── gc_metrics.rs              # Métriques : objets collectés, durée phases
└── gc_tuning.rs               # Auto-tuning : seuils selon charge système

2.7 dedup/ — Déduplication  (13 fichiers)
kernel/src/fs/exofs/dedup/
│
├── mod.rs                     # Dedup API
├── content_hash.rs            # Blake3 sur contenu brut — AVANT compression
├── chunking.rs                # Dispatch : fixe ou CDC selon taille/type
├── chunker_fixed.rs           # Fixed-size chunks 4KB/8KB (fichiers structurés)
├── chunker_cdc.rs             # Content-Defined Chunking (rolling hash Rabin)
├── chunk_cache.rs             # Cache chunks récents — évite rehash
├── blob_registry.rs           # Registry BlobId → locations disque — kernel-only
├── blob_sharing.rs            # Tracking partage : quels L-Objs partagent quel P-Blob
├── dedup_stats.rs             # Ratio dédup, économies disque, CPU utilisé
├── dedup_policy.rs            # Policy : always / size-threshold(>4KB) / off
├── chunk_index.rs             # Index hash→BlobId — BTreeMap kernel-safe
├── chunk_fingerprint.rs       # Fingerprinting rapide (early reject avant lookup)
├── similarity_detect.rs       # Near-dedup : MinHash pour similarité
└── dedup_api.rs               # API userspace : SYS_EXOFS_GET_CONTENT_HASH (audité)

2.8 compress/ — Compression  (10 fichiers)
kernel/src/fs/exofs/compress/
│
├── mod.rs                     # Compression API
├── algorithm.rs               # #[repr(u8)] enum CompressionAlgo
│                              #   { None=0, Lz4=1, Zstd=2, ZstdMax=3 }
├── lz4_wrapper.rs             # LZ4 bindings no_std
│                              #   Vérifie output_size après compression
├── zstd_wrapper.rs            # Zstd bindings — niveau configurable 1-22
├── compress_writer.rs         # Compression streaming — APRÈS calcul BlobId
├── decompress_reader.rs       # Décompression — APRÈS vérification checksum Blake3
├── compress_stats.rs          # Ratio compression, CPU time, algo utilisé
├── compress_choice.rs         # Auto-sélection par MIME type
├── compress_threshold.rs      # Taille minimum : pas de compression si < 512B
├── compress_header.rs         # Header 8B : algo+original_size — inclus dans checksum
└── compress_benchmark.rs      # Benchmark runtime pour calibrer seuils

2.9 crypto/ — Chiffrement Secrets  (12 fichiers)
kernel/src/fs/exofs/crypto/
│
├── mod.rs                     # Crypto API ExoFS
├── key_derivation.rs          # HKDF : MasterKey → VolumeKey → ObjectKey
├── master_key.rs              # Clé maître : TPM-sealed ou Argon2
├── volume_key.rs              # Clé par volume — dérivée au montage
├── object_key.rs              # Clé par L-Obj Secret : HKDF(volume_key, object_id)
├── xchacha20.rs               # XChaCha20-Poly1305
│                              #   nonce unique par objet — JAMAIS réutilisé
├── secret_writer.rs           # Pipeline : données→Blake3(BlobId)→compress→chiffrer
├── secret_reader.rs           # Pipeline inverse : déchiffrer→décompress→vérifier
├── crypto_shredding.rs        # Suppression sécurisée : oublier ObjectKey
├── key_rotation.rs            # Rotation VolumeKey sans rechiffrement des données
├── key_storage.rs             # Stockage : TPM/sealed en priorité, sinon chiffré PIN
├── entropy.rs                 # Source entropie : RDRAND + TSC pour nonces
└── crypto_audit.rs            # Audit toutes opérations crypto (ring buffer SEC-09)

2.10 snapshot/ — Snapshots  (12 fichiers)
kernel/src/fs/exofs/snapshot/
│
├── mod.rs                     # Snapshot API
├── snapshot.rs                # struct Snapshot { id, epoch_id, name, created_at }
├── snapshot_create.rs         # mark_epoch_as_snapshot() — coût O(1), un seul flag
├── snapshot_list.rs           # Liste snapshots — depuis EpochRoot flags
├── snapshot_mount.rs          # Monte snapshot en read-only via VFS
├── snapshot_delete.rs         # Supprime snapshot → déclenche GC blobs exclusifs
├── snapshot_protect.rs        # Protège snapshot de la suppression (TTL)
├── snapshot_quota.rs          # Quota espace snapshots
├── snapshot_diff.rs           # Diff entre 2 snapshots — compare EpochRoots
├── snapshot_restore.rs        # Restauration depuis snapshot (nouveau Epoch)
├── snapshot_streaming.rs      # Export incrémental streaming
└── snapshot_gc.rs             # GC snapshot-aware : préserve blobs référencés

2.11 relation/ — Graphe de Relations  (11 fichiers)
kernel/src/fs/exofs/relation/
│
├── mod.rs                     # Relation API
├── relation.rs                # struct Relation { id, source, target, kind, epoch }
├── relation_type.rs           # enum RelationType
│                              #   { DependsOn, DerivedFrom, Symlink,
│                              #     HardLink, Custom(u32) }
├── relation_graph.rs          # Graphe in-memory — itératif (règle RECUR-01)
├── relation_index.rs          # Index par source / par target
├── relation_walker.rs         # BFS/DFS itératif — stack sur heap (règle RECUR-04)
├── relation_query.rs          # API requête : find_by_source(), find_by_target()
├── relation_batch.rs          # Batch insert/delete dans un seul EpochRoot
├── relation_gc.rs             # Participation au GC tricolore
├── relation_cycle.rs          # Tarjan itératif — vérifie avant insertion
└── relation_storage.rs        # Persistance relations sur disque

2.12 quota/ — Quotas  (6 fichiers)
kernel/src/fs/exofs/quota/
│
├── mod.rs                     # Quota API
├── quota_policy.rs            # Quota lié à la capability — pas à l'UID
├── quota_tracker.rs           # Usage tracking par capability
├── quota_enforcement.rs       # ENOSPC si dépassé — vérifié AVANT toute allocation
├── quota_report.rs            # Rapports usage
├── quota_namespace.rs         # Quotas par namespace (containers)
└── quota_audit.rs             # Audit dépassements quota

2.13 syscall/ — Syscalls ExoFS 500-518  (20 fichiers)
kernel/src/fs/exofs/syscall/
│
├── mod.rs                     # register_exofs_syscalls() → table syscall kernel
├── path_resolve.rs            # SYS_EXOFS_PATH_RESOLVE    (500)
│                              #   Buffer per-CPU PATH_BUFFERS (règle PATH-07)
│                              #   copy_from_user() obligatoire (règle SYS-01)
├── object_open.rs             # SYS_EXOFS_OBJECT_OPEN     (501)
├── object_read.rs             # SYS_EXOFS_OBJECT_READ     (502)
├── object_write.rs            # SYS_EXOFS_OBJECT_WRITE    (503)
├── object_create.rs           # SYS_EXOFS_OBJECT_CREATE   (504)
├── object_delete.rs           # SYS_EXOFS_OBJECT_DELETE   (505)
├── object_stat.rs             # SYS_EXOFS_OBJECT_STAT     (506)
├── object_set_meta.rs         # SYS_EXOFS_OBJECT_SET_META (507)
├── get_content_hash.rs        # SYS_EXOFS_GET_CONTENT_HASH(508) — audité SEC-09
├── snapshot_create.rs         # SYS_EXOFS_SNAPSHOT_CREATE (509)
├── snapshot_list.rs           # SYS_EXOFS_SNAPSHOT_LIST   (510)
├── snapshot_mount.rs          # SYS_EXOFS_SNAPSHOT_MOUNT  (511)
├── relation_create.rs         # SYS_EXOFS_RELATION_CREATE (512)
├── relation_query.rs          # SYS_EXOFS_RELATION_QUERY  (513)
├── gc_trigger.rs              # SYS_EXOFS_GC_TRIGGER      (514)
├── quota_query.rs             # SYS_EXOFS_QUOTA_QUERY     (515)
├── export_object.rs           # SYS_EXOFS_EXPORT_OBJECT   (516)
├── import_object.rs           # SYS_EXOFS_IMPORT_OBJECT   (517)
├── epoch_commit.rs            # SYS_EXOFS_EPOCH_COMMIT    (518)
└── validation.rs              # copy_from_user() helpers, bounds checks
                               #   Utilisé par TOUS les autres syscalls

2.14 posix_bridge/ — Pont VFS Ring 0  (★ Correction Z-AI, 5 fichiers)
★ AJOUT non présent dans Z-AI. Remplace posix/ Ring 0. Contient UNIQUEMENT les mécanismes kernel qui touchent directement la page table ou le VFS existant.

kernel/src/fs/exofs/posix_bridge/
│
├── mod.rs                     # Re-exports, enregistrement dans VFS
├── inode_emulation.rs         # ObjectId → ino_t : mapping stable pour VFS existant
│                              #   Le VFS fs/core/vfs.rs en a besoin directement
├── vfs_compat.rs              # Adapte ExofsInodeOps/FileOps → traits VfsSuperblock
│                              #   ★ MILESTONE 1 : root_inode() fonctionnel ici
│                              #   ★ MILESTONE 2 : open/read/write fonctionnels ici
├── mmap.rs                    # mmap : promotion Class1→Class2 si MAP_SHARED|PROT_WRITE
│                              #   Touche à la page table → Ring 0 obligatoire
└── fcntl_lock.rs              # fcntl locks : mécanisme kernel, granularité byte-range

2.15 io/ — Opérations I/O  (13 fichiers)
kernel/src/fs/exofs/io/
│
├── mod.rs                     # I/O API
├── reader.rs                  # Read path : L-Obj→extent_tree→page_cache→disque
├── writer.rs                  # Write path : page_cache dirty → commit Epoch (async)
│                              #   NE fait PAS le commit directement (writeback thread)
├── zero_copy.rs               # True zero-copy : DMA → PageTable Ring 3 direct
├── direct_io.rs               # O_DIRECT : bypasse page cache, write synchrone
├── buffered_io.rs             # Buffered I/O standard via page_cache existant
├── async_io.rs                # Async I/O via callbacks bio_completion
├── io_uring.rs                # io_uring support — submission queue kernel-side
├── scatter_gather.rs          # Scatter-gather pour gros fichiers
├── prefetch.rs                # Préchargement prédictif
├── readahead.rs               # Readahead adaptatif selon pattern d'accès
├── writeback.rs               # Intégration writeback thread existant
├── io_batch.rs                # Regroupement I/Os en Bio unique
└── io_stats.rs                # Statistiques I/O par objet et par type

2.16 cache/ — Caches  (12 fichiers)
kernel/src/fs/exofs/cache/
│
├── mod.rs                     # Cache coordination
├── object_cache.rs            # LogicalObject cache
│                              #   PASSE PAR ObjectTable — jamais bypass (règle CACHE-02)
├── blob_cache.rs              # PhysicalBlob cache — LRU avec shrinker
├── path_cache.rs              # Résolution chemins — LRU 10 000 entrées
├── extent_cache.rs            # Extent trees — hot path lecture
├── metadata_cache.rs          # Metadata (ObjectMeta) cache
├── cache_policy.rs            # Politiques : LRU / LFU / ARC
├── cache_eviction.rs          # Logique éviction
├── cache_pressure.rs          # Réaction à la pression mémoire
├── cache_stats.rs             # Hit/miss ratios par cache
├── cache_warming.rs           # Préchauffage cache au boot
└── cache_shrinker.rs          # Callback memory pressure
                               #   Libère dans l'ordre : blob→path→object

2.17 recovery/ — Recovery et fsck  (13 fichiers)
kernel/src/fs/exofs/recovery/
│
├── mod.rs                     # Recovery API
├── boot_recovery.rs           # Séquence boot : magic→checksum→max(epoch)→verify_root
├── slot_recovery.rs           # Sélection slot A/B/C — vérifie les 3, prend max valide
├── epoch_replay.rs            # Rejoue l'Epoch actif si nécessaire
├── fsck.rs                    # Full check — orchestre les 4 phases
├── fsck_phase1.rs             # Phase 1 : Superblock + miroirs + feature flags
├── fsck_phase2.rs             # Phase 2 : Heap scan — tous ObjectHeaders magic+checksum
├── fsck_phase3.rs             # Phase 3 : Reconstruction graphe L-Obj→P-Blob→extents
├── fsck_phase4.rs             # Phase 4 : Détection orphelins (non-atteints racines)
├── fsck_repair.rs             # Réparations : orphelins→lost+found, tronqués→truncate
├── checkpoint.rs              # Points de reprise recovery
├── recovery_log.rs            # Journal recovery
└── recovery_audit.rs          # Audit opérations recovery

2.18 export/ — Export/Import  (9 fichiers)
kernel/src/fs/exofs/export/
│
├── mod.rs                     # Export/Import API
├── exoar_format.rs            # Format EXOAR : magic, versioning, chunks
├── exoar_writer.rs            # Write archive EXOAR
├── exoar_reader.rs            # Read archive EXOAR — vérif magic+checksum à l'entrée
├── tar_compat.rs              # Compatibilité TAR (lecture uniquement)
├── stream_export.rs           # Export streaming (pipe, réseau)
├── stream_import.rs           # Import streaming
├── incremental_export.rs      # Export incrémental depuis un Epoch de référence
├── metadata_export.rs         # Export métadonnées seules
└── export_audit.rs            # Audit toutes opérations export (données sensibles)

2.19 numa/ — NUMA Awareness  (6 fichiers)
kernel/src/fs/exofs/numa/
│
├── mod.rs
├── numa_placement.rs          # Placement objets selon NUMA node du process owner
├── numa_migration.rs          # Migration entre nodes (background, non-urgent)
├── numa_affinity.rs           # Tracking affinité process
├── numa_stats.rs              # Statistiques NUMA : local vs remote hits
└── numa_tuning.rs             # Auto-tuning seuils migration

2.20 observability/ — Observabilité  (10 fichiers)
kernel/src/fs/exofs/observability/
│
├── mod.rs
├── metrics.rs                 # Compteurs performance (AtomicU64)
├── tracing.rs                 # Tracing opérations (ring buffer non-bloquant)
├── health_check.rs            # Monitoring santé : espace libre, GC lag, commit latency
├── alert.rs                   # Génération alertes (seuils configurables)
├── perf_counters.rs           # Compteurs hardware perf (PMU)
├── latency_histogram.rs       # Distribution latences par opération
├── throughput_tracker.rs      # Débit lecture/écriture
├── space_tracker.rs           # Suivi espace : heap, blobs, metadata
└── debug_interface.rs         # Interface debug/sysrq

2.21 audit/ — Audit Trail  (8 fichiers)
kernel/src/fs/exofs/audit/
│
├── mod.rs
├── audit_log.rs               # Ring buffer non-bloquant — jamais de perte d'événement
├── audit_entry.rs             # struct AuditEntry { ts, op, actor_cap, object_id, result }
├── audit_writer.rs            # Write entrées audit — lock-free
├── audit_reader.rs            # Read entrées audit (userspace via syscall)
├── audit_rotation.rs          # Rotation log (taille max configurable)
├── audit_filter.rs            # Filtrage par opération, acteur, objet
└── audit_export.rs            # Export audit vers EXOAR

2.22 tests/ — Tests  (~20 fichiers)
kernel/src/fs/exofs/tests/
│
├── mod.rs                     # Framework de test kernel
├── unit/
│   ├── object_id_test.rs      # Tests ObjectId : class1, class2, ct_eq
│   ├── blob_id_test.rs        # Tests BlobId : Blake3 avant compression
│   ├── epoch_test.rs          # Tests EpochRecord : taille 104B, checksum
│   ├── path_index_test.rs     # Tests PathIndex : lookup, split atomique
│   ├── dedup_test.rs          # Tests dédup : même contenu = même BlobId
│   ├── compress_test.rs       # Tests compression : BlobId stable
│   ├── crypto_test.rs         # Tests chiffrement : nonce unique
│   └── gc_test.rs             # Tests GC : tricolore, cycles, underflow panic
├── integration/
│   ├── create_read_test.rs    # Create→Read cycle complet
│   ├── epoch_commit_test.rs   # Commit avec 3 barrières NVMe
│   ├── snapshot_test.rs       # Create snapshot + mount read-only
│   ├── recovery_test.rs       # Simulation crash → recovery
│   └── stress_test.rs         # Stress test concurrent
└── fuzz/
    ├── path_resolve_fuzz.rs   # Fuzzing résolution chemins
    ├── epoch_parse_fuzz.rs    # Fuzzing parsing EpochRecord
    └── object_parse_fuzz.rs   # Fuzzing parsing ObjectHeader

2.23 Ring 1 — servers/posix_server/  (★ Correction Z-AI)
Tout ce qui était dans posix/ Ring 0 chez Z-AI et qui relève de la POLITIQUE migre ici. Ce serveur redémarre sans kernel panic si il crashe.

servers/posix_server/src/
│
├── main.rs                    # Point d'entrée Ring 1 — restart automatique si crash
├── path/
│   ├── mod.rs
│   ├── parser.rs              # Parsing path POSIX : /a/b/../c → composants
│   ├── resolver.rs            # Appelle SYS_EXOFS_PATH_RESOLVE → ObjectId
│   └── cache.rs               # Cache côté Ring 1 (évite syscalls redondants)
├── ops/
│   ├── mod.rs
│   ├── open.rs                # open()    → SYS_EXOFS_OBJECT_OPEN
│   ├── read.rs                # read()    → SYS_EXOFS_OBJECT_READ
│   ├── write.rs               # write()   → SYS_EXOFS_OBJECT_WRITE
│   ├── stat.rs                # stat()    → ObjectMeta → struct stat POSIX
│   ├── readdir.rs             # readdir() → PathIndex entries → struct dirent
│   ├── rename.rs              # rename()  → SYS_EXOFS_RENAME (atomique Epoch)
│   ├── link.rs                # link() hard links → Relation HardLink
│   ├── symlink.rs             # symlink() → Relation Symlink
│   ├── xattr.rs               # getxattr/setxattr → ObjectMeta extended
│   └── acl.rs                 # ACL emulation → Rights mapping
├── compat/
│   ├── mod.rs
│   ├── permission.rs          # chmod/chown → Rights bitfield
│   ├── errno.rs               # ExofsError → errno POSIX
│   └── flags.rs               # O_RDONLY, O_CREAT, O_TRUNC → ExoFS flags
└── nfs/
    ├── mod.rs
    ├── v3.rs                  # NFSv3 server — politique réseau pure Ring 1
    └── v4.rs                  # NFSv4 server

3. Règles Fondamentales Rust no_std
3.1 Imports autorisés
Sév.	ID	Règle
✅	NO-STD-01	use core::...  pour primitives (core::sync::atomic, core::mem, core::ptr, core::fmt)
✅	NO-STD-02	use alloc::... pour collections (alloc::vec::Vec, alloc::sync::Arc, alloc::string::String)
✅	NO-STD-03	Locks : use crate::scheduler::sync::{spinlock::SpinLock, rwlock::RwLock} UNIQUEMENT
❌	NO-STD-04	INTERDIT : use std::...  — std n'existe pas en no_std kernel
❌	NO-STD-05	INTERDIT : std::sync::Mutex, std::sync::RwLock, std::thread
❌	NO-STD-06	INTERDIT : println!, eprintln!, print!  — utiliser log_kernel!() kernel
❌	NO-STD-07	INTERDIT : std::collections::HashMap — utiliser BTreeMap ou hash table shardée

3.2 OOM Safety — Allocations
RÈGLE OOM-01 : TOUT code qui alloue dans le kernel DOIT utiliser les variantes fallible (try_). Un panic en OOM est une panne kernel totale.

Sév.	ID	Règle
❌	OOM-01	INTERDIT : Vec::push(x) sans try_reserve — peut paniquer en OOM
✅	OOM-02	OBLIGATOIRE : vec.try_reserve(1).map_err(|_| ExofsError::NoMemory)?; puis vec.push(x)
❌	OOM-03	INTERDIT : Vec::with_capacity(n) sans vérification — utiliser try_with_capacity(n)?
❌	OOM-04	INTERDIT : alloc::vec![a,b,c] en hot path kernel — créer manuellement avec try_reserve
✅	OOM-05	AUTORISÉ : alloc::vec![] pour Vec vides (pas d'allocation initiale)
⚠️	OOM-06	ATTENTION : Box::new(x) en hot path — préférer types stack-allocated ou pools

// ❌ MAUVAIS — panic en OOM
fn add_object(&mut self, obj: LogicalObject) {
    self.objects.push(obj);  // INTERDIT
}

// ✅ BON — fallible, obligatoire
fn add_object(&mut self, obj: LogicalObject) -> Result<(), ExofsError> {
    self.objects.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
    self.objects.push(obj);  // safe après try_reserve
    Ok(())
}

3.3 Unsafe
Sév.	ID	Règle
✅	UNSAFE-01	Tout bloc unsafe{} DOIT avoir // SAFETY: <raison précise> immédiatement au-dessus
❌	UNSAFE-02	INTERDIT : unsafe{} sans commentaire SAFETY — rejet systématique en review
✅	UNSAFE-03	copy_from_user() OBLIGATOIRE pour tout pointeur venant de Ring 1 / userspace
❌	UNSAFE-04	INTERDIT : déréférencer un pointeur userspace sans copy_from_user() — exploit garanti

3.4 Structs on-disk — format physique
Sév.	ID	Règle
✅	ONDISK-01	Structures écrites sur disque : #[repr(C)] ou #[repr(C, packed)] — layout déterministe
✅	ONDISK-02	Types plain uniquement : u32, u64, [u8; N] — PAS AtomicU64, PAS Vec, PAS Arc
❌	ONDISK-03	INTERDIT : AtomicU64 dans struct on-disk — bit pattern non-déterministe → checksum invalide
❌	ONDISK-04	INTERDIT : Vec<T> dans struct on-disk — pas de layout fixe
✅	ONDISK-05	SÉPARER XyzDisk (types plain, on-disk) de XyzInMemory (AtomicU64, en RAM)
✅	ONDISK-06	const assert obligatoire : const _: () = assert!(size_of::<EpochRecord>() == 104)

4. Modèle d'Objets ExoFS — L-Obj / P-Blob
4.1 Séparation Logique / Physique
Principe fondamental : L-Obj = identité stable visible par les applications. P-Blob = contenu physique content-addressed. Cette séparation est NON NÉGOCIABLE et est la base de la déduplication, du CoW, et des snapshots.

Concept	Description
LogicalObject (L-Obj)	Identité stable. ObjectId Classe 1 = Blake3(contenu||owner_cap). Classe 2 = compteur u64 monotone. Possède : owner_cap, generation, droits, lien vers P-Blob.
PhysicalBlob (P-Blob)	Contenu physique. BlobId = Blake3(contenu brut non-compressé). Partagé entre L-Objs (dédup). ref_count:AtomicU32.
PhysicalRef	Enum dans L-Obj : Unique{blob_id}, Shared{blob_id, share_idx}, Inline{data:[u8;512], len:u16} pour petits fichiers.
ObjectHeader	Header universel 64 bytes — magic 0x4F424A45 + Blake3 checksum. Tout objet disque commence par lui.

Sév.	ID	Règle
✅	LOBJ-01	SYS_EXOFS_PATH_RESOLVE retourne TOUJOURS l'ObjectId du L-Obj — jamais le BlobId
❌	LOBJ-02	INTERDIT : exposer BlobId hors kernel sans Rights::INSPECT_CONTENT
❌	LOBJ-03	INTERDIT : ObjectKind::Secret → BlobId jamais exposé même avec INSPECT_CONTENT
❌	LOBJ-04	INTERDIT : mmap writable direct sur objet Class1 immuable (→ promouvoir Class2 d'abord)
✅	LOBJ-05	Comparaison ObjectId en temps constant ct_eq() — résistance timing attacks
✅	LOBJ-06	ObjectId Class1 = Blake3(blob_id || owner_cap) — calculé UNE SEULE FOIS à la création
❌	LOBJ-07	INTERDIT : modifier ObjectId Class2 après création — il est stable à vie

5. Système d'Epochs — Atomicité et Recovery
5.1 Protocole de commit — 3 barrières NVMe
RÈGLE EPOCH-01 (CRITIQUE) : L'ordre des écritures et barrières est INVIOLABLE. Inverser cet ordre = corruption garantie au prochain reboot.

Phase 1 — Écrire les données objet (payload)
   write(payload_data → heap_offset)
   nvme_flush()   ← BARRIÈRE 1 — OBLIGATOIRE

Phase 2 — Écrire l'EpochRoot
   write(EpochRoot → epoch_root_zone)
   nvme_flush()   ← BARRIÈRE 2 — OBLIGATOIRE

Phase 3 — Écrire l'EpochRecord dans le slot
   write(EpochRecord → slot_A ou slot_B ou slot_C)
   nvme_flush()   ← BARRIÈRE 3 — OBLIGATOIRE

Crash entre Phase 1 et 2 → données orphelines, ignorées au recovery
Crash entre Phase 2 et 3 → EpochRoot sans EpochRecord, ignoré
Phase 3 complète          → Epoch valide, recovery O(1) possible

Sév.	ID	Règle
❌	EPOCH-01	INTERDIT : écrire EpochRecord AVANT les données — recovery pointe vers inexistant
❌	EPOCH-02	INTERDIT : omettre une barrière NVMe — reordering disque = corruption silencieuse
✅	EPOCH-03	EPOCH_COMMIT_LOCK : SpinLock obligatoire — un seul commit à la fois
❌	EPOCH-04	INTERDIT : GC thread demande EPOCH_COMMIT_LOCK — deadlock avec writeback (DEAD-01)
✅	EPOCH-05	EpochRoot ≤ 500 objets par Epoch — commit anticipé si dépassé
✅	EPOCH-06	Recovery = max(epoch_id) parmi slots avec magic+checksum valides
✅	EPOCH-07	Chaque page EpochRoot chainée vérifie son propre magic 0x45504F43 + checksum
❌	EPOCH-08	INTERDIT : faire confiance à next_page sans vérifier magic+checksum de la page
✅	EPOCH-09	EPOCH_PINNED sur frames du slot inactif — libéré uniquement au commit suivant
❌	EPOCH-10	INTERDIT : libérer une frame FrameFlags::EPOCH_PINNED — use-after-free garanti

6. PathIndex — Répertoires
Sév.	ID	Règle
✅	PATH-01	SipHash-2-4 avec mount_secret_key:[u8;16] (aléatoire au montage) — anti Hash-DoS
❌	PATH-02	INTERDIT : hash non-keyed pour PathIndex — vulnérable DoS
✅	PATH-03	Collision de hash → comparer le nom COMPLET byte-à-byte pour confirmation
✅	PATH-04	Split automatique si > 8192 entrées — SplitOp atomique dans UN SEUL EpochRoot
❌	PATH-05	INTERDIT : split PathIndex en 2 Epochs séparés — crash mid-split = répertoire mort
❌	PATH-06	INTERDIT : tenir PathIndex lock pendant une opération I/O bloquante
✅	PATH-07	Buffer per-CPU pour PATH_MAX : static PATH_BUFFERS: PerCpu<[u8;4096]>
❌	PATH-08	INTERDIT : let buf = [0u8; 4096] sur la stack kernel — stack overflow silencieux
✅	PATH-09	rename() atomique dans le même EpochRoot — via SYS_EXOFS_RENAME dédié
❌	PATH-10	INTERDIT : rename() = unlink() + link() séparés — non-atomique

7. Sécurité — Capabilities et Zero Trust
Sév.	ID	Règle
✅	SEC-01	TOUT accès à un objet passe par verify_cap(cap, object_id, rights) — sans exception
❌	SEC-02	INTERDIT : accéder à un objet sans vérifier la capability — violation Zero Trust
✅	SEC-03	Vérification O(1) : token.generation == table[object_id].generation
✅	SEC-04	Révocation = increment atomique de generation — tous tokens existants invalides
❌	SEC-05	INTERDIT : réimplémenter verify() hors de security/capability/ — duplication interdite
✅	SEC-06	Rights::INSPECT_CONTENT requis pour SYS_EXOFS_GET_CONTENT_HASH — audité
❌	SEC-07	INTERDIT : exposer BlobId d'un ObjectKind::Secret — même avec INSPECT_CONTENT
✅	SEC-08	Délégation capability : droits_délégués ⊆ droits_délégateur — PROP-3 prouvée Coq
✅	CRYPTO-01	Pipeline crypto obligatoire : données→Blake3(BlobId)→compression→chiffrement→disque
❌	CRYPTO-02	INTERDIT : compresser après chiffrement — ciphertext incompressible
❌	CRYPTO-03	INTERDIT : réutiliser un nonce avec la même clé — violation cryptographique totale
✅	CRYPTO-04	Crypto-shredding : oublier l'ObjectKey = suppression sécurisée sans effacement physique

8. Locks — Hiérarchie et Deadlock Prevention
RÈGLE LOCK-01 (CRITIQUE) : Toujours acquérir les locks dans l'ordre croissant de niveau. Inverser = deadlock garanti.

Ordre strict (du PLUS BAS au PLUS ÉLEVÉ) :

  Niveau 1 : memory/ SpinLocks    (buddy, frame descriptor)
  Niveau 2 : scheduler/ WaitQueue SpinLocks
  Niveau 3 : memory/ PageTable Locks
  Niveau 4 : fs/ Inode RwLock
  Niveau 5 : fs/exofs/ dentry_cache LRU Lock
  Niveau 6 : fs/exofs/ PathIndex RwLock
  Niveau 7 : fs/exofs/ EPOCH_COMMIT_LOCK  ← le plus élevé de fs/

JAMAIS : tenir lock Niveau N et demander lock Niveau < N

Sév.	ID	Règle
✅	LOCK-01	Acquérir les locks dans l'ordre croissant de niveau — toujours
❌	LOCK-02	INTERDIT : tenir PathIndex lock (N6) et demander Inode lock (N4) — deadlock
✅	LOCK-03	Relâcher lock inode AVANT de dormir ou attendre I/O (release-before-sleep)
❌	LOCK-04	INTERDIT : tenir un SpinLock pendant sleep() ou wait() — non-préemptif
❌	LOCK-05	INTERDIT : tenir EPOCH_COMMIT_LOCK pendant I/O disque direct
✅	LOCK-06	GC communique avec writeback via DeferredDeleteQueue lock-free — jamais EPOCH_COMMIT_LOCK

9. Garbage Collection — Règles Critiques
Sév.	ID	Règle
✅	GC-01	DeferredDeleteQueue : délai minimum 2 Epochs avant suppression réelle
✅	GC-02	GC tricolore traverse les Relations — sinon cycles orphelins jamais collectés
✅	GC-03	File grise bornée : MAX_GC_GREY_QUEUE = 1 000 000 — si dépassé, reporter
✅	GC-04	try_reserve() obligatoire pour la file grise — si OOM : Err et reporter
❌	GC-05	INTERDIT : GC bloquant dans le chemin critique d'écriture — toujours background
✅	GC-06	Racines GC = EpochRoots des slots A/B/C valides
❌	GC-07	INTERDIT : collecter P-Blob avec EPOCH_PINNED actif sur ses frames
✅	GC-08	Création P-Blob atomique : alloc + ref_count.store(1) + insert(ObjectTable) — indivisible
❌	GC-09	INTERDIT : créer P-Blob sans ref_count=1 immédiat — GC peut le détruire avant usage

10. Erreurs Silencieuses — 10 Catégories Critiques
Ces erreurs ne provoquent PAS de crash immédiat. Elles corrompent silencieusement ou créent des fuites permanentes. Les plus dangereuses du module.

ID	Cause	Conséquence	Correction obligatoire
ARITH-01	offset + len sans checked_add()	Overflow u64 → écriture offset 0 → superblock écrasé	checked_add(len).ok_or(ExofsError::OffsetOverflow)?  pour TOUT calcul d'adresse disque
WRITE-01	Ignorer bytes_written retourné	Fichier tronqué sans erreur visible	assert!(bytes_written == data.len()) sinon Err(PartialWrite)
REFCNT-01	fetch_sub(1) sur ref_count=0	Wraps à u32::MAX → blob jamais collecté → fuite disque permanente	compare_exchange avec vérification current>0, panic si underflow (bug kernel)
SPLIT-01	Split en 2 Epochs séparés	Crash mid-split → split_marker sans enfants → répertoire inaccessible	Les 2 enfants + mise à jour parent = UN SEUL EpochRoot
CACHE-01	Créer LogicalObject sans ObjectTable	2 instances du même objet en RAM → corruption état	ObjectTable = SEULE source de vérité — tout accès via ObjectTable::get()
RECUR-01	Récursion sur stack kernel (GC, symlinks)	Stack overflow corrompt mémoire voisine silencieusement	Toujours itératif + stack explicite allouée sur le heap
HASH-01	Blake3(données_compressées)	BlobIds différents pour mêmes données → déduplication à 0%	BlobId = Blake3(contenu brut NON-compressé, NON-chiffré) — TOUJOURS
RACE-01	GC voit ref_count=0 pendant création	Blob valide détruit → use-after-free	store(ref_count=1) → barrier → insert(ObjectTable) — séquence atomique
CHAIN-01	Lire next_page sans vérifier magic	Lecture heap arbitraire → interprété comme liste objets → corruption totale	Chaque page chainée : vérifier magic 0x45504F43 + checksum AVANT lecture
DEAD-01	GC demande EPOCH_COMMIT_LOCK	Writeback tient EPOCH_COMMIT_LOCK, attend ObjectTable. GC tient ObjectTable, attend EPOCH_COMMIT_LOCK → kernel figé	GC → DeferredDeleteQueue uniquement. Jamais EPOCH_COMMIT_LOCK depuis le GC

11. Interface VFS et Syscalls
11.1 Traits à implémenter
// storage/superblock.rs :
impl VfsSuperblock for ExofsVfsSuperblock {
    fn root_inode(&self) -> FsResult<InodeRef> { /* OBLIGATOIRE — débloque path_lookup */ }
    fn statfs(&self)     -> FsResult<FsStats>  { /* statistiques */ }
    fn sync_fs(&self, wait: bool) -> FsResult<()> { /* flush + commit Epoch */ }
    fn alloc_inode(&self) -> FsResult<InodeRef> { /* créer L-Obj, wrapper Inode VFS */ }
}

// posix_bridge/vfs_compat.rs :
impl InodeOps for ExofsInodeOps {
    fn lookup(&self, dir: &InodeRef, name: &[u8]) -> FsResult<DentryRef> { /* PathIndex */ }
    fn create(&self, dir: &InodeRef, name: &[u8], ...) -> FsResult<InodeRef> { ... }
}
impl FileOps for ExofsFileOps {
    fn read (&self, fh: &FileHandle, buf: &mut [u8], off: u64) -> FsResult<usize> { ... }
    fn write(&self, fh: &FileHandle, buf: &[u8],     off: u64) -> FsResult<usize> { ... }
    fn fsync(&self, fh: &FileHandle, datasync: bool) -> FsResult<()> { /* commit Epoch */ }
}

Sév.	ID	Règle
❌	VFS-01	INTERDIT : retourner Err(FsError::NotSupported) dans root_inode() — kernel ne peut pas booter
✅	VFS-02	FileOps::fsync() déclenche SYS_EXOFS_EPOCH_COMMIT — commit dur immédiat
✅	VFS-03	FileOps::write() écrit dans page_cache + marque dirty — pas de commit direct
✅	VFS-04	Le writeback thread flush page_cache → commit Epoch périodique
❌	VFS-05	INTERDIT : modifier les traits VfsSuperblock/InodeOps/FileOps existants — uniquement les implémenter

12. Modifications Requises dans les Autres Modules
Modifications MINIMALES pour la Phase 1. Ne rien ajouter de plus pour éviter la sur-ingénierie.

Fichier existant	Modification requise
memory/physical/frame/descriptor.rs	Ajouter dans FrameFlags :   EPOCH_PINNED = 1 << 5  (blocs slot Epoch inactif)   EXOFS_PINNED = 1 << 6  (pages ExoFS non-évictables)
security/capability/rights.rs	Ajouter dans Rights bitflags :   INSPECT_CONTENT  = 1 << 10   SNAPSHOT_CREATE  = 1 << 11   RELATION_CREATE  = 1 << 12   GC_TRIGGER       = 1 << 13
syscall/numbers.rs	SYS_EXOFS_PATH_RESOLVE    = 500 SYS_EXOFS_OBJECT_OPEN     = 501 SYS_EXOFS_OBJECT_READ     = 502 SYS_EXOFS_OBJECT_WRITE    = 503 SYS_EXOFS_OBJECT_CREATE   = 504 SYS_EXOFS_OBJECT_DELETE   = 505 SYS_EXOFS_OBJECT_STAT     = 506 SYS_EXOFS_OBJECT_SET_META = 507 SYS_EXOFS_GET_CONTENT_HASH= 508 SYS_EXOFS_EPOCH_COMMIT    = 518
syscall/table.rs (ou dispatch.rs)	Router syscalls 500-518 vers fs/exofs/syscall/
fs/mod.rs	Ajouter : pub mod exofs; Remplacer : ext4_register_fs() → exofs::exofs_register_fs() Ajouter : static RT_HINT_PROVIDER: AtomicPtr<()> (injection RT bypass)   Le scheduler injecte fn()→bool au boot via ce pointeur
fs/core/vfs.rs	Compléter les stubs qui retournent NotSupported :   fd_read(), open(), close(), write()   Ces fonctions délèguent via FdTable → ExofsFileOps
scheduler/sync/wait_queue.rs	Augmenter EMERGENCY_POOL_SIZE à 96   (+16 pour GC thread + writeback thread ExoFS)
process/core/tcb.rs	Ajouter : exofs_dirty_objects: Vec<ObjectId>   Tracking objets dirty par thread

13. Séquence de Boot ExoFS
// fs/exofs/mod.rs — exofs_init()
pub fn exofs_init() -> Result<(), ExofsError> {

    // Étape 1 : Lecture superblock
    let frame = crate::memory::physical::allocator::alloc_frame(KERNEL)?;
    let sb = storage::superblock::read_and_verify(ROOT_DEV, frame.phys_addr())?;
    sb.verify_magic()?;       // 0x45584F46 EN PREMIER — si invalide : STOP
    sb.verify_checksum()?;    // Blake3 du superblock complet
    sb.verify_version()?;     // version compatible avec ce kernel
    sb.verify_mirrors()?;     // cross-validation avec les 2 miroirs

    // Étape 2 : Recovery slots A/B/C
    let epoch_id = epoch::recovery::recover_active_epoch(ROOT_DEV)?;
    //  → max(epoch_id) parmi slots avec magic+checksum valides
    //  → vérifie l'EpochRoot pointé (magic+checksum de chaque page chainée)

    // Étape 3 : Initialisation caches
    cache::object_cache::init(OBJECT_CACHE_SIZE)?;
    cache::blob_cache::init(BLOB_CACHE_SIZE)?;
    cache::path_cache::init(PATH_CACHE_SIZE)?;

    // Étape 4 : Threads background
    epoch::epoch_writeback::start_thread()?;  // commit Epoch périodique (1ms)
    gc::gc_thread::start_thread()?;           // GC tricolore background

    // Étape 5 : Shrinker memory pressure
    crate::memory::utils::shrinker::register(cache::cache_shrinker::exofs_shrink)?;

    // Étape 6 : Syscalls 500-518
    syscall::register_exofs_syscalls()?;

    // Étape 7 : Mount root
    exofs_register_fs();
    vfs_mount("exofs", ROOT_DEV, "/", MountFlags::default(), "")?;
    //  ← MILESTONE 3 : premier boot ISO

    Ok(())
}

14. Milestones Phase 1 — Ordre d'Implémentation
Milestone	Ce qui est débloqué
M1 — superblock.rs (storage/superblock.rs)	root_inode() retourne Ok(). path_lookup() peut traverser l'arbre. Le VFS peut monter ExoFS. C'est le déblocage de TOUT le reste.
M2 — vfs_compat.rs (posix_bridge/vfs_compat.rs)	InodeOps + FileOps complets. open(), read(), write(), close() fonctionnels. Les syscalls POSIX passent.
M3 — mod.rs (racine exofs/)	exofs_init() complet. Kernel boote sur ExoFS. Premier ISO bootable. Phase 2 peut commencer.

Ordre de création des fichiers : core/ (types) → storage/ (layout) → epoch/record → superblock ★M1 → epoch/slots+recovery → objects/ → path/ → io/ → epoch/commit → vfs_compat ★M2 → syscall/ → mod.rs ★M3

15. Checklist de Vérification — Passe 2 Copilot
Après chaque génération Copilot : vérifier systématiquement chaque point avant d'accepter le code.

Sév.	ID	Règle
✅	V-01	Tous les imports : use core::... ou use alloc::... ou crate::... — aucun use std::
✅	V-02	Toutes les allocations Vec : try_reserve(1)? avant push()
✅	V-03	Tous les blocs unsafe{} : commentaire // SAFETY: présent
✅	V-04	Structs on-disk : #[repr(C)] + types plain (u32, u64, [u8;N]) — pas AtomicU64
✅	V-05	const assert sur toutes les tailles critiques : size_of::<EpochRecord>() == 104
✅	V-06	Locks : crate::scheduler::sync:: uniquement — pas std::sync::
✅	V-07	DAG respecté : pas d'import scheduler/, ipc/, process/ direct
✅	V-08	verify_cap() appelé AVANT tout accès objet
✅	V-09	BlobId jamais exposé sans INSPECT_CONTENT — Secret : jamais
✅	V-10	copy_from_user() pour tout pointeur Ring 1 → Ring 0
✅	V-11	Buffer per-CPU pour PATH_MAX — pas de [u8;4096] sur stack kernel
✅	V-12	3 barrières NVMe dans le bon ordre : data→flush→root→flush→record→flush
✅	V-13	magic 0x45584F46 vérifié EN PREMIER dans tout parsing on-disk
✅	V-14	Recovery : max(epoch_id) parmi slots checksum valides
✅	V-15	Pas de récursion sur stack kernel — itératif + stack heap
✅	V-16	BlobId = Blake3(données AVANT compression) — jamais après
✅	V-17	ref_count P-Blob : panic si underflow (checked, jamais fetch_sub aveugle)
✅	V-18	Ordre locks respecté : memory < scheduler < inode < PathIndex < EPOCH_COMMIT
✅	V-19	GC ne demande jamais EPOCH_COMMIT_LOCK — DeferredDeleteQueue uniquement
✅	V-20	checked_add() pour TOUS les calculs d'offsets disque
✅	V-21	bytes_written vérifié == expected_size après chaque write disque
✅	V-22	Chaque page EpochRoot chainée : magic + checksum vérifiés AVANT lecture
✅	V-23	PathIndex split = UN SEUL EpochRoot (SPLIT-02)
