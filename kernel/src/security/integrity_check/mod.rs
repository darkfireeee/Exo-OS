// kernel/src/security/integrity_check/mod.rs
//
// Module integrity_check — Vérification d'intégrité à plusieurs niveaux
//
// Sous-modules :
//   • code_signing   — Signature Ed25519 des modules kernel
//   • runtime_check  — Hash BLAKE3 périodique de .text/.rodata
//   • secure_boot    — Chaîne de confiance exo-boot → kernel

pub mod code_signing;
pub mod runtime_check;
pub mod secure_boot;

pub use code_signing::{
    verify_module_signature,
    register_loaded_module,
    ModuleHeader,
    CodeSignError,
    code_sign_stats,
};

pub use runtime_check::{
    init_runtime_integrity,
    check_kernel_integrity,
    assert_kernel_integrity,
    integrity_stats,
    IntegrityError,
};

pub use secure_boot::{
    verify_boot_attestation,
    check_chain_of_trust,
    boot_nonce,
    read_pcr,
    extend_pcr,
    is_chain_verified,
    secureboot_stats,
    BootAttestation,
    SecureBootError,
};

/// Initialise le sous-système d'intégrité.
///
/// Ordre : runtime_check doit s'exécuter AVANT tout autre init
/// (les sections .text/.rodata doivent être intactes).
pub fn integrity_init() {
    init_runtime_integrity();
}
