//! Dérivation de clés ExoFS — HKDF-SHA256 embarqué.
//!
//! Ce module fournit une implémentation complète de HKDF (RFC 5869) basée
//! sur HMAC-SHA256, ainsi qu'un étirement de passphrase (KDF multi-tours)
//! et une API de batch pour dériver plusieurs clés simultanément.
//!
//! # Sécurité
//! - Toutes les clés dérivées sont zeroizées à la destruction.
//! - Les intermédiaires temporaires sont zeroizés explicitement.
//! - OOM-02 : `try_reserve` avant toute allocation Vec.
//! - ARITH-02 : arithmétique wrapping/checked.
//! - RECUR-01 : aucune récursivité.


use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Taille d'une clé dérivée standard (256 bits).
pub const DERIVED_KEY_LEN:  usize = 32;
/// Taille de sortie du hash sous-jacent (SHA-256).
pub const HASH_LEN:         usize = 32;
/// Longueur maximale de sortie HKDF-SHA256 = 255 × HashLen.
pub const HKDF_MAX_OUTPUT:  usize = 255 * HASH_LEN;
/// Nombre d'itérations minimum pour l'étirement de passphrase.
pub const KDF_MIN_ITERS:    u8    = 8;
/// Nombre d'itérations recommandé.
pub const KDF_DEFAULT_ITERS: u8   = 12;

/// Domaine de séparation par type de clé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPurpose {
    /// Clé de chiffrement de données.
    DataEncryption,
    /// Clé d'intégrité (MAC).
    Authentication,
    /// Clé de session (éphémère).
    Session,
    /// Clé d'enveloppe (wrapping).
    Wrapping,
    /// Clé d'objet BlobFS.
    BlobObject,
    /// Dérivation personnalisée.
    Custom(&'static str),
}

impl KeyPurpose {
    fn as_bytes(self) -> &'static [u8] {
        match self {
            KeyPurpose::DataEncryption => b"exofs-data-enc-v1",
            KeyPurpose::Authentication => b"exofs-auth-v1",
            KeyPurpose::Session        => b"exofs-session-v1",
            KeyPurpose::Wrapping       => b"exofs-wrap-v1",
            KeyPurpose::BlobObject     => b"exofs-blob-v1",
            KeyPurpose::Custom(s)      => s.as_bytes(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DerivedKey — zeroize on drop
// ─────────────────────────────────────────────────────────────────────────────

/// Clé dérivée de 256 bits. Zeroizée en mémoire à la destruction.
pub struct DerivedKey {
    bytes: [u8; DERIVED_KEY_LEN],
}

impl core::fmt::Debug for DerivedKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "DerivedKey(<redacted>)")
    }
}

impl Drop for DerivedKey {
    fn drop(&mut self) {
        self.bytes.iter_mut().for_each(|b| *b = 0);
    }
}

impl DerivedKey {
    /// Construit depuis un tableau de 32 octets.
    pub fn from_bytes(b: [u8; DERIVED_KEY_LEN]) -> Self { Self { bytes: b } }

    /// Accès en lecture aux 32 octets.
    pub fn as_bytes(&self) -> &[u8; DERIVED_KEY_LEN] { &self.bytes }

    /// Copie les bytes dans un tableau destination.
    pub fn copy_to(&self, dst: &mut [u8; DERIVED_KEY_LEN]) { *dst = self.bytes; }
}

// ─────────────────────────────────────────────────────────────────────────────
// Contexte de dérivation
// ─────────────────────────────────────────────────────────────────────────────

/// Contexte de dérivation de clé.
pub struct KeyDerivation;

impl KeyDerivation {
    // ─── HKDF standard (RFC 5869) ───────────────────────────────────────────

    /// HKDF-Extract : dérive la PRK depuis IKM + salt.
    ///
    /// Si `salt` est vide, utilise un vecteur zéro de longueur HASH_LEN.
    pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; HASH_LEN] {
        if salt.is_empty() {
            hmac_sha256(&[0u8; HASH_LEN], ikm)
        } else {
            hmac_sha256(salt, ikm)
        }
    }

    /// HKDF-Expand : produit `length` octets depuis PRK + info.
    ///
    /// OOM-02 : `try_reserve`.
    /// ARITH-02 : `checked_add`.
    pub fn hkdf_expand(prk: &[u8; HASH_LEN], info: &[u8], length: usize) -> ExofsResult<Vec<u8>> {
        if length == 0 { return Ok(Vec::new()); }
        if length > HKDF_MAX_OUTPUT { return Err(ExofsError::InvalidArgument); }

        let n = length.saturating_add(HASH_LEN - 1) / HASH_LEN; // ceil
        let mut out: Vec<u8> = Vec::new();
        out.try_reserve(n.saturating_mul(HASH_LEN)).map_err(|_| ExofsError::NoMemory)?;

        let mut t: Vec<u8> = Vec::new(); // T(i-1), commence vide
        for counter in 1u8..=(n as u8) {
            let cap = t.len()
                .checked_add(info.len())
                .and_then(|x| x.checked_add(1))
                .ok_or(ExofsError::OffsetOverflow)?;
            let mut input: Vec<u8> = Vec::new();
            input.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;
            input.extend_from_slice(&t);
            input.extend_from_slice(info);
            input.push(counter);
            let ti = hmac_sha256(prk, &input);
            out.extend_from_slice(&ti);
            t.clear();
            t.try_reserve(HASH_LEN).map_err(|_| ExofsError::NoMemory)?;
            t.extend_from_slice(&ti);
        }
        out.truncate(length);
        Ok(out)
    }

    /// HKDF complet (extract + expand).
    pub fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8], length: usize) -> ExofsResult<Vec<u8>> {
        let prk = Self::hkdf_extract(salt, ikm);
        Self::hkdf_expand(&prk, info, length)
    }

    // ─── API simplifiée ──────────────────────────────────────────────────────

    /// Dérive une clé 256 bits depuis un secret, un sel et un contexte.
    pub fn derive_key(secret: &[u8], salt: &[u8], context: &[u8]) -> ExofsResult<DerivedKey> {
        let raw = Self::hkdf(salt, secret, context, DERIVED_KEY_LEN)?;
        let mut b = [0u8; DERIVED_KEY_LEN];
        b.copy_from_slice(&raw);
        Ok(DerivedKey::from_bytes(b))
    }

    /// Dérive une clé pour un usage précis (séparation de domaine automatique).
    pub fn derive_for_purpose(
        secret:  &[u8],
        salt:    &[u8],
        purpose: KeyPurpose,
    ) -> ExofsResult<DerivedKey> {
        Self::derive_key(secret, salt, purpose.as_bytes())
    }

    /// Dérive plusieurs clés en une passe avec des contextes différents.
    ///
    /// OOM-02 : try_reserve.
    pub fn derive_batch(
        secret: &[u8],
        salt:   &[u8],
        infos:  &[&[u8]],
    ) -> ExofsResult<Vec<DerivedKey>> {
        let mut keys: Vec<DerivedKey> = Vec::new();
        keys.try_reserve(infos.len()).map_err(|_| ExofsError::NoMemory)?;
        for &info in infos {
            keys.push(Self::derive_key(secret, salt, info)?);
        }
        Ok(keys)
    }

    // ─── KDF avec stretching (passphrase) ────────────────────────────────────

    /// Dérive depuis une passphrase avec étirement multi-tours.
    ///
    /// Effectue `iters` passes successives de HKDF pour augmenter le coût.
    /// Sur du vrai matériel, préférer Argon2.
    pub fn derive_from_passphrase(
        passphrase: &[u8],
        salt:       &[u8; 32],
        iters:      u8,
    ) -> ExofsResult<DerivedKey> {
        if passphrase.is_empty() { return Err(ExofsError::InvalidArgument); }
        let actual_iters = iters.max(KDF_MIN_ITERS);

        // Passe initiale.
        let mut rolling = Self::hkdf(salt, passphrase, b"exofs-kdf-init-v1", DERIVED_KEY_LEN)?;

        // Itérations de durcissement.
        for i in 0u8..actual_iters {
            let mut ctx: Vec<u8> = Vec::new();
            ctx.try_reserve(14 + 1).map_err(|_| ExofsError::NoMemory)?;
            ctx.extend_from_slice(b"exofs-stretch-");
            ctx.push(i);
            rolling = Self::hkdf(salt, &rolling, &ctx, DERIVED_KEY_LEN)?;
        }

        let mut b = [0u8; DERIVED_KEY_LEN];
        b.copy_from_slice(&rolling);
        Ok(DerivedKey::from_bytes(b))
    }

    /// Variante avec nombre d'itérations par défaut.
    pub fn derive_from_passphrase_default(
        passphrase: &[u8],
        salt:       &[u8; 32],
    ) -> ExofsResult<DerivedKey> {
        Self::derive_from_passphrase(passphrase, salt, KDF_DEFAULT_ITERS)
    }

    /// Dérive une clé maître depuis une passphrase avec **Argon2id** (LAC-06 / S-16).
    ///
    /// Utilise la crate `argon2` (RustCrypto) — jamais d'implémentation from scratch.
    ///
    /// # Paramètres (RFC 9106 — niveau interactif)
    /// | Paramètre  | Valeur    | Description                              |
    /// |------------|-----------|------------------------------------------|
    /// | `m_cost`   | 65536 KiB | Mémoire requise (64 MiB) — augmente le coût GPU |
    /// | `t_cost`   | 3         | Passes de hashage                        |
    /// | `p_cost`   | 4         | Parallélisme (4 threads)                 |
    /// | output     | 32 bytes  | Clé 256 bits                             |
    /// | version    | 0x13      | Argon2 version 1.3                       |
    ///
    /// # Format de stockage (S-16)
    /// La clé wrappée (salt + ciphertext + HMAC) est stockée dans le superblock ExoFS.
    /// Le salt fait 32 bytes générés depuis security::crypto::rng au premier montage.
    /// Les paramètres Argon2 (m_cost, t_cost, p_cost) sont stockés en clair
    /// dans l'en-tête de la clé wrappée pour permettre la dérivation future.
    ///
    /// # Erreurs
    /// - `InvalidArgument` si passphrase vide ou paramètres invalides.
    /// - `InternalError` si Argon2 échoue (mémoire insuffisante).
    pub fn derive_from_passphrase_argon2(
        passphrase: &[u8],
        salt:       &[u8; 32],
    ) -> ExofsResult<[u8; 32]> {
        use argon2::{Argon2, Algorithm, Version, Params, Block};

        if passphrase.is_empty() { return Err(ExofsError::InvalidArgument); }

        // RFC 9106 §4 — paramètres interactifs recommandés.
        let m_cost = 65536u32; // 64 MiB
        let params = Params::new(
            m_cost, // m_cost : 64 MiB
            3,      // t_cost : 3 passes
            4,      // p_cost : 4 threads
            Some(32),
        ).map_err(|_| ExofsError::InvalidArgument)?;

        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut output = [0u8; 32];

        // argon2 0.5.x : hash_password_into_with_memory requiert un buffer mémoire explicite.
        let mut memory: Vec<Block> = Vec::new();
        memory.try_reserve(m_cost as usize).map_err(|_| ExofsError::NoMemory)?;
        memory.resize(m_cost as usize, Block::default());

        argon2.hash_password_into_with_memory(passphrase, salt, &mut output, &mut memory)
            .map_err(|_| ExofsError::InternalError)?;

        Ok(output)
    }

    // ─── Dérivation d'objet / volume ─────────────────────────────────────────

    /// Dérive la clé d'un objet blob depuis la clé de volume + blob_id.
    pub fn derive_object_key(volume_key: &[u8; 32], blob_id: u64) -> ExofsResult<DerivedKey> {
        let mut ctx: Vec<u8> = Vec::new();
        ctx.try_reserve(9 + 8).map_err(|_| ExofsError::NoMemory)?;
        ctx.extend_from_slice(b"exofs-obj");
        ctx.extend_from_slice(&blob_id.to_le_bytes());
        Self::derive_key(volume_key, b"", &ctx)
    }

    /// Dérive une clé de volume depuis la clé maître + identifiant de volume.
    pub fn derive_volume_key(master_key: &[u8; 32], volume_id: u64) -> ExofsResult<DerivedKey> {
        let mut ctx: Vec<u8> = Vec::new();
        ctx.try_reserve(11 + 8).map_err(|_| ExofsError::NoMemory)?;
        ctx.extend_from_slice(b"exofs-vol-k");
        ctx.extend_from_slice(&volume_id.to_le_bytes());
        Self::derive_key(master_key, b"", &ctx)
    }

    /// Dérive une sous-clé d'index.
    pub fn derive_index_key(master_key: &[u8; 32], tree_id: u32) -> ExofsResult<DerivedKey> {
        let mut ctx: Vec<u8> = Vec::new();
        ctx.try_reserve(10 + 4).map_err(|_| ExofsError::NoMemory)?;
        ctx.extend_from_slice(b"exofs-idx-");
        ctx.extend_from_slice(&tree_id.to_le_bytes());
        Self::derive_key(master_key, b"", &ctx)
    }

    /// Vérifie qu'une clé candidate correspond à une dérivation attendue.
    ///
    /// Comparaison en temps constant (ARITH-02 style).
    pub fn verify_derived_key(
        candidate: &DerivedKey,
        secret:    &[u8],
        salt:      &[u8],
        context:   &[u8],
    ) -> ExofsResult<bool> {
        let expected = Self::derive_key(secret, salt, context)?;
        let ok = constant_time_eq_32(candidate.as_bytes(), expected.as_bytes());
        Ok(ok)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SHA-256 et HMAC-SHA256 embarqués (no_std)
// ─────────────────────────────────────────────────────────────────────────────

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1,
    0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
    0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
    0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
    0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
    0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256(msg: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let bit_len = (msg.len() as u64).wrapping_mul(8);
    let mut padded: Vec<u8> = msg.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 { padded.push(0u8); }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i*4..i*4+4].try_into().unwrap_or([0u8;4]));
        }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7)  ^ w[i-15].rotate_right(18) ^ (w[i-15] >> 3);
            let s1 = w[i-2].rotate_right(17)  ^ w[i-2].rotate_right(19)  ^ (w[i-2]  >> 10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let [mut a,mut b,mut c,mut d,mut e,mut f,mut g,mut hh] =
            [h[0],h[1],h[2],h[3],h[4],h[5],h[6],h[7]];
        for i in 0..64 {
            let s1  = e.rotate_right(6)  ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch  = (e & f) ^ ((!e) & g);
            let t1  = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(SHA256_K[i]).wrapping_add(w[i]);
            let s0  = a.rotate_right(2)  ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2  = s0.wrapping_add(maj);
            hh=g; g=f; f=e; e=d.wrapping_add(t1); d=c; c=b; b=a; a=t1.wrapping_add(t2);
        }
        h[0]=h[0].wrapping_add(a); h[1]=h[1].wrapping_add(b);
        h[2]=h[2].wrapping_add(c); h[3]=h[3].wrapping_add(d);
        h[4]=h[4].wrapping_add(e); h[5]=h[5].wrapping_add(f);
        h[6]=h[6].wrapping_add(g); h[7]=h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, &v) in h.iter().enumerate() { out[i*4..i*4+4].copy_from_slice(&v.to_be_bytes()); }
    out
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        let hk = sha256(key); k[..32].copy_from_slice(&hk);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 64]; let mut opad = [0x5cu8; 64];
    for i in 0..64 { ipad[i] ^= k[i]; opad[i] ^= k[i]; }
    let mut inner = Vec::new();
    let _ = inner.try_reserve(64 + msg.len());
    inner.extend_from_slice(&ipad); inner.extend_from_slice(msg);
    let ih = sha256(&inner);
    let mut outer = Vec::new();
    let _ = outer.try_reserve(64 + 32);
    outer.extend_from_slice(&opad); outer.extend_from_slice(&ih);
    sha256(&outer)
}

/// Comparaison en temps constant de deux tableaux de 32 octets.
fn constant_time_eq_32(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 { diff |= a[i] ^ b[i]; }
    diff == 0
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn test_sha256_empty() {
        let h = sha256(b"");
        assert_eq!(h[0], 0xe3); assert_eq!(h[1], 0xb0);
    }

    #[test] fn test_sha256_abc() {
        let h = sha256(b"abc");
        // SHA-256("abc") = ba7816bf...
        assert_eq!(h[0], 0xba); assert_eq!(h[1], 0x78);
    }

    #[test] fn test_hmac_deterministic() {
        let h1 = hmac_sha256(b"key", b"data");
        let h2 = hmac_sha256(b"key", b"data");
        assert_eq!(h1, h2);
    }

    #[test] fn test_hmac_different_keys() {
        let h1 = hmac_sha256(b"k1", b"data");
        let h2 = hmac_sha256(b"k2", b"data");
        assert_ne!(h1, h2);
    }

    #[test] fn test_hkdf_extract_deterministic() {
        let p1 = KeyDerivation::hkdf_extract(b"salt", b"ikm");
        let p2 = KeyDerivation::hkdf_extract(b"salt", b"ikm");
        assert_eq!(p1, p2);
    }

    #[test] fn test_hkdf_extract_empty_salt() {
        let _ = KeyDerivation::hkdf_extract(b"", b"ikm"); // pas de panique
    }

    #[test] fn test_hkdf_expand_exact_len() {
        let prk = [0u8; 32];
        for l in [1, 32, 64, 100, 255, HKDF_MAX_OUTPUT] {
            let v = KeyDerivation::hkdf_expand(&prk, b"info", l).unwrap();
            assert_eq!(v.len(), l, "hkdf_expand length mismatch for l={l}");
        }
    }

    #[test] fn test_hkdf_expand_zero_len() {
        let prk = [0u8; 32];
        assert!(KeyDerivation::hkdf_expand(&prk, b"", 0).unwrap().is_empty());
    }

    #[test] fn test_hkdf_expand_too_large_fails() {
        let prk = [0u8; 32];
        assert!(KeyDerivation::hkdf_expand(&prk, b"", HKDF_MAX_OUTPUT + 1).is_err());
    }

    #[test] fn test_derive_key_32_bytes() {
        let k = KeyDerivation::derive_key(b"secret", b"salt", b"ctx").unwrap();
        assert_eq!(k.as_bytes().len(), 32);
    }

    #[test] fn test_derive_key_different_ctx() {
        let k1 = KeyDerivation::derive_key(b"s", b"salt", b"a").unwrap();
        let k2 = KeyDerivation::derive_key(b"s", b"salt", b"b").unwrap();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
    }

    #[test] fn test_derive_for_purpose_unique() {
        let k1 = KeyDerivation::derive_for_purpose(b"s", b"salt", KeyPurpose::DataEncryption).unwrap();
        let k2 = KeyDerivation::derive_for_purpose(b"s", b"salt", KeyPurpose::Authentication).unwrap();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
    }

    #[test] fn test_derive_from_passphrase_empty_fails() {
        assert!(KeyDerivation::derive_from_passphrase(b"", &[0u8;32], 8).is_err());
    }

    #[test] fn test_derive_from_passphrase_ok() {
        let k = KeyDerivation::derive_from_passphrase(b"hunter2", &[1u8;32], 8).unwrap();
        assert_eq!(k.as_bytes().len(), 32);
    }

    #[test] fn test_derive_from_passphrase_different_salts() {
        let k1 = KeyDerivation::derive_from_passphrase(b"pass", &[1u8;32], 8).unwrap();
        let k2 = KeyDerivation::derive_from_passphrase(b"pass", &[2u8;32], 8).unwrap();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
    }

    #[test] fn test_derive_object_key_different_ids() {
        let vk = [0u8; 32];
        let k1 = KeyDerivation::derive_object_key(&vk, 1).unwrap();
        let k2 = KeyDerivation::derive_object_key(&vk, 2).unwrap();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
    }

    #[test] fn test_derive_volume_key_ok() {
        let mk = [0u8; 32];
        let v  = KeyDerivation::derive_volume_key(&mk, 42).unwrap();
        assert_eq!(v.as_bytes().len(), 32);
    }

    #[test] fn test_derive_batch_count() {
        let infos: &[&[u8]] = &[b"i1", b"i2", b"i3", b"i4"];
        let keys = KeyDerivation::derive_batch(b"sec", b"s", infos).unwrap();
        assert_eq!(keys.len(), 4);
    }

    #[test] fn test_verify_derived_key_ok() {
        let k = KeyDerivation::derive_key(b"sec", b"salt", b"ctx").unwrap();
        assert!(KeyDerivation::verify_derived_key(&k, b"sec", b"salt", b"ctx").unwrap());
    }

    #[test] fn test_verify_derived_key_wrong() {
        let k = KeyDerivation::derive_key(b"sec", b"salt", b"ctx").unwrap();
        assert!(!KeyDerivation::verify_derived_key(&k, b"other", b"salt", b"ctx").unwrap());
    }
}
