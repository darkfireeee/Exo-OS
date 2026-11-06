// kernel/src/security/crypto/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Crypto module — Kernel cryptographic primitives for Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Submodules:
//   • blake3             : BLAKE3 hash — 256-bit, keyed + derive_key mode
//   • xchacha20_poly1305 : AEAD XChaCha20-Poly1305
//   • rng                : CSPRNG (RDRAND + ChaCha20 block function)
//   • kdf                : HKDF-BLAKE3 — key derivation
//   • x25519             : ECDH X25519
//   • ed25519            : Ed25519 signatures
//   • aes_gcm            : AES-256-GCM (software T-tables + AES-NI runtime detection)
//
// Security rules (RULE CRYPTO-*):
//   • CRYPTO-01: All primitives are pure-Rust no_std.
//   • CRYPTO-02: IETF validated RustCrypto crates only (except custom ChaCha20).
//   • CRYPTO-03: Keys are zeroized on Drop when wrapped.
//   • CRYPTO-04: Never reuse nonce/IV.
//   • CRYPTO-05: Constant-time for all tag/key comparisons.
// ═══════════════════════════════════════════════════════════════════════════════

pub mod aes_gcm;
pub mod blake3;
pub mod ed25519;
pub mod kdf;
pub mod rng;
pub mod x25519;
pub mod xchacha20_poly1305;

// ─────────────────────────────────────────────────────────────────────────────
// Main re-exports
// ─────────────────────────────────────────────────────────────────────────────

/// BLAKE3
pub use blake3::{blake3_derive_key, blake3_hash, blake3_mac, constant_time_eq, Blake3Hasher};

/// XChaCha20-Poly1305
pub use xchacha20_poly1305::{
    xchacha20_poly1305_open, xchacha20_poly1305_seal, xchacha20_xor, KEY_LEN as XCHACHA20_KEY_SIZE,
    TAG_LEN as XCHACHA20_TAG_SIZE, XCHACHA20_NONCE_LEN as XCHACHA20_NONCE_SIZE,
};

/// RNG
pub use rng::{
    rng_fill, rng_init, rng_is_ready, rng_key32, rng_nonce24, rng_stats, rng_u32, rng_u64,
    RngError, RngStats,
};

/// KDF
pub use kdf::{
    blake3_kdf,
    derive_enc_mac_keys,
    derive_fs_block_key,
    derive_ipc_channel_key,
    derive_key_encryption_key,
    derive_subkey,
    derive_tcb_attestation_key,
    hkdf_expand_32,
    hkdf_expand_64,
    hkdf_extract,
    DerivedKey32,
    DerivedKey64,
    KdfError,
    // labels: removed (module absent from new crates-based kdf.rs)
};

/// X25519
pub use x25519::{x25519_diffie_hellman, x25519_keypair_from_secret, X25519Error, X25519KeyPair};

/// Ed25519
pub use ed25519::{
    ed25519_keypair_from_seed, ed25519_sign, ed25519_verify, Ed25519Error, Ed25519KeyPair,
};

/// AES-256-GCM
pub use aes_gcm::{
    aes_gcm_open, aes_gcm_seal, AesGcmError, AES_GCM_NONCE_LEN, AES_GCM_TAG_LEN, AES_KEY_LEN,
};

// ─────────────────────────────────────────────────────────────────────────────
// Cryptographic subsystem initialization
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes the cryptographic subsystem.
/// Must be called after CPU init (RDRAND available).
pub fn crypto_init() {
    rng::rng_init();
}
