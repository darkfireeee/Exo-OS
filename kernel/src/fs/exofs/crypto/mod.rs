//! Module de chiffrement ExoFS — XChaCha20-Poly1305 + gestion des clés.
//!
//! Architecture :
//! - master_key : clé maître dérivée du volume password.
//! - volume_key : clé de volume chiffrée par la master key.
//! - object_key : clé par-objet dérivée de volume_key + BlobId.
//! - xchacha20   : primitives AEAD.
//! - key_storage : stockage sécurisé des clés en mémoire (zeroize).

#![allow(dead_code)]

pub mod crypto_audit;
pub mod crypto_shredding;
pub mod entropy;
pub mod key_derivation;
pub mod key_rotation;
pub mod key_storage;
pub mod master_key;
pub mod object_key;
pub mod secret_reader;
pub mod secret_writer;
pub mod volume_key;
pub mod xchacha20;

pub use crypto_audit::CryptoAuditLog;
pub use crypto_shredding::CryptoShredder;
pub use entropy::EntropyPool;
pub use key_derivation::{KeyDerivation, DerivedKey};
pub use key_rotation::KeyRotation;
pub use key_storage::KeyStorage;
pub use master_key::MasterKey;
pub use object_key::ObjectKey;
pub use secret_reader::SecretReader;
pub use secret_writer::SecretWriter;
pub use volume_key::VolumeKey;
pub use xchacha20::{XChaCha20Poly1305, Nonce, Tag};
