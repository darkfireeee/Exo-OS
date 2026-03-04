// kernel/src/security/crypto/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Module crypto — Primitives cryptographiques kernel pour Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Sous-modules :
//   • blake3             : Hash BLAKE3 — 256 bits, mode keyed + derive_key
//   • xchacha20_poly1305 : AEAD XChaCha20-Poly1305
//   • rng                : CSPRNG (RDRAND + BLAKE3 mixing)
//   • kdf                : HKDF-BLAKE3 — dérivation de clés
//   • x25519             : ECDH X25519
//   • ed25519            : Signatures Ed25519
//   • aes_gcm            : AES-256-GCM avec AES-NI
//
// Règles de sécurité (RÈGLE CRYPTO-*) :
//   • CRYPTO-01 : Toutes les primitives sont pure-Rust no_std.
//   • CRYPTO-02 : Aucune crate externe sauf `spin`.
//   • CRYPTO-03 : Les clés sont zéroïsées en Drop quand elles sont wrappées.
//   • CRYPTO-04 : Jamais de réutilisation de nonce/IV.
//   • CRYPTO-05 : constant-time pour toutes les comparaisons de tags/clés.
// ═══════════════════════════════════════════════════════════════════════════════

pub mod blake3;
pub mod xchacha20_poly1305;
pub mod rng;
pub mod kdf;
pub mod x25519;
pub mod ed25519;
pub mod aes_gcm;

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports principaux
// ─────────────────────────────────────────────────────────────────────────────

/// BLAKE3
pub use blake3::{
    Blake3Hasher,
    blake3_hash,
    blake3_mac,
    blake3_derive_key,
    constant_time_eq,
};

/// XChaCha20-Poly1305
pub use xchacha20_poly1305::{
    xchacha20_poly1305_seal,
    xchacha20_poly1305_open,
    TAG_LEN          as XCHACHA20_TAG_SIZE,
    XCHACHA20_NONCE_LEN as XCHACHA20_NONCE_SIZE,
    KEY_LEN          as XCHACHA20_KEY_SIZE,
};

/// RNG
pub use rng::{
    rng_init,
    rng_fill,
    rng_u64,
    rng_u32,
    rng_key32,
    rng_nonce24,
    rng_is_ready,
    rng_stats,
    RngError,
    RngStats,
};

/// KDF
pub use kdf::{
    hkdf_extract,
    hkdf_expand_32,
    hkdf_expand_64,
    derive_subkey,
    derive_enc_mac_keys,
    derive_ipc_channel_key,
    derive_tcb_attestation_key,
    derive_key_encryption_key,
    derive_fs_block_key,
    blake3_kdf,
    DerivedKey32,
    DerivedKey64,
    KdfError,
    // labels: supprimé (module absent du nouveau kdf.rs basé sur crates)
};

/// X25519
pub use x25519::{
    x25519_keypair_from_secret,
    x25519_diffie_hellman,
    X25519Error,
    X25519KeyPair,
};

/// Ed25519
pub use ed25519::{
    ed25519_keypair_from_seed,
    ed25519_sign,
    ed25519_verify,
    Ed25519Error,
    Ed25519KeyPair,
};

/// AES-256-GCM
pub use aes_gcm::{
    aes_gcm_seal,
    aes_gcm_open,
    AesGcmError,
};

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation du sous-système crypto
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise le sous-système cryptographique.
/// Doit être appelé après l'init CPU (RDRAND disponible).
pub fn crypto_init() {
    rng::rng_init();
}
