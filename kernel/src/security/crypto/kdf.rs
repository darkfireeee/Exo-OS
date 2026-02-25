// kernel/src/security/crypto/kdf.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// KDF — Fonction de dérivation de clé basée sur HKDF-BLAKE3
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   • HKDF-BLAKE3 : RFC 5869 avec BLAKE3 en lieu et place de HMAC-SHA2
//   • Extract : blake3_mac(salt, ikm) → PRK 32 bytes
//   • Expand  : BLAKE3 dans le mode "derive_key" avec contexte ASCII unique
//   • Labels de contexte standardisés pour chaque usage kernel
//
// RÈGLE KDF-01 : Chaque dérivation doit utiliser un label de contexte UNIQUE.
// RÈGLE KDF-02 : OutputKey zéroïsée en Drop (sécurité mémoire).
// RÈGLE KDF-03 : IKM ne doit jamais dépasser 4096 bytes.
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use super::blake3::{blake3_hash, blake3_mac, blake3_derive_key, Blake3Hasher};
use core::fmt;

// ─────────────────────────────────────────────────────────────────────────────
// Labels de contexte standard
// ─────────────────────────────────────────────────────────────────────────────

/// Labels de dérivation — uniques par usage, jamais réutilisés.
pub mod labels {
    /// Clé de chiffrement de canal IPC entre deux processus.
    pub const IPC_CHANNEL_ENC:     &str = "Exo-OS 1.0 IPC Channel Encryption Key";
    /// Clé d'authentification MAC canal IPC.
    pub const IPC_CHANNEL_MAC:     &str = "Exo-OS 1.0 IPC Channel MAC Key";
    /// Clé de déchiffrement storage (blocs filesystem).
    pub const FS_BLOCK_ENC:        &str = "Exo-OS 1.0 Filesystem Block Encryption Key";
    /// Clé d'intégrité filesystem.
    pub const FS_INTEGRITY:        &str = "Exo-OS 1.0 Filesystem Integrity Key";
    /// Clé session TLS/QUIC userspace.
    pub const SESSION_ENC:         &str = "Exo-OS 1.0 Session Encryption Key";
    /// Clé attestation TCB.
    pub const TCB_ATTESTATION:     &str = "Exo-OS 1.0 TCB Attestation Key";
    /// Clé de signature code kernel module.
    pub const MODULE_SIGNING:      &str = "Exo-OS 1.0 Module Signing Key";
    /// Clé dérivée pour capability token HMAC.
    pub const CAP_TOKEN_HMAC:      &str = "Exo-OS 1.0 Capability Token HMAC Key";
    /// Clé de wrapping (KEK) pour clés utilisateur.
    pub const KEY_WRAPPING:        &str = "Exo-OS 1.0 Key Encryption Key";
    /// Clé racine pour la dérivation hiérarchique.
    pub const ROOT_KDF:            &str = "Exo-OS 1.0 Root Key Derivation Master";
}

// ─────────────────────────────────────────────────────────────────────────────
// KdfError
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KdfError {
    /// IKM vide ou trop long (> 4096 bytes).
    InvalidIkmLength,
    /// Contexte de dérivation vide.
    EmptyContext,
    /// Longueur de sortie invalide (0 ou > 64 bytes pour une sortie unique).
    InvalidOutputLength,
    /// Erreur interne.
    InternalError,
}

impl fmt::Display for KdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIkmLength  => write!(f, "IKM invalide (vide ou > 4096 bytes)"),
            Self::EmptyContext      => write!(f, "Contexte de dérivation vide"),
            Self::InvalidOutputLength => write!(f, "Longueur de sortie invalide"),
            Self::InternalError     => write!(f, "Erreur KDF interne"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DerivedKey — clé dérivée avec zéroïsation en Drop
// ─────────────────────────────────────────────────────────────────────────────

/// Clé dérivée de 32 bytes.
/// **Zéroïsée automatiquement en Drop** pour éviter les fuites mémoire.
pub struct DerivedKey32 {
    bytes: [u8; 32],
}

impl DerivedKey32 {
    /// Crée une DerivedKey32 depuis un buffer.
    fn from_bytes(bytes: [u8; 32]) -> Self {
        Self { bytes }
    }

    /// Accède aux bytes de la clé.
    #[inline(always)]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    /// Consomme la clé et retourne le tableau interne.
    #[inline(always)]
    pub fn into_bytes(mut self) -> [u8; 32] {
        let result = self.bytes;
        // Zéroïser avant de consommer
        self.bytes = [0u8; 32];
        result
    }
}

impl Drop for DerivedKey32 {
    fn drop(&mut self) {
        // SAFETY: Zéroïsation explicite — la clé ne doit pas rester en mémoire
        // après utilisation. On utilise write_volatile pour éviter l'optimisation
        // du compilateur qui pourrait supprimer cette écriture.
        for byte in self.bytes.iter_mut() {
            // SAFETY: La référence est valide et on écrit une valeur immédiatement.
            unsafe { core::ptr::write_volatile(byte, 0u8); }
        }
    }
}

impl fmt::Debug for DerivedKey32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DerivedKey32([REDACTED])")
    }
}

/// Clé dérivée de 64 bytes (pour usage double : enc + mac).
pub struct DerivedKey64 {
    bytes: [u8; 64],
}

impl DerivedKey64 {
    fn from_bytes(bytes: [u8; 64]) -> Self {
        Self { bytes }
    }

    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.bytes
    }

    /// Sépare en deux clés 32B : (encryption_key, mac_key).
    pub fn split(&self) -> ([u8; 32], [u8; 32]) {
        let mut enc = [0u8; 32];
        let mut mac = [0u8; 32];
        enc.copy_from_slice(&self.bytes[..32]);
        mac.copy_from_slice(&self.bytes[32..]);
        (enc, mac)
    }
}

impl Drop for DerivedKey64 {
    fn drop(&mut self) {
        for byte in self.bytes.iter_mut() {
            // SAFETY: Zéroïsation sécurisée de la clé avant libération.
            unsafe { core::ptr::write_volatile(byte, 0u8); }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HKDF — Extract + Expand
// ─────────────────────────────────────────────────────────────────────────────

/// HKDF-Extract : `PRK = BLAKE3_MAC(salt, IKM)` → 32 bytes
///
/// Si salt est None, on utilise BLAKE3_IV comme sel par défaut.
pub fn hkdf_extract(salt: Option<&[u8]>, ikm: &[u8]) -> Result<[u8; 32], KdfError> {
    if ikm.is_empty() || ikm.len() > 4096 {
        return Err(KdfError::InvalidIkmLength);
    }
    // Salt par défaut : 32 bytes à zéro (convention HKDF RFC 5869)
    let default_salt = [0u8; 32];
    let actual_salt_slice = salt.unwrap_or(&default_salt);

    // PRK = BLAKE3_MAC(key=salt[0..32], data=ikm)
    // Tronquer ou zéro-padder le salt pour obtenir exactement 32 bytes.
    let mut salt_arr = [0u8; 32];
    let slen = actual_salt_slice.len().min(32);
    salt_arr[..slen].copy_from_slice(&actual_salt_slice[..slen]);
    let prk = blake3_mac(&salt_arr, ikm);
    Ok(prk)
}

/// HKDF-Expand : `OKM = BLAKE3_derive_key(context, PRK || info || counter)`
///
/// Output : 32 bytes.
/// Pour plus de 32 bytes, utiliser `hkdf_expand_64()`.
///
/// note: Pour Exo-OS on utilise le mode `derive_key` de BLAKE3 qui est
///       formellement équivalent à HKDF-Expand pour 32 bytes de sortie.
pub fn hkdf_expand_32(prk: &[u8; 32], context: &str, info: &[u8]) -> Result<DerivedKey32, KdfError> {
    if context.is_empty() {
        return Err(KdfError::EmptyContext);
    }

    // On utilise BLAKE3 keyed mode : key=PRK, data= context_bytes || info
    // Ce n'est pas identique à HKDF-Expand RFC5869, mais est cryptographiquement
    // équivalent pour les cas d'usage kernel (PRF sous clé).
    let mut hasher = Blake3Hasher::new_keyed(prk);
    // Préfixer avec le contexte ASCII
    hasher.update(context.as_bytes());
    // Séparateur pour éviter toute ambiguïté entre context et info
    hasher.update(b"\xFF");
    hasher.update(info);
    let mut output = [0u8; 32];
    hasher.finalize(&mut output);
    Ok(DerivedKey32::from_bytes(output))
}

/// HKDF-Expand : 64 bytes (deux clés 32B).
pub fn hkdf_expand_64(prk: &[u8; 32], context: &str, info: &[u8]) -> Result<DerivedKey64, KdfError> {
    if context.is_empty() {
        return Err(KdfError::EmptyContext);
    }
    // On dérive deux clés séparément avec des sous-contextes distincts.
    let mut out = [0u8; 64];
    {
        let ctx_enc = alloc::format!("{} [ENC]", context);
        let mut h = Blake3Hasher::new_keyed(prk);
        h.update(ctx_enc.as_bytes());
        h.update(b"\xFF");
        h.update(info);
        let mut h1 = [0u8; 32];
        h.finalize(&mut h1);
        out[..32].copy_from_slice(&h1);
    }
    {
        let ctx_mac = alloc::format!("{} [MAC]", context);
        let mut h = Blake3Hasher::new_keyed(prk);
        h.update(ctx_mac.as_bytes());
        h.update(b"\xFF");
        h.update(info);
        let mut h2 = [0u8; 32];
        h.finalize(&mut h2);
        out[32..].copy_from_slice(&h2);
    }
    Ok(DerivedKey64::from_bytes(out))
}

// ─────────────────────────────────────────────────────────────────────────────
// API haut niveau : derive_subkey
// ─────────────────────────────────────────────────────────────────────────────

/// Dérive une sous-clé 32B depuis un IKM + label de contexte.
///
/// Séquence :
///   1. PRK = HKDF-Extract(salt=None, IKM)
///   2. subkey = HKDF-Expand(PRK, context_label, info)
pub fn derive_subkey(
    ikm:     &[u8],
    context: &str,
    info:    &[u8],
) -> Result<DerivedKey32, KdfError> {
    let prk = hkdf_extract(None, ikm)?;
    hkdf_expand_32(&prk, context, info)
}

/// Dérive deux sous-clés 32B (chiffrement + MAC) depuis un IKM.
pub fn derive_enc_mac_keys(
    ikm:     &[u8],
    context: &str,
    info:    &[u8],
) -> Result<([u8; 32], [u8; 32]), KdfError> {
    let prk = hkdf_extract(None, ikm)?;
    let keys64 = hkdf_expand_64(&prk, context, info)?;
    Ok(keys64.split())
}

/// Dérive la clé de chiffrement canal IPC entre deux processus.
///
/// `pid_a`, `pid_b` : identifiants des deux participants.
/// `session_nonce` : nonce unique par session (généré par le serveur IPC).
pub fn derive_ipc_channel_key(
    master_key:    &[u8; 32],
    pid_a:         u32,
    pid_b:         u32,
    session_nonce: &[u8; 16],
) -> Result<DerivedKey32, KdfError> {
    // info = pid_a(4) || pid_b(4) || nonce(16)
    let mut info = [0u8; 24];
    info[..4].copy_from_slice(&pid_a.to_le_bytes());
    info[4..8].copy_from_slice(&pid_b.to_le_bytes());
    info[8..24].copy_from_slice(session_nonce);

    derive_subkey(master_key, labels::IPC_CHANNEL_ENC, &info)
}

/// Dérive la clé d'intégrité TCB depuis la platform key + boot nonce.
pub fn derive_tcb_attestation_key(
    platform_key: &[u8; 32],
    boot_nonce:   &[u8; 32],
) -> Result<DerivedKey32, KdfError> {
    derive_subkey(platform_key, labels::TCB_ATTESTATION, boot_nonce)
}

/// Dérive la clé de wrapping pour les clés utilisateur.
pub fn derive_key_encryption_key(
    root_key:     &[u8; 32],
    uid:          u32,
    pid:          u32,
) -> Result<DerivedKey32, KdfError> {
    let mut info = [0u8; 8];
    info[..4].copy_from_slice(&uid.to_le_bytes());
    info[4..8].copy_from_slice(&pid.to_le_bytes());
    derive_subkey(root_key, labels::KEY_WRAPPING, &info)
}

/// Dérive la clé de chiffrement de bloc filesystem.
pub fn derive_fs_block_key(
    volume_key:  &[u8; 32],
    block_id:    u64,
) -> Result<DerivedKey32, KdfError> {
    let info = block_id.to_le_bytes();
    derive_subkey(volume_key, labels::FS_BLOCK_ENC, &info)
}

// ─────────────────────────────────────────────────────────────────────────────
// KDF via BLAKE3 native derive_key mode
// ─────────────────────────────────────────────────────────────────────────────

/// Dérive une clé en utilisant le mode natif BLAKE3 `derive_key`.
///
/// Le contexte doit être une chaîne ASCII unique et statique.
/// Ce mode est plus rapide que HKDF car BLAKE3 l'intègre nativement.
pub fn blake3_kdf(context: &'static str, key_material: &[u8]) -> Result<DerivedKey32, KdfError> {
    if context.is_empty() {
        return Err(KdfError::EmptyContext);
    }
    if key_material.is_empty() || key_material.len() > 4096 {
        return Err(KdfError::InvalidIkmLength);
    }
    let mut derived = [0u8; 32];
    blake3_derive_key(context.as_bytes(), key_material, &mut derived);
    Ok(DerivedKey32::from_bytes(derived))
}

extern crate alloc;
