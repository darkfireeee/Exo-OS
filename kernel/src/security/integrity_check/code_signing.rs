// kernel/src/security/integrity/code_signing.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Code Signing — Vérification de signature Ed25519 pour modules kernel
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   • Chaque module kernel doit être signé par une clé Ed25519 Exo-OS
//   • La clé publique de vérification est embeddée dans le kernel (ROM)
//   • Vérification : Ed25519.verify(module_hash || metadata, signature, pub_key)
//   • La liste des modules chargés est auditée
//
// RÈGLE CSIGN-01 : Un module non signé ne peut JAMAIS être chargé en kernel-space.
// RÈGLE CSIGN-02 : La clé publique maître est en ROM (non modifiable au runtime).
// RÈGLE CSIGN-03 : Chaque module a ses propres métadonnées vérifiées (name, version).
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use super::super::crypto::ed25519::{ed25519_verify, Ed25519Error};
use super::super::crypto::blake3::blake3_hash;

// ─────────────────────────────────────────────────────────────────────────────
// Clé publique maître (embeddée en ROM)
// ─────────────────────────────────────────────────────────────────────────────

/// Clé publique Ed25519 maître pour la vérification des modules Exo-OS.
/// En production : issue de la PKI Exo-OS, générée lors du build sécurisé.
/// Cette valeur est un placeholder cryptographiquement cohérent.
static MASTER_PUBLIC_KEY: [u8; 32] = [
    0x3d, 0x40, 0x17, 0xc3, 0xe8, 0x43, 0x89, 0x5a,
    0x92, 0xb7, 0x0a, 0xa7, 0x4d, 0x1b, 0x7e, 0xbc,
    0x9c, 0x98, 0x2c, 0xcf, 0x2e, 0xc4, 0x96, 0x8c,
    0xc0, 0xcd, 0x55, 0xf1, 0x2a, 0xf4, 0x66, 0x0c,
];

/// Clé publique secondaire pour mise à jour en vol (firmware updates).
static UPDATE_PUBLIC_KEY: [u8; 32] = [
    0xd7, 0x5a, 0x98, 0x01, 0x82, 0x6a, 0x3d, 0x82,
    0x28, 0x34, 0x78, 0xd2, 0x69, 0x0d, 0xd7, 0x73,
    0x68, 0x56, 0x25, 0x85, 0x03, 0x71, 0xb8, 0x6f,
    0x44, 0x23, 0xa5, 0x25, 0xa0, 0x1a, 0xaa, 0x01,
];

// ─────────────────────────────────────────────────────────────────────────────
// Erreurs de code signing
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeSignError {
    /// Signature invalide.
    InvalidSignature,
    /// Hash de module invalide.
    InvalidModuleHash,
    /// Métadonnées corrompues.
    CorruptedMetadata,
    /// Module trop grand (> MAX_MODULE_SIZE).
    ModuleTooLarge,
    /// Clé de vérification inconnue.
    UnknownPublicKey,
    /// Module déjà chargé (anti-replay).
    AlreadyLoaded,
}

// ─────────────────────────────────────────────────────────────────────────────
// ModuleHeader — en-tête de module signé
// ─────────────────────────────────────────────────────────────────────────────

/// En-tête d'un module kernel signé (présent au début du binaire).
#[repr(C)]
pub struct ModuleHeader {
    /// Magic number : b"EXOMOD\xFE\xFF"
    pub magic:       [u8; 8],
    /// Version du format d'en-tête.
    pub version:     u32,
    /// Taille du module (bytes).
    pub module_size: u32,
    /// Nom du module (UTF-8, 64 bytes max).
    pub name:        [u8; 64],
    /// Version sémantique (major.minor.patch)
    pub semver:      [u32; 3],
    /// Hash BLAKE3 du code du module (excluant cet en-tête).
    pub code_hash:   [u8; 32],
    /// Signature Ed25519 de (magic || version || name || semver || code_hash).
    pub signature:   [u8; 64],
    /// Index de clé publique utilisée (0=master, 1=update).
    pub key_index:   u8,
    /// Padding pour alignment 512 bytes.
    pub _pad:        [u8; 3],
}

impl ModuleHeader {
    pub const MAGIC: [u8; 8] = *b"EXOMOD\xFE\xFF";
    pub const SIZE: usize = core::mem::size_of::<ModuleHeader>();

    /// Vérifie le magic number.
    pub fn check_magic(&self) -> bool {
        self.magic == Self::MAGIC
    }

    /// Retourne le nom du module comme slice UTF-8.
    pub fn name_str(&self) -> &[u8] {
        let len = self.name.iter().position(|&b| b == 0).unwrap_or(64);
        &self.name[..len]
    }

    /// Construit les données signées (ce qui est hashé avant la signature).
    fn signed_data(&self) -> [u8; 8+4+64+12+32] {
        let mut data = [0u8; 8+4+64+12+32];
        data[..8].copy_from_slice(&self.magic);
        data[8..12].copy_from_slice(&self.version.to_le_bytes());
        data[12..76].copy_from_slice(&self.name);
        data[76..80].copy_from_slice(&self.semver[0].to_le_bytes());
        data[80..84].copy_from_slice(&self.semver[1].to_le_bytes());
        data[84..88].copy_from_slice(&self.semver[2].to_le_bytes());
        data[88..120].copy_from_slice(&self.code_hash);
        data
    }
}

/// Limite de taille de module kernel (64 MiB).
const MAX_MODULE_SIZE: u32 = 64 * 1024 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// Vérification de module
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie la signature et l'intégrité d'un module kernel.
///
/// - `header` : en-tête du module (pointeur vers le début du binaire)
/// - `code`   : code du module (excluant l'en-tête)
pub fn verify_module_signature(
    header: &ModuleHeader,
    code:   &[u8],
) -> Result<(), CodeSignError> {
    // Vérifier le magic
    if !header.check_magic() {
        SIGN_STATS.failures.fetch_add(1, Ordering::Relaxed);
        return Err(CodeSignError::CorruptedMetadata);
    }

    // Vérifier la taille
    if header.module_size > MAX_MODULE_SIZE {
        SIGN_STATS.failures.fetch_add(1, Ordering::Relaxed);
        return Err(CodeSignError::ModuleTooLarge);
    }

    // Sélectionner la clé publique
    let pub_key = match header.key_index {
        0 => &MASTER_PUBLIC_KEY,
        1 => &UPDATE_PUBLIC_KEY,
        _ => {
            SIGN_STATS.failures.fetch_add(1, Ordering::Relaxed);
            return Err(CodeSignError::UnknownPublicKey);
        }
    };

    // Vérifier le hash du code
    let computed_hash = blake3_hash(code);
    let mut hash_diff = 0u8;
    for i in 0..32 { hash_diff |= computed_hash[i] ^ header.code_hash[i]; }
    if hash_diff != 0 {
        SIGN_STATS.failures.fetch_add(1, Ordering::Relaxed);
        return Err(CodeSignError::InvalidModuleHash);
    }

    // Vérifier la signature Ed25519
    let signed_data = header.signed_data();
    ed25519_verify(&signed_data, &header.signature, pub_key)
        .map_err(|e| match e {
            Ed25519Error::InvalidSignature => CodeSignError::InvalidSignature,
            Ed25519Error::InvalidPublicKey => CodeSignError::UnknownPublicKey,
            _ => CodeSignError::InvalidSignature,
        })?;

    SIGN_STATS.verifications.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Registre des modules chargés (anti-replay)
// ─────────────────────────────────────────────────────────────────────────────

const MAX_LOADED_MODULES: usize = 64;

struct LoadedModule {
    name_hash: [u8; 32],
    code_hash: [u8; 32],
}

struct ModuleRegistry {
    entries: [Option<LoadedModule>; MAX_LOADED_MODULES],
    count:   usize,
}

impl ModuleRegistry {
    const fn new() -> Self {
        const NONE: Option<LoadedModule> = None;
        Self { entries: [NONE; MAX_LOADED_MODULES], count: 0 }
    }

    fn is_loaded(&self, code_hash: &[u8; 32]) -> bool {
        for entry in self.entries.iter().flatten() {
            let mut diff = 0u8;
            for i in 0..32 { diff |= entry.code_hash[i] ^ code_hash[i]; }
            if diff == 0 { return true; }
        }
        false
    }

    fn register(&mut self, name_hash: [u8; 32], code_hash: [u8; 32]) -> bool {
        if self.count >= MAX_LOADED_MODULES { return false; }
        for slot in self.entries.iter_mut() {
            if slot.is_none() {
                *slot = Some(LoadedModule { name_hash, code_hash });
                self.count += 1;
                return true;
            }
        }
        false
    }
}

static MODULE_REG: spin::Mutex<ModuleRegistry> = spin::Mutex::new(ModuleRegistry::new());

/// Enregistre un module comme chargé (après vérification réussie).
pub fn register_loaded_module(header: &ModuleHeader) -> Result<(), CodeSignError> {
    let name_hash = blake3_hash(header.name_str());
    let mut reg = MODULE_REG.lock();
    if reg.is_loaded(&header.code_hash) {
        return Err(CodeSignError::AlreadyLoaded);
    }
    reg.register(name_hash, header.code_hash);
    SIGN_STATS.modules_loaded.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques
// ─────────────────────────────────────────────────────────────────────────────

struct SignStats {
    verifications: AtomicU64,
    failures:      AtomicU64,
    modules_loaded: AtomicU32,
}

static SIGN_STATS: SignStats = SignStats {
    verifications:  AtomicU64::new(0),
    failures:       AtomicU64::new(0),
    modules_loaded: AtomicU32::new(0),
};

#[derive(Debug, Clone, Copy)]
pub struct CodeSignStats {
    pub verifications:  u64,
    pub failures:       u64,
    pub modules_loaded: u32,
}

pub fn code_sign_stats() -> CodeSignStats {
    CodeSignStats {
        verifications:  SIGN_STATS.verifications.load(Ordering::Relaxed),
        failures:       SIGN_STATS.failures.load(Ordering::Relaxed),
        modules_loaded: SIGN_STATS.modules_loaded.load(Ordering::Relaxed),
    }
}
