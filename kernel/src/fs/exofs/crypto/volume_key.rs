//! Clé de volume ExoFS — chiffrement de la hiérarchie de fichiers.
//!
//! Chaque volume possède sa propre `VolumeKey` dérivée de la `MasterKey`.
//! Elle est utilisée pour dériver les clés d'objets individuels.
//! En transit, elle est encapsulée dans une `WrappedVolumeKey`.
//!
//! OOM-02 / ARITH-02 / RECUR-01 respectés.

use super::entropy::ENTROPY_POOL;
use super::key_derivation::KeyDerivation;
use super::master_key::MasterKey;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Magic d'identification des enveloppes de clé de volume.
pub const VOLUME_KEY_MAGIC: u32 = 0xEF_56_4B_59; // "EFVKY"
/// Taille d'une clé de volume en octets.
pub const VOLUME_KEY_LEN: usize = 32;
/// Taille sérialisée d'une `WrappedVolumeKey`.
pub const WRAPPED_VK_SIZE: usize = 4 + 8 + 32 + 32 + 32; // magic+vol_id+salt+ct+mac

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant de volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VolumeId(pub u64);

impl VolumeId {
    pub fn generate() -> Self {
        Self(ENTROPY_POOL.random_u64())
    }
}

/// Clé de volume (zeroize on drop).
pub struct VolumeKey {
    /// Matériel de clé.
    key: [u8; VOLUME_KEY_LEN],
    /// Identifiant du volume associé.
    volume_id: VolumeId,
}

impl core::fmt::Debug for VolumeKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "VolumeKey {{ volume_id: {:?}, key: <redacted> }}",
            self.volume_id
        )
    }
}

impl Drop for VolumeKey {
    fn drop(&mut self) {
        self.key.iter_mut().for_each(|b| *b = 0);
    }
}

/// Clé de volume encapsulée.
#[derive(Debug, Clone)]
pub struct WrappedVolumeKey {
    /// Magic d'identification.
    pub magic: u32,
    /// ID du volume.
    pub volume_id: VolumeId,
    /// Sel de dérivation du KEK.
    pub salt: [u8; 32],
    /// Texte chiffré XOR.
    pub ct: [u8; VOLUME_KEY_LEN],
    /// MAC d'intégrité.
    pub mac: [u8; 32],
}

impl WrappedVolumeKey {
    /// Sérialise en tableau d'octets plat.
    pub fn to_bytes(&self) -> [u8; WRAPPED_VK_SIZE] {
        let mut out = [0u8; WRAPPED_VK_SIZE];
        out[..4].copy_from_slice(&self.magic.to_le_bytes());
        out[4..12].copy_from_slice(&self.volume_id.0.to_le_bytes());
        out[12..44].copy_from_slice(&self.salt);
        out[44..76].copy_from_slice(&self.ct);
        out[76..108].copy_from_slice(&self.mac);
        out
    }

    /// Désérialise depuis des octets bruts.
    pub fn from_bytes(bytes: &[u8; WRAPPED_VK_SIZE]) -> ExofsResult<Self> {
        let magic = u32::from_le_bytes(
            bytes[..4]
                .try_into()
                .map_err(|_| ExofsError::CorruptedStructure)?,
        );
        if magic != VOLUME_KEY_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        let volume_id = VolumeId(u64::from_le_bytes(
            bytes[4..12]
                .try_into()
                .map_err(|_| ExofsError::CorruptedStructure)?,
        ));
        let mut salt = [0u8; 32];
        salt.copy_from_slice(&bytes[12..44]);
        let mut ct = [0u8; 32];
        ct.copy_from_slice(&bytes[44..76]);
        let mut mac = [0u8; 32];
        mac.copy_from_slice(&bytes[76..108]);
        Ok(Self {
            magic,
            volume_id,
            salt,
            ct,
            mac,
        })
    }
}

impl VolumeKey {
    // ── Constructeurs ─────────────────────────────────────────────────────────

    /// Génère une nouvelle clé de volume aléatoire.
    pub fn generate(volume_id: VolumeId) -> ExofsResult<Self> {
        let raw = ENTROPY_POOL.random_bytes(VOLUME_KEY_LEN)?;
        let mut key = [0u8; VOLUME_KEY_LEN];
        key.copy_from_slice(&raw);
        Ok(Self { key, volume_id })
    }

    /// Dérive depuis une clé maître et un identifiant de volume.
    pub fn derive_from_master(master: &MasterKey, volume_id: VolumeId) -> ExofsResult<Self> {
        let raw = master.derive_volume_key(volume_id.0)?;
        Ok(Self {
            key: raw,
            volume_id,
        })
    }

    /// Construit depuis des bytes bruts (import).
    pub fn from_bytes(bytes: [u8; VOLUME_KEY_LEN], volume_id: VolumeId) -> Self {
        Self {
            key: bytes,
            volume_id,
        }
    }

    // ── Accesseurs ────────────────────────────────────────────────────────────

    pub fn volume_id(&self) -> VolumeId {
        self.volume_id
    }
    pub fn raw_bytes(&self) -> &[u8; VOLUME_KEY_LEN] {
        &self.key
    }

    // ── Dérivation objet ──────────────────────────────────────────────────────

    /// Dérive la clé d'un blob depuis cette clé de volume.
    pub fn derive_object_key(&self, blob_id: u64) -> ExofsResult<[u8; 32]> {
        let dk = KeyDerivation::derive_object_key(&self.key, blob_id)?;
        Ok(*dk.as_bytes())
    }

    /// Dérive plusieurs clés d'objets en une passe.
    ///
    /// OOM-02.
    pub fn derive_object_keys_batch(&self, blob_ids: &[u64]) -> ExofsResult<Vec<([u8; 32], u64)>> {
        let mut out: Vec<([u8; 32], u64)> = Vec::new();
        out.try_reserve(blob_ids.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for &bid in blob_ids {
            let k = self.derive_object_key(bid)?;
            out.push((k, bid));
        }
        Ok(out)
    }

    // ── Wrapping ──────────────────────────────────────────────────────────────

    /// Encapsule la clé de volume avec la clé maître.
    ///
    /// La KEK est dérivée via HKDF depuis la clé maître.
    pub fn wrap(&self, master: &MasterKey) -> ExofsResult<WrappedVolumeKey> {
        let salt_raw = ENTROPY_POOL.random_bytes(32)?;
        let mut salt = [0u8; 32];
        salt.copy_from_slice(&salt_raw);

        // KEK = HKDF(master_key, salt, "exofs-wrap-vk")
        let kek_dk = KeyDerivation::derive_key(master.raw_bytes(), &salt, b"exofs-wrap-vk")?;
        let kek = kek_dk.as_bytes();

        // Chiffrement XOR.
        let mut ct = [0u8; VOLUME_KEY_LEN];
        for i in 0..VOLUME_KEY_LEN {
            ct[i] = self.key[i] ^ kek[i];
        }

        // MAC.
        let mac = vk_mac(VOLUME_KEY_MAGIC, self.volume_id, &salt, &ct, kek);
        Ok(WrappedVolumeKey {
            magic: VOLUME_KEY_MAGIC,
            volume_id: self.volume_id,
            salt,
            ct,
            mac,
        })
    }

    /// Déencapsule une clé de volume avec la clé maître.
    pub fn unwrap(wrapped: &WrappedVolumeKey, master: &MasterKey) -> ExofsResult<Self> {
        if wrapped.magic != VOLUME_KEY_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        let kek_dk =
            KeyDerivation::derive_key(master.raw_bytes(), &wrapped.salt, b"exofs-wrap-vk")?;
        let kek = kek_dk.as_bytes();
        let expected = vk_mac(
            wrapped.magic,
            wrapped.volume_id,
            &wrapped.salt,
            &wrapped.ct,
            kek,
        );
        if !ct_eq_32(&expected, &wrapped.mac) {
            return Err(ExofsError::CorruptedStructure);
        }
        let mut key = [0u8; VOLUME_KEY_LEN];
        for i in 0..VOLUME_KEY_LEN {
            key[i] = wrapped.ct[i] ^ kek[i];
        }
        Ok(Self {
            key,
            volume_id: wrapped.volume_id,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn vk_mac(magic: u32, vid: VolumeId, salt: &[u8; 32], ct: &[u8; 32], mk: &[u8; 32]) -> [u8; 32] {
    let mut d: Vec<u8> = Vec::new();
    let _ = d.try_reserve(4 + 8 + 32 + 32);
    d.extend_from_slice(&magic.to_le_bytes());
    d.extend_from_slice(&vid.0.to_le_bytes());
    d.extend_from_slice(salt);
    d.extend_from_slice(ct);
    hmac_sha256_simple(mk, &d)
}

fn hmac_sha256_simple(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        let h = sha256_simple(key);
        k[..32].copy_from_slice(&h);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ip = [0x36u8; 64];
    let mut op = [0x5cu8; 64];
    for i in 0..64 {
        ip[i] ^= k[i];
        op[i] ^= k[i];
    }
    let mut inn = Vec::new();
    let _ = inn.try_reserve(64 + msg.len());
    inn.extend_from_slice(&ip);
    inn.extend_from_slice(msg);
    let ih = sha256_simple(&inn);
    let mut out2 = Vec::new();
    let _ = out2.try_reserve(96);
    out2.extend_from_slice(&op);
    out2.extend_from_slice(&ih);
    sha256_simple(&out2)
}

fn sha256_simple(msg: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bl = (msg.len() as u64).wrapping_mul(8);
    let mut p = msg.to_vec();
    p.push(0x80);
    while p.len() % 64 != 56 {
        p.push(0);
    }
    p.extend_from_slice(&bl.to_be_bytes());
    for ch in p.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(ch[i * 4..i * 4 + 4].try_into().unwrap_or([0; 4]));
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] =
            [h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch2 = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch2)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, &v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn ct_eq_32(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut d = 0u8;
    for i in 0..32 {
        d |= a[i] ^ b[i];
    }
    d == 0
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    // use super::super::master_key::MasterKey;

    fn mk() -> MasterKey {
        MasterKey::generate().unwrap()
    }

    #[test]
    fn test_generate_ok() {
        let vk = VolumeKey::generate(VolumeId(1)).unwrap();
        assert_eq!(vk.raw_bytes().len(), 32);
    }

    #[test]
    fn test_derive_from_master_ok() {
        let mk = mk();
        let vk = VolumeKey::derive_from_master(&mk, VolumeId(42)).unwrap();
        assert_eq!(vk.raw_bytes().len(), 32);
    }

    #[test]
    fn test_derive_from_master_different_vols() {
        let mk = mk();
        let vk1 = VolumeKey::derive_from_master(&mk, VolumeId(1)).unwrap();
        let vk2 = VolumeKey::derive_from_master(&mk, VolumeId(2)).unwrap();
        assert_ne!(vk1.raw_bytes(), vk2.raw_bytes());
    }

    #[test]
    fn test_wrap_unwrap_roundtrip() {
        let mk = mk();
        let vk = VolumeKey::generate(VolumeId(1)).unwrap();
        let orig = *vk.raw_bytes();
        let wrap = vk.wrap(&mk).unwrap();
        let vk2 = VolumeKey::unwrap(&wrap, &mk).unwrap();
        assert_eq!(*vk2.raw_bytes(), orig);
    }

    #[test]
    fn test_wrap_wrong_magic_fails() {
        let mk = mk();
        let vk = VolumeKey::generate(VolumeId(1)).unwrap();
        let mut wrap = vk.wrap(&mk).unwrap();
        wrap.magic = 0xDEAD;
        assert!(VolumeKey::unwrap(&wrap, &mk).is_err());
    }

    #[test]
    fn test_wrap_tampered_ct_fails() {
        let mk = mk();
        let vk = VolumeKey::generate(VolumeId(1)).unwrap();
        let mut wrap = vk.wrap(&mk).unwrap();
        wrap.ct[0] ^= 0xFF;
        assert!(VolumeKey::unwrap(&wrap, &mk).is_err());
    }

    #[test]
    fn test_from_bytes_ok() {
        let b = [0x42u8; 32];
        let vk = VolumeKey::from_bytes(b, VolumeId(5));
        assert_eq!(*vk.raw_bytes(), b);
    }

    #[test]
    fn test_derive_object_key_ok() {
        let vk = VolumeKey::generate(VolumeId(1)).unwrap();
        let ok = vk.derive_object_key(99).unwrap();
        assert_eq!(ok.len(), 32);
    }

    #[test]
    fn test_derive_object_key_different_blobs() {
        let vk = VolumeKey::generate(VolumeId(1)).unwrap();
        let ok1 = vk.derive_object_key(1).unwrap();
        let ok2 = vk.derive_object_key(2).unwrap();
        assert_ne!(ok1, ok2);
    }

    #[test]
    fn test_derive_batch_count() {
        let vk = VolumeKey::generate(VolumeId(1)).unwrap();
        let batch = vk.derive_object_keys_batch(&[1, 2, 3, 4]).unwrap();
        assert_eq!(batch.len(), 4);
    }

    #[test]
    fn test_serialise_roundtrip() {
        let mk = mk();
        let vk = VolumeKey::generate(VolumeId(7)).unwrap();
        let wrap = vk.wrap(&mk).unwrap();
        let b = wrap.to_bytes();
        let restored = WrappedVolumeKey::from_bytes(&b).unwrap();
        assert_eq!(restored.magic, VOLUME_KEY_MAGIC);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Cache de clés d'objets (LRU simple)
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée dans le cache de clés d'objets.
struct CacheEntry {
    blob_id: u64,
    key: [u8; 32],
}

/// Cache LRU compact de clés d'objets.
///
/// Évite de recalculer `derive_object_key` à chaque accès.
pub struct ObjectKeyCache {
    entries: Vec<CacheEntry>,
    capacity: usize,
}

impl ObjectKeyCache {
    /// Crée un cache de capacité `cap` (0 = cache désactivé).
    pub fn new(cap: usize) -> ExofsResult<Self> {
        let mut v: Vec<CacheEntry> = Vec::new();
        if cap > 0 {
            v.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;
        }
        Ok(Self {
            entries: v,
            capacity: cap,
        })
    }

    /// Cherche une clé en cache ou la dérive et la met en cache.
    pub fn get_or_derive(&mut self, vk: &VolumeKey, blob_id: u64) -> ExofsResult<[u8; 32]> {
        // Recherche linéaire (petite capacité).
        for e in &self.entries {
            if e.blob_id == blob_id {
                return Ok(e.key);
            }
        }
        // Dérivation.
        let key = vk.derive_object_key(blob_id)?;
        if self.capacity > 0 {
            if self.entries.len() >= self.capacity {
                // Éviction LRU : retire le premier.
                self.entries.remove(0);
            }
            self.entries.push(CacheEntry { blob_id, key });
        }
        Ok(key)
    }

    /// Invalide une entrée du cache.
    pub fn invalidate(&mut self, blob_id: u64) {
        self.entries.retain(|e| e.blob_id != blob_id);
    }

    /// Vide entièrement le cache.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Retourne le nombre d'entrées actuellement en cache.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Retourne `true` si le cache est vide.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    // use super::super::master_key::MasterKey;

    #[test]
    fn test_cache_basic() {
        let vk = VolumeKey::generate(VolumeId(1)).unwrap();
        let mut c = ObjectKeyCache::new(4).unwrap();
        let k1 = c.get_or_derive(&vk, 10).unwrap();
        let k2 = c.get_or_derive(&vk, 10).unwrap(); // depuis cache
        assert_eq!(k1, k2);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn test_cache_eviction() {
        let vk = VolumeKey::generate(VolumeId(1)).unwrap();
        let mut c = ObjectKeyCache::new(2).unwrap();
        c.get_or_derive(&vk, 1).unwrap();
        c.get_or_derive(&vk, 2).unwrap();
        c.get_or_derive(&vk, 3).unwrap(); // éviction de 1
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn test_cache_invalidate() {
        let vk = VolumeKey::generate(VolumeId(1)).unwrap();
        let mut c = ObjectKeyCache::new(4).unwrap();
        c.get_or_derive(&vk, 5).unwrap();
        c.invalidate(5);
        assert!(c.is_empty());
    }

    #[test]
    fn test_cache_clear() {
        let vk = VolumeKey::generate(VolumeId(1)).unwrap();
        let mut c = ObjectKeyCache::new(4).unwrap();
        c.get_or_derive(&vk, 1).unwrap();
        c.get_or_derive(&vk, 2).unwrap();
        c.clear();
        assert!(c.is_empty());
    }

    #[test]
    fn test_cache_zero_capacity_no_cache() {
        let vk = VolumeKey::generate(VolumeId(1)).unwrap();
        let mut c = ObjectKeyCache::new(0).unwrap();
        c.get_or_derive(&vk, 1).unwrap();
        assert!(c.is_empty()); // pas de mise en cache
    }
}
