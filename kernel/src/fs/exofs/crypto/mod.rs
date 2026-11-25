//! Module de cryptographie ExoFS (façade de câblage).

pub mod at_rest;
pub mod crypto_audit;
pub mod crypto_shredding;
pub mod entropy;
mod facade;
pub mod key_derivation;
pub mod key_rotation;
pub mod key_storage;
pub mod master_key;
pub mod object_key;
pub mod secret_reader;
pub mod secret_writer;
pub mod volume_key;
pub mod volume_secret;
pub mod xchacha20;

pub use crypto_audit::{AuditEntry, AuditKind, AuditSummary, CryptoAuditLog, AUDIT_LOG};
pub use crypto_shredding::{
    CryptoShredder, NullOverwriter, OverwriteBlob, OverwriteStrategy, ShredPolicy, ShredResult,
    ShredScheduler,
};
pub use entropy::{generate_salt, generate_unique_id, EntropyPool, ENTROPY_POOL};
pub use facade::{
    decrypt_with_key, derive_key_simple, encrypt_with_key, generate_key, run_health_check,
    CryptoConfig, CryptoModule, CryptoStats, HealthCheckResult, CRYPTO_STATS,
};
pub use key_derivation::{DerivedKey, KeyDerivation, KeyPurpose};
pub use key_rotation::{KeyRotation, RotationReason, RotationResult, RotationSchedule};
pub use key_storage::{KeyKind, KeySlotId, KeyStorage, SlotInfo, SlotState, KEY_STORAGE};
pub use master_key::{MasterKey, MasterKeyId, WrappedMasterKey};
pub use object_key::{BlobKeyId, ObjectKey, ObjectKeyPool, RotationPolicy};
pub use secret_reader::{ReadResult, SecretHeader, SecretReader, SECRET_HEADER_SIZE, SECRET_MAGIC};
pub use secret_writer::{
    build_payload, EncryptedBlob, EnvelopeWriter, SealedEnvelope, SecretWriteResult, SecretWriter,
    SecretWriterPool, WriterId,
};
pub use volume_key::{ObjectKeyCache, VolumeId, VolumeKey, WrappedVolumeKey};
pub use xchacha20::{Nonce, Tag, XChaCha20Key, XChaCha20Poly1305};
