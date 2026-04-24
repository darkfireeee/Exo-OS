// kernel/src/fs/exofs/core/constants.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Constantes ExoFS — Magic, offsets disque fixes, limites opérationnelles
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════

// ─────────────────────────────────────────────────────────────────────────────
// Nombres magiques
// ─────────────────────────────────────────────────────────────────────────────

/// Magic ExoFS on-disk : "EXOF" en little-endian u32.
pub const EXOFS_MAGIC: u32 = 0x45584F46;

/// Magic EpochRoot on-disk : "EPOC" en little-endian u32.
pub const EPOCH_ROOT_MAGIC: u32 = 0x45504F43;

/// Magic ObjectHeader on-disk : "OBJE" en little-endian u32.
pub const OBJECT_HEADER_MAGIC: u32 = 0x4F424A45;

// ─────────────────────────────────────────────────────────────────────────────
// Layout disque — offsets FIXES (immuables pour la compatibilité)
// ─────────────────────────────────────────────────────────────────────────────

/// Offset du SuperBlock primaire (octet 0, taille 4 KB).
pub const SB_PRIMARY_OFFSET: u64 = 0;

/// Offset du Slot Epoch A (octet 4 KB).
pub const EPOCH_SLOT_A_OFFSET: u64 = 4 * 1024;

/// Offset du Slot Epoch B (octet 8 KB).
pub const EPOCH_SLOT_B_OFFSET: u64 = 8 * 1024;

/// Offset du SuperBlock miroir (octet 12 KB).
pub const SB_MIRROR_12K_OFFSET: u64 = 12 * 1024;

/// Début du heap général (blobs, objets) : 1 MB.
pub const HEAP_START_OFFSET: u64 = 1 * 1024 * 1024;

/// Slot Epoch C : disk_size - 8 KB (calculé dynamiquement depuis la taille du disque).
pub const EPOCH_SLOT_C_FROM_END: u64 = 8 * 1024;

/// SuperBlock miroir final : disk_size - 4 KB.
pub const SB_MIRROR_END_FROM_END: u64 = 4 * 1024;

/// Taille de la zone EpochSlot en octets (4 KB).
pub const EPOCH_SLOT_SIZE: u64 = 4 * 1024;

/// Taille du SuperBlock on-disk en octets (4 KB, aligné page).
pub const SUPERBLOCK_SIZE: u64 = 4 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// Limites opérationnelles
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum d'objets modifiables dans un seul Epoch (règle EPOCH-05).
pub const EPOCH_MAX_OBJECTS: usize = 500;

/// Profondeur maximum de résolution de symlinks (règle RECUR-01 / symlink.rs).
pub const SYMLINK_MAX_DEPTH: usize = 40;

/// Longueur maximum d'un composant de chemin en octets.
pub const NAME_MAX: usize = 255;

/// Longueur maximum d'un chemin complet en octets.
pub const PATH_MAX: usize = 4096;

/// Nombre d'entrées PathIndex avant split automatique (règle PATH-04).
pub const PATH_INDEX_SPLIT_THRESHOLD: usize = 8192;

/// Nombre d'entrées PathIndex minimum après merge.
pub const PATH_INDEX_MERGE_THRESHOLD: usize = 4096;

/// Taille d'un chunk fixe pour déduplication.
pub const DEDUP_CHUNK_SIZE_FIXED: usize = 4 * 1024;

/// Taille maximale des données inline dans un LogicalObject.
pub const INLINE_DATA_MAX: usize = 512;

/// Délai GC minimum en epochs avant suppression effective d'un P-Blob.
pub const GC_MIN_EPOCH_DELAY: u64 = 2;

/// Taille maximale de la file grise GC (règle GC-03).
pub const GC_MAX_GREY_QUEUE: usize = 1_000_000;

/// Seuil espace libre GC déclenchement automatique (20 %).
pub const GC_FREE_THRESHOLD_PCT: u64 = 20;

/// Intervalle GC timer (secondes).
pub const GC_TIMER_INTERVAL_SECS: u64 = 60;

/// Intervalle writeback timer (millisecondes).
pub const WRITEBACK_INTERVAL_MS: u64 = 1;

/// Capacité du LRU path cache (dentries).
pub const PATH_CACHE_CAPACITY: usize = 10_000;

/// Taille minimum pour compression (règle compress_threshold.rs).
pub const COMPRESS_MIN_SIZE: usize = 512;

/// Alignement des structures on-disk.
pub const DISK_STRUCT_ALIGN: u64 = 64;

/// Taille d'un bloc de stockage (4 KB).
pub const BLOCK_SIZE: u64 = 4 * 1024;

/// Version majeure du format ExoFS.
pub const FORMAT_VERSION_MAJOR: u16 = 1;

/// Version mineure du format ExoFS.
pub const FORMAT_VERSION_MINOR: u16 = 0;

// ─────────────────────────────────────────────────────────────────────────────
// Magic numbers additionnels
// ─────────────────────────────────────────────────────────────────────────────

/// Magic d'un SnapshotRecord on-disk : "SNAP".
pub const SNAPSHOT_RECORD_MAGIC: u32 = 0x534E4150;

/// Magic d'un BlobHeader on-disk : "BLOB".
pub const BLOB_HEADER_MAGIC: u32 = 0x424C4F42;

/// Magic d'un RelationRecord on-disk : "RELA".
pub const RELATION_RECORD_MAGIC: u32 = 0x52454C41;

/// Magic d'un QuotaBlock on-disk : "QUOT".
pub const QUOTA_BLOCK_MAGIC: u32 = 0x51554F54;

/// Magic d'un PathIndexNode on-disk : "PIDX".
pub const PATH_INDEX_MAGIC: u32 = 0x50494458;

/// Magic d'une page journal on-disk : "JRNA".
pub const JOURNAL_PAGE_MAGIC: u32 = 0x4A524E41;

/// Magic d'un EpochCommitSummary on-disk : "ECMS".
pub const EPOCH_COMMIT_SUMMARY_MAGIC: u32 = 0x45434D53;

// ─────────────────────────────────────────────────────────────────────────────
// Tailles de structures on-disk (règle ONDISK-01)
// ─────────────────────────────────────────────────────────────────────────────

/// Taille du SuperBlock on-disk en octets.
pub const SUPERBLOCK_ONDISK_SIZE: usize = 128;

/// Taille d'un EpochSlot on-disk en octets.
pub const EPOCH_SLOT_ONDISK_SIZE: usize = 64;

/// Taille d'un ObjectHeader on-disk en octets.
pub const OBJECT_HEADER_ONDISK_SIZE: usize = 128;

/// Taille d'un BlobHeader on-disk en octets.
pub const BLOB_HEADER_ONDISK_SIZE: usize = 64;

/// Taille d'un SnapshotRecord on-disk en octets.
pub const SNAPSHOT_RECORD_ONDISK_SIZE: usize = 96;

/// Taille d'un EpochCommitSummary on-disk en octets.
pub const EPOCH_COMMIT_SUMMARY_ONDISK_SIZE: usize = 32;

// ─────────────────────────────────────────────────────────────────────────────
// Limites de capacité ExoFS
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum d'objets par volume ExoFS (2^48 ≈ 281 trillions).
pub const MAX_OBJECTS_PER_VOLUME: u64 = 1u64 << 48;

/// Nombre maximum de snapshots par volume.
pub const MAX_SNAPSHOTS: u64 = 65_536;

/// Nombre maximum d'epochs actifs simultanément (ring buffer).
pub const MAX_ACTIVE_EPOCHS: u64 = 8;

/// Nombre maximum de relations par objet (règle REL-05).
pub const MAX_RELATIONS_PER_OBJECT: usize = 256;

/// Longueur maximale d'un nom de snapshot (UTF-8).
pub const SNAPSHOT_NAME_MAX: usize = 128;

/// Taille maximale d'un attribut de quota (valeur).
pub const QUOTA_VALUE_MAX: u64 = u64::MAX / 2;

/// Nombre maximum d'extents par objet avant compaction.
pub const MAX_EXTENTS_PER_OBJECT: usize = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes d'alignement et de granularité
// ─────────────────────────────────────────────────────────────────────────────

/// Alignement des blobs sur le disque (512 octets pour NVMe/SSD).
pub const BLOB_DISK_ALIGN: u64 = 512;

/// Alignement des structures on-disk critiques (cache line = 64 octets).
pub const CRITICAL_STRUCT_ALIGN: u64 = 64;

/// Taille du cache line CPU (pour padding des structs hot-path).
pub const CACHE_LINE_SIZE: usize = 64;

/// Taille d'une page mémoire kernel (4 KiB).
pub const PAGE_SIZE: usize = 4096;

/// Granularité minimale d'allocation disque (1 bloc = 4 KiB).
pub const MIN_ALLOC_GRANULARITY: u64 = BLOCK_SIZE;

/// Taille du stripe de déduplication (64 KiB pour CDC).
pub const DEDUP_STRIPE_SIZE: usize = 64 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// Timeouts et délais operationnels
// ─────────────────────────────────────────────────────────────────────────────

/// Timeout d'un commit epoch en millisecondes avant de le marquer ABORTED.
pub const EPOCH_COMMIT_TIMEOUT_MS: u64 = 5_000;

/// Délai maximum avant writeback forcé d'un objet dirty (en ms).
pub const DIRTY_EXPIRE_MS: u64 = 30_000;

/// Délai de rétention d'un snapshot avant GC eligibilité (en epochs).
pub const SNAPSHOT_MIN_RETENTION_EPOCHS: u64 = 10;

/// Intervalle de vérification d'intégrité en ligne (en secondes).
pub const ONLINE_VERIFY_INTERVAL_SECS: u64 = 3600;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes CRC32C et checksum
// ─────────────────────────────────────────────────────────────────────────────

/// Polynôme CRC32C Castagnoli (réexporté pour documentation).
pub const CRC32C_POLY: u32 = 0x82F63B78;

/// Valeur CRC32C d'un buffer vide (utilité : test).
pub const CRC32C_EMPTY: u32 = 0x0000_0000;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes de compression
// ─────────────────────────────────────────────────────────────────────────────

/// Magic en tête d'un bloc compressé LZ4 ExoFS.
pub const COMPRESS_MAGIC_LZ4: u32 = 0x4C5A3400; // "LZ4\0"

/// Magic en tête d'un bloc compressé Zstd ExoFS.
pub const COMPRESS_MAGIC_ZSTD: u32 = 0x5A535444; // "ZSTD"

/// Taux de compression minimum pour stocker le bloc compressé (%).
/// En dessous: stocker les données brutes.
pub const COMPRESS_MIN_RATIO_PCT: u64 = 10;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes de sécurité
// ─────────────────────────────────────────────────────────────────────────────

/// Longueur de la clé de chiffrement AES-256 en octets.
pub const ENCRYPTION_KEY_SIZE: usize = 32;

/// Longueur du nonce AES-GCM en octets.
pub const ENCRYPTION_NONCE_SIZE: usize = 12;

/// Longueur du tag d'authentification AES-GCM en octets.
pub const ENCRYPTION_TAG_SIZE: usize = 16;

/// Longueur d'un sel de dérivation de clé (HKDF) en octets.
pub const KDF_SALT_SIZE: usize = 32;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes Class1 / Class2 ObjectId
// ─────────────────────────────────────────────────────────────────────────────

/// Marqueur des ObjectId de Classe 2 : les 2 octets de tête = 0xFFFF.
pub const CLASS2_MARKER: u16 = 0xFFFF;

/// Offset dans ObjectId[32] où le compteur Class2 u64 est stocké (octets 2..10).
pub const CLASS2_COUNTER_OFFSET: usize = 2;

/// Longueur du compteur Class2 dans l'ObjectId.
pub const CLASS2_COUNTER_LEN: usize = 8;

// ─────────────────────────────────────────────────────────────────────────────
// Tiers de stockage
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant du tier « chaud » (hot) — SSD NVMe, accès fréquent.
pub const TIER_HOT: u8 = 0;
/// Identifiant du tier « tiède » (warm) — SSD SATA / HDD rapide, accès modéré.
pub const TIER_WARM: u8 = 1;
/// Identifiant du tier « froid » (cold) — HDD / stockage objet, accès rare.
pub const TIER_COLD: u8 = 2;

/// Latence maximale admise pour le tier HOT (microsecondes).
pub const TIER_HOT_MAX_LATENCY_US: u64 = 100;
/// Latence maximale admise pour le tier WARM (microsecondes).
pub const TIER_WARM_MAX_LATENCY_US: u64 = 1_000;
/// Latence maximale admise pour le tier COLD (microsecondes).
pub const TIER_COLD_MAX_LATENCY_US: u64 = 10_000;

/// Nombre de tiers de stockage dans le système.
pub const TIER_COUNT: usize = 3;

// ─────────────────────────────────────────────────────────────────────────────
// Compression
// ─────────────────────────────────────────────────────────────────────────────

/// Ratio de compression minimal acceptable exprimé en pourcent ×10.
///
/// En dessous de ce seuil (ex. 950 = compression < 5%), la compression
/// n'est pas appliquée (surcoût CPU inutile).
pub const COMPRESS_MIN_GAIN_PCT10: u64 = 950;

/// Taille minimale d'un bloc pour déclencher la compression (octets).
pub const COMPRESS_MIN_BLOCK_BYTES: u64 = 4096;

/// Niveau de compression LZ4 par défaut (1 = plus rapide, 16 = meilleure compression).
pub const COMPRESS_LZ4_LEVEL_DEFAULT: u8 = 3;

/// Niveau de compression Zstd par défaut (1..22).
pub const COMPRESS_ZSTD_LEVEL_DEFAULT: u8 = 3;

// ─────────────────────────────────────────────────────────────────────────────
// Seuils de pression GC
// ─────────────────────────────────────────────────────────────────────────────

/// Seuil de pression GC « medium » : nombre de blobs pendants.
pub const GC_PRESSURE_MEDIUM_BLOBS: u64 = 25;
/// Seuil de pression GC « high ».
pub const GC_PRESSURE_HIGH_BLOBS: u64 = 100;
/// Seuil de pression GC « critique » — GC immédiat obligatoire.
pub const GC_PRESSURE_CRITICAL_BLOBS: u64 = 500;

/// Délai minimal entre deux passes GC (secondes).
pub const GC_MIN_EPOCH_DELAY_SECS: u64 = 2;

// ─────────────────────────────────────────────────────────────────────────────
// Limites de taille par ObjectKind
// ─────────────────────────────────────────────────────────────────────────────

/// Taille maximale pour un objet BLOB (256 Gio).
pub const MAX_BLOB_BYTES: u64 = 256 * 1024 * 1024 * 1024;

/// Taille maximale pour un objet CODE (256 Mio).
pub const MAX_CODE_BYTES: u64 = 256 * 1024 * 1024;

/// Taille maximale pour un objet CONFIG (4 Mio).
pub const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

/// Taille maximale pour un objet SECRET (64 Kio).
pub const MAX_SECRET_BYTES: u64 = 64 * 1024;

/// Taille maximale pour un objet PATHINDEX (64 Mio).
pub const MAX_PATHINDEX_BYTES: u64 = 64 * 1024 * 1024;

/// Taille maximale pour un objet RELATION (4 Kio).
pub const MAX_RELATION_BYTES: u64 = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// Limites d'extents par ObjectKind
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximal d'extents pour un objet BLOB.
pub const MAX_BLOB_EXTENTS: u32 = 65536;
/// Nombre maximal d'extents pour un objet CODE.
pub const MAX_CODE_EXTENTS: u32 = 8192;
/// Nombre maximal d'extents pour un objet CONFIG.
pub const MAX_CONFIG_EXTENTS: u32 = 256;
/// Nombre maximal d'extents pour un objet SECRET.
pub const MAX_SECRET_EXTENTS: u32 = 16;
/// Nombre maximal d'extents pour un objet PATHINDEX.
pub const MAX_PATHINDEX_EXTENTS: u32 = 4096;
/// Nombre maximal d'extents pour un objet RELATION.
pub const MAX_RELATION_EXTENTS: u32 = 1;

// ─────────────────────────────────────────────────────────────────────────────
// NUMA
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximal de nœuds NUMA supportés.
pub const NUMA_NODE_MAX: u32 = 8;

/// Stratégie d'affinité NUMA locale (allouer sur le nœud courant de l'appelant).
pub const NUMA_AFFINITY_LOCAL: u8 = 0;

/// Stratégie d'affinité NUMA distante (allouer sur n'importe quel nœud).
pub const NUMA_AFFINITY_ANY: u8 = 1;

/// Stratégie d'affinité NUMA interleaved (répartir entre tous les nœuds).
pub const NUMA_AFFINITY_INTERLEAVED: u8 = 2;

/// Overhead estimé (pct ×10) dû au trafic NUMA inter-nœuds.
pub const NUMA_REMOTE_OVERHEAD_PCT10: u64 = 300; // 30 %

// ─────────────────────────────────────────────────────────────────────────────
// Journal d'audit
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre d'entrées maximales dans l'anneau d'audit en RAM.
pub const AUDIT_RING_SIZE: usize = 1024;

/// Taille en octets d'une entrée d'audit (doit correspondre à RightsAuditEntry).
pub const AUDIT_ENTRY_SIZE: usize = 24;

/// Magic d'identification du log d'audit ExoFS sur disque.
pub const AUDIT_LOG_MAGIC: u64 = 0x4558_4F41_5544_0001;

// ─────────────────────────────────────────────────────────────────────────────
// Export réseau / archive ExoAR
// ─────────────────────────────────────────────────────────────────────────────

/// Numéro magique d'une archive ExoAR (« EXOAR\x00\x00\x01 »).
pub const EXOAR_MAGIC: u64 = 0x4558_4F41_5200_0001;

/// Taille maximale d'un chunk dans une archive ExoAR (4 Mio).
pub const EXOAR_MAX_CHUNK_BYTES: u64 = 4 * 1024 * 1024;

/// Version courante du format d'archive ExoAR.
pub const EXOAR_FORMAT_VERSION: u32 = 1;

/// Taille de l'en-tête ExoAR (fixe, non compressé).
pub const EXOAR_HEADER_SIZE: u32 = 128;

// ─────────────────────────────────────────────────────────────────────────────
// Recovery / fsck
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximal de tentatives de recovery automatique pour un bloc corrompu.
pub const RECOVERY_MAX_RETRIES: u32 = 3;

/// Nombre maximal d'erreurs de checksum tolérées avant de passer le FS en lecture seule.
pub const RECOVERY_CHECKSUM_LIMIT: u32 = 16;

/// Valeur magique vérifiée dans les superblocs de secours.
pub const RECOVERY_SUPERBLOCK_MAGIC: u64 = 0x4578_6F46_5300_CAFE;

/// Délai entre deux tentatives de recovery automatique (millisecondes).
pub const RECOVERY_RETRY_DELAY_MS: u64 = 100;

// ─────────────────────────────────────────────────────────────────────────────
// Snapshots et CoW
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximal de snapshots actifs simultanément sur un même volume.
pub const SNAPSHOT_MAX_COUNT: u32 = 256;

/// Nombre maximal d'epochs retenus pour les snapshots.
pub const SNAPSHOT_MAX_RETAINED_EPOCHS: u64 = 64;

/// Taille minimale d'un objet pour bénéficier du CoW par extent partiel (4 Kio).
pub const COW_PARTIAL_EXTENT_MIN_BYTES: u64 = 4096;

/// Overhead maximal estimé (pct ×10) accepté pour un CoW inline.
/// Au-delà, on bascule en FullCopy ou Deferred.
pub const COW_INLINE_MAX_OVERHEAD_PCT10: u64 = 150; // 15 %

// ─────────────────────────────────────────────────────────────────────────────
// RefCount / GC interne
// ─────────────────────────────────────────────────────────────────────────────

/// Valeur maximale d'un refcount avant saturation (les créations suivantes sont refusées).
pub const REF_COUNT_MAX: u64 = u64::MAX - 1;

/// Valeur sentinelle indiquant qu'un objet est en cours de suppression.
pub const REF_COUNT_DYING: u64 = u64::MAX;

/// Nombre maximal de blobs en attente de GC dans la queue.
pub const GC_QUEUE_MAX_BLOBS: u32 = 65536;

// ─────────────────────────────────────────────────────────────────────────────
// Limites d'inline data
// ─────────────────────────────────────────────────────────────────────────────

/// Taille maximale des données inline dans les métadonnées d'un objet (octets).
pub const INLINE_DATA_MAX_BYTES: usize = 256;

/// Seuil de promotion inline→extent : si les données dépassent ce seuil,
/// elles sont déplacées vers un extent dédié.
pub const INLINE_PROMOTION_THRESHOLD: usize = 128;

// ─────────────────────────────────────────────────────────────────────────────
// Identifiants de composants internes
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant de la couche ExoFS dans les traces kernel.
pub const EXOFS_COMPONENT_ID: u32 = 0xE307_0001;

/// Version du protocole IPC entre l'espace utilisateur et le FS.
pub const EXOFS_IPC_VERSION: u32 = 1;

/// Préfixe ASCII des entrées de log ExoFS (4 octets → u32 LE).
pub const EXOFS_LOG_PREFIX: u32 = 0x4578_4F46; // « ExOF »

// ─────────────────────────────────────────────────────────────────────────────
// Watermarks d'écriture
// ─────────────────────────────────────────────────────────────────────────────

/// Limite basse du writeback : si le nombre de pages dirty descend en-dessous,
/// le writeback peut s'arrêter.
pub const WRITEBACK_LOW_WATERMARK_PAGES: u64 = 64;

/// Limite haute du writeback : au-dessus, une passe forcée est déclenchée.
pub const WRITEBACK_HIGH_WATERMARK_PAGES: u64 = 512;

/// Intervalle de writeback périodique par défaut (millisecondes).
pub const WRITEBACK_DEFAULT_INTERVAL_MS: u64 = 500;
