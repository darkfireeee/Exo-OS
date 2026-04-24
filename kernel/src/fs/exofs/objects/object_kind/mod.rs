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
    blob_compute_id, blob_offset_aligned, blob_verify_content, BlobCreateParams, BlobDescriptor,
    BlobDescriptorDisk, BlobStats, BLOB_DESCRIPTOR_MAGIC, BLOB_DESCRIPTOR_VERSION,
    BLOB_FLAG_COMPRESSED, BLOB_FLAG_DEDUPLICATED, BLOB_FLAG_ENCRYPTED, BLOB_FLAG_PINNED,
    BLOB_FLAG_SEALED, BLOB_MAX_SIZE,
};

// ── Re-exports code ─────────────────────────────────────────────────────────────

pub use code::{
    code_is_valid, validate_elf_header, CodeDescriptor, CodeDescriptorDisk, CodeStats,
    CodeValidationResult, ElfClass, ElfMachine, CODE_DESCRIPTOR_MAGIC, CODE_DESCRIPTOR_VERSION,
    CODE_FLAG_ELF_VERIFIED, CODE_FLAG_PRIVILEGED, CODE_FLAG_SIGNATURE_VALID, CODE_FLAG_TRUSTED,
    CODE_MAX_SIZE,
};

// ── Re-exports config ───────────────────────────────────────────────────────────

pub use config::{
    ConfigEntry, ConfigEntryDisk, ConfigStats, ConfigStore, CONFIG_ENTRY_FLAG_DELETED,
    CONFIG_ENTRY_FLAG_READONLY, CONFIG_ENTRY_FLAG_REQUIRED, CONFIG_ENTRY_FLAG_SECRET,
    CONFIG_KEY_LEN, CONFIG_MAX_ENTRIES, CONFIG_MAX_SIZE, CONFIG_VALUE_LEN,
};

// ── Re-exports secret ───────────────────────────────────────────────────────────

pub use secret::{
    secret_compute_plaintext_id, secret_flags_valid, SecretAccessRecord, SecretCipher,
    SecretDescriptor, SecretDescriptorDisk, SecretStats, SECRET_AUTH_TAG_LEN,
    SECRET_DESCRIPTOR_MAGIC, SECRET_DESCRIPTOR_VERSION, SECRET_MAX_SIZE, SECRET_NONCE_LEN,
};

// ── Re-exports path_index ───────────────────────────────────────────────────────

pub use path_index::{
    fnv1a_hash_u64, PathIndexEntry, PathIndexEntryDisk, PathIndexPage, PathIndexPageHeader,
    PathIndexStats, PATH_ENTRY_FLAG_DELETED, PATH_ENTRY_FLAG_MOUNT, PATH_ENTRY_FLAG_SYMLINK,
    PATH_INDEX_MAGIC, PATH_INDEX_MAX_ENTRIES, PATH_INDEX_PAGE_SIZE, PATH_INDEX_VERSION,
    PATH_NAME_MAX,
};

// ── Re-exports relation ─────────────────────────────────────────────────────────

pub use relation::{
    RelationDescriptor, RelationEntryDisk, RelationFlags, RelationKind, RelationStats,
    RelationTable, RELATION_MAX_COUNT, RELATION_TABLE_MAGIC, RELATION_TABLE_VERSION,
};
