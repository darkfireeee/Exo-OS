//! signing_key.rs — GÉNÉRÉ par `tools/kernel_signer` (keygen). NE PAS ÉDITER.
//!
//! Clé PUBLIQUE Ed25519 de vérification du kernel, embarquée dans le
//! bootloader et consommée par `exo-verity::verify_image`. La clé PRIVÉE
//! correspondante est dans `.secrets/kernel_signing.seed` (gitignored).
//! Régénérer : `cargo run -p exo-kernel-signer -- keygen --force`.

/// Clé publique de signature kernel (32 octets, Ed25519).
pub const KERNEL_SIGNING_PUBLIC_KEY: [u8; 32] = [
    0x1c, 0xfc, 0xe1, 0x73, 0x4f, 0x9b, 0xc8, 0x76,
    0x66, 0xf4, 0xe5, 0xda, 0x46, 0xa7, 0x03, 0x93,
    0xd0, 0x00, 0xa8, 0x3f, 0x35, 0x58, 0xd1, 0x02,
    0x54, 0xa5, 0xf0, 0x33, 0x6d, 0x6e, 0x2c, 0x8d,
];
