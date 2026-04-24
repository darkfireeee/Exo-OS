// kernel/src/fs/exofs/objects/mod.rs
//
// ==============================================================================
// Module objects/ -- Objets logiques et physiques ExoFS
// Ring 0 . no_std . Exo-OS
//
// Ce module regroupe l'integralite de la couche objet de ExoFS :
//   * Meta-donnees on-disk et in-memory (ObjectMeta)
//   * Representation physique des donnees (InlineData, Extent, PhysicalBlob)
//   * References physiques polymorphes (PhysicalRef)
//   * Arbre d'extensions B-tree leger (ExtentTree)
//   * Objet logique complet avec cycle de vie (LogicalObject)
//   * Types d'objets specialises (object_kind/)
//   * Constructeur, chargeur, cache (object_builder, object_loader, object_cache)
//
// Conformite : DAG-01 . ONDISK-01 . REFCNT-01 . HASH-01 . LOBJ-01 . HDR-03
// ==============================================================================

// ------------------------------------------------------------------------------
// Declarations de sous-modules
// ------------------------------------------------------------------------------

/// Meta-donnees portables d'un objet (nom, timestamps, UID/GID, xattrs, CRC32).
pub mod object_meta;

/// Donnees inline stockees directement dans le descripteur on-disk.
pub mod inline_data;

/// Extent unique : plage de blocs physiques contigus.
pub mod extent;

/// Blob physique : representation compressee/hashee d'un blob a l'ecriture.
pub mod physical_blob;

/// Reference physique polymorphe : Inline | Blob | Empty.
pub mod physical_ref;

/// Arbre d'extents lightweight (B-tree plat, <= INLINE_EXTENT_COUNT noeuds inline).
pub mod extent_tree;

/// Objet logique : structure on-disk 256 B + in-memory avec cycle de vie.
pub mod logical_object;

/// Types d'objets specialises (Blob, Code, Config, Secret, PathIndex, Relation).
pub mod object_kind;

/// Constructeur d'objets : fabrique type-safe pour tous les kinds.
pub mod object_builder;

/// Chargeur d'objets : lecture depuis le disque via ReadFn injectable (DAG-01).
pub mod object_loader;

/// Cache d'objets LRU avec epinglage et eviction par lot.
pub mod object_cache;

// ------------------------------------------------------------------------------
// Re-exports -- object_meta
// ------------------------------------------------------------------------------

pub use object_meta::{
    crc32_compute, ObjectMeta, ObjectMetaDisk, ObjectMetaStats, XAttrEntry, MODE_DEFAULT_FILE,
};

// ------------------------------------------------------------------------------
// Re-exports -- inline_data
// ------------------------------------------------------------------------------

pub use inline_data::{InlineData, InlineDataDisk, InlineDataStats};

// ------------------------------------------------------------------------------
// Re-exports -- extent
// ------------------------------------------------------------------------------

pub use extent::{ExtentBuilder, ExtentStats, ObjectExtent, ObjectExtentDisk};

// ------------------------------------------------------------------------------
// Re-exports -- physical_blob
// ------------------------------------------------------------------------------

pub use physical_blob::{
    BlobStats as PhysBlobStats, CompressionType, PhysicalBlobInMemory, PhysicalBlobRef,
    PhysicalBlobTable,
};

// ------------------------------------------------------------------------------
// Re-exports -- physical_ref
// ------------------------------------------------------------------------------

pub use physical_ref::{PhysicalRef, PhysicalRefStats};

// ------------------------------------------------------------------------------
// Re-exports -- extent_tree
// ------------------------------------------------------------------------------

pub use extent_tree::{ExtentTree, ExtentTreeStats, INLINE_EXTENT_COUNT};

// ------------------------------------------------------------------------------
// Re-exports -- logical_object
// ------------------------------------------------------------------------------

pub use logical_object::{
    LogicalObject, LogicalObjectDisk, LogicalObjectRef, ObjectVersion, LOGICAL_OBJECT_MAGIC,
    LOGICAL_OBJECT_VERSION,
};

// ------------------------------------------------------------------------------
// Re-exports -- object_kind
// ------------------------------------------------------------------------------

pub use object_kind::{
    blob_compute_id,

    code_is_valid,

    fnv1a_hash_u64,
    secret_compute_plaintext_id,

    BlobCreateParams,
    // Blob
    BlobDescriptor,
    BlobDescriptorDisk,
    BlobStats,
    // Code
    CodeDescriptor,
    CodeDescriptorDisk,
    CodeStats,
    CodeValidationResult,
    // Config
    ConfigEntry,
    ConfigEntryDisk,
    ConfigStats,
    ConfigStore,
    ElfClass,
    ElfMachine,
    // PathIndex
    PathIndexEntry,
    PathIndexEntryDisk,
    PathIndexPage,
    PathIndexPageHeader,
    PathIndexStats,
    RelationDescriptor,
    RelationEntryDisk,
    RelationFlags,
    // Relation
    RelationKind,
    RelationStats,
    RelationTable,
    SecretAccessRecord,
    SecretCipher,
    // Secret
    SecretDescriptor,
    SecretDescriptorDisk,
    SecretStats,
    BLOB_DESCRIPTOR_MAGIC,
    BLOB_MAX_SIZE,
    CODE_MAX_SIZE,
    CONFIG_KEY_LEN,
    CONFIG_MAX_ENTRIES,

    CONFIG_VALUE_LEN,
    PATH_INDEX_MAGIC,
    PATH_INDEX_MAX_ENTRIES,

    RELATION_MAX_COUNT,
};

// ------------------------------------------------------------------------------
// Re-exports -- object_builder
// ------------------------------------------------------------------------------

pub use object_builder::{BuildError, BuildParams, BuildResult, BuildStats, ObjectBuilder};

// ------------------------------------------------------------------------------
// Re-exports -- object_loader
// ------------------------------------------------------------------------------

pub use object_loader::{LoadParams, LoadResult, LoaderStats, ObjectLoader, ReadFn};

// ------------------------------------------------------------------------------
// Re-exports -- object_cache
// ------------------------------------------------------------------------------

pub use object_cache::{ObjectCache, ObjectCacheStats, OBJECT_CACHE_DEFAULT_CAPACITY};
