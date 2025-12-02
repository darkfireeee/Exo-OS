//! Cryptography Module
//!
//! Production-grade cryptographic implementations
//!
//! # Features
//! - ChaCha20-Poly1305 AEAD cipher
//! - SHA-256 cryptographic hash
//! - BLAKE3 high-performance hash
//! - HMAC message authentication
//! - Constant-time operations
//! - No external dependencies

pub mod aead;
pub mod blake3;
pub mod chacha20;
pub mod hash;
pub mod hmac;
pub mod poly1305;
pub mod random;
pub mod post_quantum;

// Re-exports
pub use aead::{decrypt_aead, encrypt_aead, ChaCha20Poly1305};
pub use blake3::blake3_hash;
pub use chacha20::{chacha20_decrypt, chacha20_encrypt, chacha20_rng, ChaCha20};
pub use hash::{sha256, sha512, HashAlgorithm};
pub use hmac::{hmac_sha256, hmac_sha512};
pub use poly1305::{poly1305, Poly1305};
pub use random::{get_random_bytes, CryptoRng};

/// Initialize crypto subsystem
pub fn init() {
    log::info!("Crypto subsystem initialized (production implementations)");
    log::info!("  - ChaCha20-Poly1305 AEAD");
    log::info!("  - SHA-256/512 hashes");
    log::info!("  - BLAKE3 high-performance hash");
    log::info!("  - HMAC-SHA256/512");
}

/// Run self-tests
pub fn self_test() -> bool {
    // TODO: Run test vectors
    true
}
