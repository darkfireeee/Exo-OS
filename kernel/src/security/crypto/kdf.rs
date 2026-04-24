// kernel/src/security/crypto/kdf.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// KDF (Key Derivation Functions) — hkdf + sha2 + blake3
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE CRYPTO-CRATES : implémentation via crates RustCrypto validées IETF.
// Crates utilisées :
//   • hkdf  v0.12.x + sha2  v0.10.x  → HKDF-SHA256/HKDF-SHA512 (RFC 5869)
//   • blake3 v1.x (features=["pure"]) → blake3_kdf (mode derive_key BLAKE3)
//
// Deux familles de KDF exportées :
//   1. HKDF-SHA256/HKDF-SHA512   → standard IETF, utilisé pour les clés IPC/FS
//   2. BLAKE3 KDF (derive_key)    → pour les usages haute performance kernel
//
// Tous les contextes de domaine DOIVENT être uniques et statiques.
// ═══════════════════════════════════════════════════════════════════════════════

extern crate alloc;

use hkdf::Hkdf;
use sha2::{Sha256, Sha512};

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Clé dérivée 32 octets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedKey32(pub [u8; 32]);

impl DerivedKey32 {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
    pub fn zeroize(&mut self) {
        self.0.iter_mut().for_each(|b| *b = 0);
    }
}

/// Clé dérivée 64 octets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedKey64(pub [u8; 64]);

impl DerivedKey64 {
    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }
    pub fn zeroize(&mut self) {
        self.0.iter_mut().for_each(|b| *b = 0);
    }
}

/// Erreur KDF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KdfError {
    /// Longueur de sortie non supportée.
    InvalidOutputLength,
    /// Paramètre d'entrée invalide.
    InvalidInput,
    /// Erreur interne HKDF expand.
    ExpandError,
}

impl core::fmt::Display for KdfError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            KdfError::InvalidOutputLength => write!(f, "KDF: invalid output length"),
            KdfError::InvalidInput => write!(f, "KDF: invalid input"),
            KdfError::ExpandError => write!(f, "KDF: HKDF expand error"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HKDF-SHA256
// ─────────────────────────────────────────────────────────────────────────────

/// HKDF-SHA256 — Phase d'extraction.
/// Retourne `(PRK 32 octets, Hkdf struct)`.
/// `salt` peut être `None` (HKDF utilise alors un sel de zéros).
pub fn hkdf_extract(salt: Option<&[u8]>, ikm: &[u8]) -> (DerivedKey32, Hkdf<Sha256>) {
    let (prk, hkdf) = Hkdf::<Sha256>::extract(salt, ikm);
    let mut out = [0u8; 32];
    out.copy_from_slice(&prk);
    (DerivedKey32(out), hkdf)
}

/// HKDF-SHA256 — Phase d'expansion vers 32 octets.
pub fn hkdf_expand_32(prk: &Hkdf<Sha256>, info: &[u8]) -> Result<DerivedKey32, KdfError> {
    let mut okm = [0u8; 32];
    prk.expand(info, &mut okm)
        .map_err(|_| KdfError::ExpandError)?;
    Ok(DerivedKey32(okm))
}

/// HKDF-SHA512 — Phase d'expansion vers 64 octets.
pub fn hkdf_expand_64(
    ikm: &[u8],
    salt: Option<&[u8]>,
    info: &[u8],
) -> Result<DerivedKey64, KdfError> {
    let (_, hkdf) = Hkdf::<Sha512>::extract(salt, ikm);
    let mut okm = [0u8; 64];
    hkdf.expand(info, &mut okm)
        .map_err(|_| KdfError::ExpandError)?;
    Ok(DerivedKey64(okm))
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions de dérivation spécialisées (HKDF-SHA256)
// ─────────────────────────────────────────────────────────────────────────────

/// Dérive une sous-clé 32B depuis un IKM avec sel + contexte.
pub fn derive_subkey(
    ikm: &[u8],
    salt: Option<&[u8]>,
    context: &[u8],
) -> Result<DerivedKey32, KdfError> {
    let (_, hkdf) = Hkdf::<Sha256>::extract(salt, ikm);
    hkdf_expand_32(&hkdf, context)
}

/// Dérive les clés de chiffrement (enc) et d'authentification (mac) depuis un IKM.
///
/// Retourne `(enc_key 32B, mac_key 32B)`.
pub fn derive_enc_mac_keys(
    ikm: &[u8],
    salt: Option<&[u8]>,
) -> Result<(DerivedKey32, DerivedKey32), KdfError> {
    let (_, hkdf) = Hkdf::<Sha256>::extract(salt, ikm);
    let enc = hkdf_expand_32(&hkdf, b"ExoOS 2025 enc-key")?;
    let mac = hkdf_expand_32(&hkdf, b"ExoOS 2025 mac-key")?;
    Ok((enc, mac))
}

/// Dérive la clé d'un canal IPC depuis un secret partagé DH.
pub fn derive_ipc_channel_key(
    dh_shared: &[u8; 32],
    channel_id: u64,
) -> Result<DerivedKey32, KdfError> {
    let mut info = [0u8; 8 + 18]; // channel_id (8B) + label
    info[..8].copy_from_slice(&channel_id.to_le_bytes());
    info[8..].copy_from_slice(b"ExoOS IPC channel ");
    derive_subkey(dh_shared, Some(b"ExoOS-IPC-2025"), &info)
}

/// Dérive la clé d'attestation TCB.
pub fn derive_tcb_attestation_key(
    root_secret: &[u8],
    tcb_hash: &[u8; 32],
) -> Result<DerivedKey32, KdfError> {
    derive_subkey(root_secret, Some(tcb_hash), b"ExoOS 2025 tcb-attest")
}

/// Dérive une clé de chiffrement de clé (KEK).
pub fn derive_key_encryption_key(
    master_key: &[u8; 32],
    key_id: u32,
) -> Result<DerivedKey32, KdfError> {
    let mut info = [0u8; 4 + 15];
    info[..4].copy_from_slice(&key_id.to_le_bytes());
    info[4..].copy_from_slice(b"ExoOS 2025 kek ");
    derive_subkey(master_key, Some(b"ExoOS-KEK-2025"), &info)
}

/// Dérive une clé de chiffrement de bloc filesystem.
pub fn derive_fs_block_key(
    volume_key: &[u8; 32],
    block_index: u64,
) -> Result<DerivedKey32, KdfError> {
    let mut info = [0u8; 8 + 18];
    info[..8].copy_from_slice(&block_index.to_le_bytes());
    info[8..].copy_from_slice(b"ExoOS 2025 fs-blk ");
    derive_subkey(volume_key, Some(b"ExoOS-FS-2025"), &info)
}

// ─────────────────────────────────────────────────────────────────────────────
// BLAKE3 KDF (haute performance)
// ─────────────────────────────────────────────────────────────────────────────

/// Dérivation de clé BLAKE3 native.
///
/// Utilise le mode `derive_key` de BLAKE3 (plus rapide que HKDF pour usage
/// interne kernel sans besoin d'interopérabilité RFC).
/// `context` doit être unique et statique (ASCII).
pub fn blake3_kdf(context: &[u8], material: &[u8]) -> DerivedKey32 {
    let ctx = core::str::from_utf8(context).unwrap_or("ExoOS-KDF-Blake3");
    let derived = blake3::derive_key(ctx, material);
    DerivedKey32(derived)
}

/// BLAKE3 KDF avec sortie XOF (longueur arbitraire, max 64B via ce wrapper).
pub fn blake3_kdf_xof(context: &[u8], material: &[u8], out: &mut [u8]) {
    let ctx = core::str::from_utf8(context).unwrap_or("ExoOS-KDF-Blake3");
    if out.len() <= 32 {
        let d = blake3::derive_key(ctx, material);
        let n = out.len();
        out[..n].copy_from_slice(&d[..n]);
    } else {
        let mut h = blake3::Hasher::new_derive_key(ctx);
        h.update(material);
        h.finalize_xof().fill(out);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hkdf_extract_expand() {
        let ikm = b"input key material";
        let salt = Some(b"ExoOS salt".as_ref());
        let (prk, hkdf) = hkdf_extract(salt, ikm);
        assert_eq!(prk.0.len(), 32);

        let k1 = hkdf_expand_32(&hkdf, b"enc").unwrap();
        let k2 = hkdf_expand_32(&hkdf, b"mac").unwrap();
        assert_ne!(k1, k2, "contextes différents → clés différentes");
    }

    #[test]
    fn test_derive_enc_mac_keys() {
        let (enc, mac) = derive_enc_mac_keys(b"shared-secret", None).unwrap();
        assert_ne!(enc, mac);
    }

    #[test]
    fn test_blake3_kdf_domain_separation() {
        let k1 = blake3_kdf(b"ExoOS 2025 volume-enc", b"material");
        let k2 = blake3_kdf(b"ExoOS 2025 volume-mac", b"material");
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_derive_fs_block_key_unique_per_block() {
        let vol_key = [0x55u8; 32];
        let k0 = derive_fs_block_key(&vol_key, 0).unwrap();
        let k1 = derive_fs_block_key(&vol_key, 1).unwrap();
        assert_ne!(k0, k1);
    }

    #[test]
    fn test_hkdf_expand_64() {
        let k64 = hkdf_expand_64(b"ikm", Some(b"salt"), b"info-64").unwrap();
        assert_eq!(k64.0.len(), 64);
    }
}
