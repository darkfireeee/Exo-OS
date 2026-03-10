// kernel/src/security/integrity_check/secure_boot.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Secure Boot — Vérification de la chaîne de confiance au démarrage
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   • Chaîne de confiance : UEFI Secure Boot → Exo-Boot → Exo-Kernel
//   • Vérification Ed25519 à chaque niveau
//   • Le bootloader passe les mesures PCR dans la BootInfo
//   • Le kernel vérifie que la BootInfo est signée par la clé Exo-Boot
//   • TPM PCR en dehors de scope (délégué au bootloader exo-boot/)
//
// RÈGLE SECBOOT-01 : Sans vérification de la chaîne de confiance, le kernel
//                    refuse de monter le filesystem racine.
// RÈGLE SECBOOT-02 : Les mesures PCR simulées (sans TPM) sont basées sur BLAKE3.
// RÈGLE SECBOOT-03 : Le kernel ne fait confiance qu'à UNE seule clé bootloader.
// ═══════════════════════════════════════════════════════════════════════════════


use core::sync::atomic::{AtomicBool, Ordering};
use super::super::crypto::ed25519::ed25519_verify;
use super::super::crypto::blake3::blake3_hash;

// ─────────────────────────────────────────────────────────────────────────────
// Clé publique du bootloader Exo-Boot
// ─────────────────────────────────────────────────────────────────────────────

/// Clé publique Ed25519 du bootloader Exo-Boot.
/// Embeddée en ROM — non modifiable au runtime.
static BOOTLOADER_PUBLIC_KEY: [u8; 32] = [
    0xab, 0x3d, 0x18, 0x76, 0x2e, 0x7f, 0x09, 0x4c,
    0xe4, 0xa9, 0x1b, 0x35, 0x78, 0xde, 0x9c, 0x2b,
    0x4f, 0x61, 0xa7, 0x3e, 0xb0, 0x52, 0x94, 0x77,
    0xc1, 0xfa, 0x38, 0x0d, 0x56, 0x9a, 0xc2, 0x6e,
];

// ─────────────────────────────────────────────────────────────────────────────
// BootAttestation — informations passées par le bootloader
// ─────────────────────────────────────────────────────────────────────────────

/// Attestation de boot passée par exo-boot au kernel via BootInfo.
#[repr(C)]
pub struct BootAttestation {
    /// Magic : b"EXOATST\x00"
    pub magic:          [u8; 8],
    /// Version de l'attestation.
    pub version:        u32,
    /// Hash BLAKE3 du kernel chargé (code + data sections, excluant cette struct).
    pub kernel_hash:    [u8; 32],
    /// Hash BLAKE3 de la configuration de boot (cmdline, etc.).
    pub config_hash:    [u8; 32],
    /// Timestamp TSC au moment du boot (mesure d'entropie supplémentaire).
    pub boot_tsc:       u64,
    /// Nonce unique généré par le bootloader.
    pub nonce:          [u8; 16],
    /// Signature Ed25519 de (magic || version || kernel_hash || config_hash || boot_tsc || nonce).
    pub signature:      [u8; 64],
    /// Padding pour alignment.
    pub _pad:           [u8; 4],
}

impl BootAttestation {
    pub const MAGIC: [u8; 8] = *b"EXOATST\x00";

    pub fn check_magic(&self) -> bool {
        self.magic == Self::MAGIC
    }

    /// Retourne les données couvertes par la signature.
    fn signed_data(&self) -> [u8; 8+4+32+32+8+16] {
        let mut d = [0u8; 8+4+32+32+8+16];
        d[..8].copy_from_slice(&self.magic);
        d[8..12].copy_from_slice(&self.version.to_le_bytes());
        d[12..44].copy_from_slice(&self.kernel_hash);
        d[44..76].copy_from_slice(&self.config_hash);
        d[76..84].copy_from_slice(&self.boot_tsc.to_be_bytes());
        d[84..100].copy_from_slice(&self.nonce);
        d
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Erreurs Secure Boot
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum SecureBootError {
    /// Attestation invalide (magic manquant).
    InvalidAttestation,
    /// Signature du bootloader invalide.
    InvalidBootloaderSignature,
    /// Hash kernel ne correspond pas.
    KernelHashMismatch,
    /// Non initialisé.
    NotInitialized,
    /// Chaîne de confiance non vérifiée.
    ChainNotVerified,
}

// ─────────────────────────────────────────────────────────────────────────────
// État du Secure Boot
// ─────────────────────────────────────────────────────────────────────────────

struct SecureBootState {
    /// Attestation de boot vérifiée.
    attestation_verified: bool,
    /// Hash kernel vérifié.
    kernel_hash:          [u8; 32],
    /// Nonce de boot (pour anti-replay).
    boot_nonce:           [u8; 16],
    /// Nombre de tentatives de vérification.
    verify_attempts:      u32,
}

impl SecureBootState {
    const fn new() -> Self {
        Self {
            attestation_verified: false,
            kernel_hash:          [0u8; 32],
            boot_nonce:           [0u8; 16],
            verify_attempts:      0,
        }
    }
}

static SECBOOT_STATE: spin::Mutex<SecureBootState> =
    spin::Mutex::new(SecureBootState::new());
static CHAIN_VERIFIED: AtomicBool = AtomicBool::new(false);
static SECBOOT_ENFORCE: AtomicBool = AtomicBool::new(true);

// ─────────────────────────────────────────────────────────────────────────────
// PCR simulé — mesure d'intégrité sans TPM
// ─────────────────────────────────────────────────────────────────────────────

/// Banque PCR simulée (8 registres × 32 bytes).
struct PcrBank {
    pcr: [[u8; 32]; 8],
}

impl PcrBank {
    const fn new() -> Self { Self { pcr: [[0u8; 32]; 8] } }

    /// Étend un PCR : PCR[i] = BLAKE3(PCR[i] || measurement).
    fn extend(&mut self, index: usize, measurement: &[u8]) {
        if index >= 8 { return; }
        let mut data = [0u8; 32 + 64]; // PCR(32) + measurement(≤64)
        data[..32].copy_from_slice(&self.pcr[index]);
        let mlen = measurement.len().min(64);
        data[32..32+mlen].copy_from_slice(&measurement[..mlen]);
        self.pcr[index] = blake3_hash(&data[..32+mlen]);
    }

    fn read(&self, index: usize) -> Option<&[u8; 32]> {
        if index >= 8 { None } else { Some(&self.pcr[index]) }
    }
}

static PCR_BANK: spin::Mutex<PcrBank> = spin::Mutex::new(PcrBank::new());

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie l'attestation de boot passée par exo-boot.
///
/// Appelé en early boot, avant toute initialisation de driver.
pub fn verify_boot_attestation(attestation: &BootAttestation) -> Result<(), SecureBootError> {
    if !attestation.check_magic() {
        return Err(SecureBootError::InvalidAttestation);
    }

    let signed_data = attestation.signed_data();
    ed25519_verify(&BOOTLOADER_PUBLIC_KEY, &signed_data, &attestation.signature)
        .map_err(|_| SecureBootError::InvalidBootloaderSignature)?;

    // Stocker l'état vérifié
    let mut state = SECBOOT_STATE.lock();
    state.attestation_verified = true;
    state.kernel_hash = attestation.kernel_hash;
    state.boot_nonce  = attestation.nonce;
    state.verify_attempts += 1;

    // Étendre PCR[0] avec le hash kernel
    drop(state);
    PCR_BANK.lock().extend(0, &attestation.kernel_hash);
    PCR_BANK.lock().extend(1, &attestation.config_hash);

    CHAIN_VERIFIED.store(true, Ordering::Release);
    Ok(())
}

/// Vérifie que la chaîne de confiance est établie.
///
/// RÈGLE SECBOOT-01 : Retourne Err si la chaîne n'est pas vérifiée.
pub fn check_chain_of_trust() -> Result<(), SecureBootError> {
    if !CHAIN_VERIFIED.load(Ordering::Acquire) {
        if SECBOOT_ENFORCE.load(Ordering::Relaxed) {
            return Err(SecureBootError::ChainNotVerified);
        }
    }
    Ok(())
}

/// Désactive l'enforcement du Secure Boot (mode debug uniquement).
///
/// **NE PAS utiliser en production.**
pub fn disable_enforcement() {
    SECBOOT_ENFORCE.store(false, Ordering::SeqCst);
}

/// Retourne le nonce de boot (entropie supplémentaire pour le RNG).
pub fn boot_nonce() -> Option<[u8; 16]> {
    let state = SECBOOT_STATE.lock();
    if state.attestation_verified {
        Some(state.boot_nonce)
    } else {
        None
    }
}

/// Lit un registre PCR simulé.
pub fn read_pcr(index: usize) -> Option<[u8; 32]> {
    PCR_BANK.lock().read(index).copied()
}

/// Étend un PCR avec une mesure.
pub fn extend_pcr(index: usize, measurement: &[u8]) {
    PCR_BANK.lock().extend(index, measurement);
}

/// Retourne vrai si la chaîne de confiance est vérifiée.
pub fn is_chain_verified() -> bool {
    CHAIN_VERIFIED.load(Ordering::Acquire)
}

#[derive(Debug, Clone, Copy)]
pub struct SecureBootStats {
    pub chain_verified:   bool,
    pub verify_attempts:  u32,
}

pub fn secureboot_stats() -> SecureBootStats {
    let state = SECBOOT_STATE.lock();
    SecureBootStats {
        chain_verified:  CHAIN_VERIFIED.load(Ordering::Relaxed),
        verify_attempts: state.verify_attempts,
    }
}
