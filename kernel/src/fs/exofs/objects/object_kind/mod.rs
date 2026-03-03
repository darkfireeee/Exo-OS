// SPDX-License-Identifier: MIT
// ExoFS — object_kind/mod.rs
// Sous-types spécifiques selon le kind d'objet ExoFS.

pub mod blob;
pub mod code;
pub mod config;
pub mod path_index;
pub mod relation;
pub mod secret;

// ── Re-exports blob ─────────────────────────────────────────────────────────────

pub use blob::{
    BlobDescriptor,
    BlobDescriptorDisk,
    BlobCreateParams,
    BlobStats,
    BLOB_DESCRIPTOR_MAGIC,
    BLOB_DESCRIPTOR_VERSION,
    BLOB_MAX_SIZE,
    BLOB_FLAG_COMPRESSED,
    BLOB_FLAG_ENCRYPTED,
    BLOB_FLAG_PINNED,
    BLOB_FLAG_DEDUPLICATED,
    BLOB_FLAG_SEALED,
    blob_verify_content,
    blob_compute_id,
    blob_offset_aligned,
};

// ── Re-exports code ─────────────────────────────────────────────────────────────

pub use code::{
    CodeDescriptor,
    CodeDescriptorDisk,
    CodeValidationResult,
    CodeStats,
    ElfClass,
    ElfMachine,
    CODE_DESCRIPTOR_MAGIC,
    CODE_DESCRIPTOR_VERSION,
    CODE_MAX_SIZE,
    CODE_FLAG_ELF_VERIFIED,
    CODE_FLAG_SIGNATURE_VALID,
    CODE_FLAG_PRIVILEGED,
    CODE_FLAG_TRUSTED,
    code_is_valid,
    validate_elf_header,
};

// ── Re-exports config ───────────────────────────────────────────────────────────

pub use config::{
    ConfigEntry,
    ConfigEntryDisk,
    ConfigStore,
    ConfigStats,
    CONFIG_KEY_LEN,
    CONFIG_VALUE_LEN,
    CONFIG_MAX_ENTRIES,
    CONFIG_MAX_SIZE,
    CONFIG_ENTRY_FLAG_REQUIRED,
    CONFIG_ENTRY_FLAG_READONLY,
    CONFIG_ENTRY_FLAG_DELETED,
    CONFIG_ENTRY_FLAG_SECRET,
};

// ── Re-exports secret ───────────────────────────────────────────────────────────

pub use secret::{
    SecretDescriptor,
    SecretDescriptorDisk,
    SecretAccessRecord,
    SecretCipher,
    SecretStats,
    SECRET_DESCRIPTOR_MAGIC,
    SECRET_DESCRIPTOR_VERSION,
    SECRET_MAX_SIZE,
    SECRET_NONCE_LEN,
    SECRET_AUTH_TAG_LEN,
    secret_flags_valid,
    secret_compute_plaintext_id,
};

// ── Re-exports path_index ───────────────────────────────────────────────────────

pub use path_index::{
    PathIndexEntry,
    PathIndexEntryDisk,
    PathIndexPage,
    PathIndexPageHeader,
    PathIndexStats,
    PATH_INDEX_PAGE_SIZE,
    PATH_INDEX_MAGIC,
    PATH_INDEX_VERSION,
    PATH_NAME_MAX,
    PATH_INDEX_MAX_ENTRIES,
    PATH_ENTRY_FLAG_DELETED,
    PATH_ENTRY_FLAG_SYMLINK,
    PATH_ENTRY_FLAG_MOUNT,
    fnv1a_hash_u64,
};

// ── Re-exports relation ─────────────────────────────────────────────────────────

pub use relation::{
    RelationDescriptor,
    RelationEntryDisk,
    RelationKind,
    RelationFlags,
    RelationTable,
    RelationStats,
    RELATION_TABLE_MAGIC,
    RELATION_TABLE_VERSION,
    RELATION_MAX_COUNT,
};
