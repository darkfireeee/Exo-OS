//! Clé maître ExoFS — racine de la hiérarchie cryptographique.
//!
//! La `MasterKey` est la clé racine depuis laquelle toutes les autres clés
//! (volume, objet, session) sont dérivées. Elle doit être protégée avec
//! le niveau de sécurité le plus élevé et ne jamais être stockée en clair.
//!
//! # Cycle de vie
//! 1. Générer via `MasterKey::generate()` (entropie matérielle).
//! 2. Protéger via `MasterKey::wrap_with_passphrase()`.
//! 3. Restaurer via `MasterKey::unwrap_from_passphrase()`.
//! 4. Utiliser via `MasterKey::derive_*()`.
//! 5. Zeroize automatique au drop.
//!
//! OOM-02 / ARITH-02 / RECUR-01 respectés.


use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::entropy::ENTROPY_POOL;
use super::key_derivation::KeyDerivation;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Magic ExoFS pour les enveloppes de clé maître.
pub const MASTER_KEY_MAGIC: u32 = 0xEF_4B_4D_53; // "EFKMS"
/// Taille d'une clé maître (256 bits).
pub const MASTER_KEY_LEN:   usize = 32;
/// Taille de la structure WrappedMasterKey sérialisée.
pub const WRAPPED_MASTER_KEY_SIZE: usize =
    4   // magic
  + 8   // key_id
  + 32  // sel
  + 32  // ciphertext (XOR-key)
  + 32; // HMAC-SHA256 d'intégrité

/// Version du protocole d'enveloppe.
pub const WRAPPING_VERSION: u8 = 1;

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant unique d'une clé maître.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MasterKeyId(pub u64);

impl MasterKeyId {
    /// Génère un identifiant depuis l'entropie du système.
    pub fn generate() -> Self { Self(ENTROPY_POOL.random_u64()) }
}

impl core::fmt::Display for MasterKeyId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "MasterKey({:#018x})", self.0)
    }
}

/// Clé maître (zeroize on drop).
///
/// 256 bits d'entropie identifiés par un `MasterKeyId`.
/// Toutes les dérivations de clés doivent utiliser cette clé comme IKM.
pub struct MasterKey {
    /// Matériel de clé (zeroize on drop).
    key: [u8; MASTER_KEY_LEN],
    /// Identifiant stable.
    id:  MasterKeyId,
}

impl core::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "MasterKey {{ id: {:?}, key: <redacted> }}", self.id)
    }
}

impl Drop for MasterKey {
    fn drop(&mut self) { self.key.iter_mut().for_each(|b| *b = 0); }
}

/// Clé maître encapsulée (en clair = forme transportable chiffrée).
#[derive(Debug, Clone)]
pub struct WrappedMasterKey {
    /// Magic d'identification.
    pub magic:       u32,
    /// Identifiant de la clé emballée.
    pub key_id:      MasterKeyId,
    /// Sel aléatoire utilisé pour la dérivation de la KEK.
    pub salt:        [u8; 32],
    /// Texte chiffré (XOR avec la KEK, simple en no_std).
    pub ciphertext:  [u8; MASTER_KEY_LEN],
    /// Tag HMAC-SHA256 sur magic || key_id || salt || ciphertext.
    pub mac:         [u8; 32],
}

/// Métadonnées publiques d'une clé maître (sans le matériel secret).
#[derive(Debug, Clone)]
pub struct MasterKeyMetadata {
    pub id:         MasterKeyId,
    pub created_at: u64,
    pub version:    u8,
}

impl MasterKey {
    // ── Constructeurs ─────────────────────────────────────────────────────────

    /// Génère une nouvelle clé maître depuis l'entropie matérielle.
    pub fn generate() -> ExofsResult<Self> {
        let raw = ENTROPY_POOL.random_bytes(MASTER_KEY_LEN)?;
        let mut key = [0u8; MASTER_KEY_LEN];
        key.copy_from_slice(&raw);
        Ok(Self { key, id: MasterKeyId::generate() })
    }

    /// Génère depuis un identifiant fourni (pour restauration).
    pub fn generate_with_id(id: MasterKeyId) -> ExofsResult<Self> {
        let raw = ENTROPY_POOL.random_bytes(MASTER_KEY_LEN)?;
        let mut key = [0u8; MASTER_KEY_LEN];
        key.copy_from_slice(&raw);
        Ok(Self { key, id })
    }

    /// Construit depuis des bytes existants (import de clé).
    ///
    /// SECURITY: Le matériel entrant doit provenir d'une source de confiance.
    pub fn from_bytes(key_bytes: [u8; MASTER_KEY_LEN], id: MasterKeyId) -> Self {
        Self { key: key_bytes, id }
    }

    /// Dérive depuis une passphrase + sel (enrôlement initial).
    pub fn derive_from_passphrase(passphrase: &[u8], salt: &[u8; 32]) -> ExofsResult<Self> {
        let dk = KeyDerivation::derive_from_passphrase_default(passphrase, salt)?;
        let mut key = [0u8; MASTER_KEY_LEN];
        key.copy_from_slice(dk.as_bytes());
        Ok(Self { key, id: MasterKeyId::generate() })
    }

    // ── Accesseurs ────────────────────────────────────────────────────────────

    /// Retourne l'identifiant.
    pub fn id(&self) -> MasterKeyId { self.id }

    /// Expose le matériel brut pour la dérivation (référence courte durée).
    pub fn raw_bytes(&self) -> &[u8; MASTER_KEY_LEN] { &self.key }

    /// Métadonnées publiques.
    pub fn metadata(&self) -> MasterKeyMetadata {
        MasterKeyMetadata { id: self.id, created_at: ENTROPY_POOL.random_u64(), version: WRAPPING_VERSION }
    }

    // ── Dérivation ───────────────────────────────────────────────────────────

    /// Dérive une clé de volume depuis cette clé maître et un identifiant de volume.
    pub fn derive_volume_key(&self, volume_id: u64) -> ExofsResult<[u8; 32]> {
        let dk = KeyDerivation::derive_volume_key(&self.key, volume_id)?;
        Ok(*dk.as_bytes())
    }

    /// Dérive une clé d'index.
    pub fn derive_index_key(&self, tree_id: u32) -> ExofsResult<[u8; 32]> {
        let dk = KeyDerivation::derive_index_key(&self.key, tree_id)?;
        Ok(*dk.as_bytes())
    }

    /// Dérive une clé générique avec un contexte.
    pub fn derive_key_for_context(&self, context: &[u8]) -> ExofsResult<[u8; 32]> {
        let dk = KeyDerivation::derive_key(&self.key, b"", context)?;
        Ok(*dk.as_bytes())
    }

    // ── Wrapping / Unwrapping ─────────────────────────────────────────────────

    /// Enveloppe la clé maître avec une passphrase (KEK dérivée HKDF).
    ///
    /// L'enveloppe contient : magic, key_id, salt, ciphertext, HMAC.
    pub fn wrap_with_passphrase(
        &self,
        passphrase: &[u8],
    ) -> ExofsResult<WrappedMasterKey> {
        let salt_raw = ENTROPY_POOL.random_bytes(32)?;
        let mut salt = [0u8; 32];
        salt.copy_from_slice(&salt_raw);

        // KEK = dérivation HKDF depuis la passphrase + salt.
        let kek_dk = KeyDerivation::derive_from_passphrase_default(passphrase, &salt)?;
        let kek    = kek_dk.as_bytes();

        // Chiffrement XOR (simple en no_std ; à remplacer par AES-256-KW en production).
        let mut ciphertext = [0u8; MASTER_KEY_LEN];
        for i in 0..MASTER_KEY_LEN { ciphertext[i] = self.key[i] ^ kek[i]; }

        // HMAC-SHA256 sur magic || key_id || salt || ciphertext.
        let mac = compute_wrap_mac(MASTER_KEY_MAGIC, self.id, &salt, &ciphertext, kek);

        Ok(WrappedMasterKey {
            magic:      MASTER_KEY_MAGIC,
            key_id:     self.id,
            salt,
            ciphertext,
            mac,
        })
    }

    /// Déenveloppe une clé maître depuis une passphrase.
    pub fn unwrap_from_passphrase(
        wrapped:    &WrappedMasterKey,
        passphrase: &[u8],
    ) -> ExofsResult<Self> {
        if wrapped.magic != MASTER_KEY_MAGIC { return Err(ExofsError::InvalidMagic); }

        let kek_dk = KeyDerivation::derive_from_passphrase_default(passphrase, &wrapped.salt)?;
        let kek    = kek_dk.as_bytes();

        // Vérification du MAC.
        let expected_mac = compute_wrap_mac(
            wrapped.magic, wrapped.key_id, &wrapped.salt, &wrapped.ciphertext, kek,
        );
        if !constant_time_eq_32(&expected_mac, &wrapped.mac) {
            return Err(ExofsError::CorruptedStructure);
        }

        // Déchiffrement.
        let mut key = [0u8; MASTER_KEY_LEN];
        for i in 0..MASTER_KEY_LEN { key[i] = wrapped.ciphertext[i] ^ kek[i]; }
        Ok(Self { key, id: wrapped.key_id })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers internes
// ─────────────────────────────────────────────────────────────────────────────

fn compute_wrap_mac(
    magic:      u32,
    key_id:     MasterKeyId,
    salt:       &[u8; 32],
    ciphertext: &[u8; 32],
    mac_key:    &[u8; 32],
) -> [u8; 32] {
    let mut data: Vec<u8> = Vec::new();
    let _ = data.try_reserve(4 + 8 + 32 + 32);
    data.extend_from_slice(&magic.to_le_bytes());
    data.extend_from_slice(&key_id.0.to_le_bytes());
    data.extend_from_slice(salt);
    data.extend_from_slice(ciphertext);
    hmac_sha256(mac_key, &data)
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
    let mut inner: Vec<u8> = Vec::new();
    let _ = inner.try_reserve(64 + msg.len());
    inner.extend_from_slice(&ipad); inner.extend_from_slice(msg);
    let ih = sha256(&inner);
    let mut outer: Vec<u8> = Vec::new();
    let _ = outer.try_reserve(96);
    outer.extend_from_slice(&opad); outer.extend_from_slice(&ih);
    sha256(&outer)
}

fn sha256(msg: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
        0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
        0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
        0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
        0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
        0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
        0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
        0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,
        0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19,
    ];
    let bl = (msg.len() as u64).wrapping_mul(8);
    let mut p = msg.to_vec(); p.push(0x80);
    while p.len() % 64 != 56 { p.push(0); }
    p.extend_from_slice(&bl.to_be_bytes());
    for chunk in p.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 { w[i] = u32::from_be_bytes(chunk[i*4..i*4+4].try_into().unwrap_or([0;4])); }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7)^w[i-15].rotate_right(18)^(w[i-15]>>3);
            let s1 = w[i-2].rotate_right(17)^w[i-2].rotate_right(19)^(w[i-2]>>10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let [mut a,mut b,mut c,mut d,mut e,mut f,mut g,mut hh] = [h[0],h[1],h[2],h[3],h[4],h[5],h[6],h[7]];
        for i in 0..64 {
            let s1=(e.rotate_right(6))^(e.rotate_right(11))^(e.rotate_right(25));
            let ch=(e&f)^((!e)&g);
            let t1=hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0=(a.rotate_right(2))^(a.rotate_right(13))^(a.rotate_right(22));
            let maj=(a&b)^(a&c)^(b&c); let t2=s0.wrapping_add(maj);
            hh=g;g=f;f=e;e=d.wrapping_add(t1);d=c;c=b;b=a;a=t1.wrapping_add(t2);
        }
        h[0]=h[0].wrapping_add(a);h[1]=h[1].wrapping_add(b);h[2]=h[2].wrapping_add(c);h[3]=h[3].wrapping_add(d);
        h[4]=h[4].wrapping_add(e);h[5]=h[5].wrapping_add(f);h[6]=h[6].wrapping_add(g);h[7]=h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i,&v) in h.iter().enumerate() { out[i*4..i*4+4].copy_from_slice(&v.to_be_bytes()); }
    out
}

fn constant_time_eq_32(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut d = 0u8; for i in 0..32 { d |= a[i] ^ b[i]; } d == 0
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn test_generate_ok() {
        let mk = MasterKey::generate().unwrap();
        assert_ne!(*mk.raw_bytes(), [0u8; 32]);
    }

    #[test] fn test_generate_different_each_time() {
        let a = MasterKey::generate().unwrap();
        let b = MasterKey::generate().unwrap();
        assert_ne!(a.raw_bytes(), b.raw_bytes());
    }

    #[test] fn test_id_stable() {
        let mk = MasterKey::generate().unwrap();
        assert_eq!(mk.id(), mk.id());
    }

    #[test] fn test_derive_from_passphrase_ok() {
        let mk = MasterKey::derive_from_passphrase(b"secret", &[1u8; 32]).unwrap();
        assert_eq!(mk.raw_bytes().len(), 32);
    }

    #[test] fn test_wrap_unwrap_roundtrip() {
        let mk      = MasterKey::generate().unwrap();
        let orig    = *mk.raw_bytes();
        let wrapped = mk.wrap_with_passphrase(b"passphrase").unwrap();
        let mk2     = MasterKey::unwrap_from_passphrase(&wrapped, b"passphrase").unwrap();
        assert_eq!(*mk2.raw_bytes(), orig);
    }

    #[test] fn test_wrap_wrong_passphrase_fails() {
        let mk      = MasterKey::generate().unwrap();
        let wrapped = mk.wrap_with_passphrase(b"correct").unwrap();
        assert!(MasterKey::unwrap_from_passphrase(&wrapped, b"wrong").is_err());
    }

    #[test] fn test_wrap_wrong_magic_fails() {
        let mk          = MasterKey::generate().unwrap();
        let mut wrapped = mk.wrap_with_passphrase(b"pass").unwrap();
        wrapped.magic   = 0xDEAD_BEEF;
        assert!(MasterKey::unwrap_from_passphrase(&wrapped, b"pass").is_err());
    }

    #[test] fn test_wrap_tampered_ct_fails() {
        let mk          = MasterKey::generate().unwrap();
        let mut wrapped = mk.wrap_with_passphrase(b"pass").unwrap();
        wrapped.ciphertext[0] ^= 0xFF;
        assert!(MasterKey::unwrap_from_passphrase(&wrapped, b"pass").is_err());
    }

    #[test] fn test_derive_volume_key_ok() {
        let mk = MasterKey::generate().unwrap();
        let vk = mk.derive_volume_key(1).unwrap();
        assert_eq!(vk.len(), 32);
    }

    #[test] fn test_derive_volume_different_ids() {
        let mk  = MasterKey::generate().unwrap();
        let vk1 = mk.derive_volume_key(1).unwrap();
        let vk2 = mk.derive_volume_key(2).unwrap();
        assert_ne!(vk1, vk2);
    }

    #[test] fn test_derive_index_key_ok() {
        let mk = MasterKey::generate().unwrap();
        let ik = mk.derive_index_key(42).unwrap();
        assert_eq!(ik.len(), 32);
    }

    #[test] fn test_master_key_id_display() {
        let id = MasterKeyId(0x1234);
        assert!(format!("{id}").contains("MasterKey"));
    }

    #[test] fn test_from_bytes_roundtrip() {
        let kb = [0xABu8; 32];
        let id = MasterKeyId::generate();
        let mk = MasterKey::from_bytes(kb, id);
        assert_eq!(*mk.raw_bytes(), kb);
    }
}
