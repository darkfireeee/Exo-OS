//! Module objects/ — Objets logiques et physiques ExoFS (façade de câblage).

pub mod extent;
pub mod extent_tree;
pub mod inline_data;
pub mod logical_object;
pub mod object_builder;
pub mod object_cache;
pub mod object_kind;
pub mod object_loader;
pub mod object_meta;
pub mod physical_blob;
pub mod physical_ref;

pub use extent::{ExtentBuilder, ExtentStats, ObjectExtent, ObjectExtentDisk};
pub use extent_tree::{ExtentTree, ExtentTreeStats, INLINE_EXTENT_COUNT};
pub use inline_data::{InlineData, InlineDataDisk, InlineDataStats};
pub use logical_object::{
    LogicalObject, LogicalObjectDisk, LogicalObjectRef, ObjectVersion, LOGICAL_OBJECT_MAGIC,
    LOGICAL_OBJECT_VERSION,
};
pub use object_builder::{BuildError, BuildParams, BuildResult, BuildStats, ObjectBuilder};
pub use object_cache::{ObjectCache, ObjectCacheStats, OBJECT_CACHE_DEFAULT_CAPACITY};
pub use object_kind::{
    blob_compute_id, code_is_valid, fnv1a_hash_u64, secret_compute_plaintext_id, BlobCreateParams,
    BlobDescriptor, BlobDescriptorDisk, BlobStats, CodeDescriptor, CodeDescriptorDisk, CodeStats,
    CodeValidationResult, ConfigEntry, ConfigEntryDisk, ConfigStats, ConfigStore, ElfClass,
    ElfMachine, PathIndexEntry, PathIndexEntryDisk, PathIndexPage, PathIndexPageHeader,
    PathIndexStats, RelationDescriptor, RelationEntryDisk, RelationFlags, RelationKind,
    RelationStats, RelationTable, SecretAccessRecord, SecretCipher, SecretDescriptor,
    SecretDescriptorDisk, SecretStats, BLOB_DESCRIPTOR_MAGIC, BLOB_MAX_SIZE, CODE_MAX_SIZE,
    CONFIG_KEY_LEN, CONFIG_MAX_ENTRIES, CONFIG_VALUE_LEN, PATH_INDEX_MAGIC,
    PATH_INDEX_MAX_ENTRIES, RELATION_MAX_COUNT,
};
pub use object_loader::{LoadParams, LoadResult, LoaderStats, ObjectLoader, ReadFn};
pub use object_meta::{
    crc32_compute, ObjectMeta, ObjectMetaDisk, ObjectMetaStats, XAttrEntry, MODE_DEFAULT_FILE,
};
pub use physical_blob::{
    BlobStats as PhysBlobStats, CompressionType, PhysicalBlobInMemory, PhysicalBlobRef,
    PhysicalBlobTable,
};
pub use physical_ref::{PhysicalRef, PhysicalRefStats};
