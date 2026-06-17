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
use super::xchacha20::{Nonce, Tag, XChaCha20Key, XChaCha20Poly1305};
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
pub const WRAPPED_VK_SIZE: usize = 4 + 8 + 32 + 24 + 32 + 16; // magic+vol_id+salt+nonce+ct+tag

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
    /// Nonce XChaCha20 (192 bits), aléatoire par wrap.
    pub nonce: [u8; 24],
    /// Texte chiffré par XChaCha20-Poly1305 (AEAD).
    pub ct: [u8; VOLUME_KEY_LEN],
    /// Tag d'authentification AEAD (128 bits).
    pub tag: [u8; 16],
}

impl WrappedVolumeKey {
    /// Sérialise en tableau d'octets plat.
    pub fn to_bytes(&self) -> [u8; WRAPPED_VK_SIZE] {
        let mut out = [0u8; WRAPPED_VK_SIZE];
        out[..4].copy_from_slice(&self.magic.to_le_bytes());
        out[4..12].copy_from_slice(&self.volume_id.0.to_le_bytes());
        out[12..44].copy_from_slice(&self.salt);
        out[44..68].copy_from_slice(&self.nonce);
        out[68..100].copy_from_slice(&self.ct);
        out[100..116].copy_from_slice(&self.tag);
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
        let mut nonce = [0u8; 24];
        nonce.copy_from_slice(&bytes[44..68]);
        let mut ct = [0u8; 32];
        ct.copy_from_slice(&bytes[68..100]);
        let mut tag = [0u8; 16];
        tag.copy_from_slice(&bytes[100..116]);
        Ok(Self {
            magic,
            volume_id,
            salt,
            nonce,
            ct,
            tag,
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
        let nonce_raw = ENTROPY_POOL.random_bytes(24)?;
        let mut nonce = [0u8; 24];
        nonce.copy_from_slice(&nonce_raw);

        // FIX-F7 : KEK = HKDF-BLAKE3(master_key, salt) ; chiffrement AEAD
        // XChaCha20-Poly1305 (site crypto unique audité) — remplace XOR + HMAC.
        let kek_dk = KeyDerivation::derive_key(master.raw_bytes(), &salt, b"exofs-wrap-vk")?;
        let key = XChaCha20Key(*kek_dk.as_bytes());
        let aad = vk_aad(VOLUME_KEY_MAGIC, self.volume_id);
        let (ct_vec, tag) = XChaCha20Poly1305::encrypt(&key, &Nonce(nonce), &aad, &self.key)?;
        if ct_vec.len() != VOLUME_KEY_LEN {
            return Err(ExofsError::InternalError);
        }
        let mut ct = [0u8; VOLUME_KEY_LEN];
        ct.copy_from_slice(&ct_vec);
        Ok(WrappedVolumeKey {
            magic: VOLUME_KEY_MAGIC,
            volume_id: self.volume_id,
            salt,
            nonce,
            ct,
            tag: tag.0,
        })
    }

    /// Déencapsule une clé de volume avec la clé maître.
    pub fn unwrap(wrapped: &WrappedVolumeKey, master: &MasterKey) -> ExofsResult<Self> {
        if wrapped.magic != VOLUME_KEY_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        let kek_dk =
            KeyDerivation::derive_key(master.raw_bytes(), &wrapped.salt, b"exofs-wrap-vk")?;
        let key = XChaCha20Key(*kek_dk.as_bytes());
        let aad = vk_aad(wrapped.magic, wrapped.volume_id);
        let plain = XChaCha20Poly1305::decrypt(
            &key,
            &Nonce(wrapped.nonce),
            &aad,
            &wrapped.ct,
            &Tag(wrapped.tag),
        )
        .map_err(|_| ExofsError::CorruptedStructure)?;
        if plain.len() != VOLUME_KEY_LEN {
            return Err(ExofsError::CorruptedStructure);
        }
        let mut key_bytes = [0u8; VOLUME_KEY_LEN];
        key_bytes.copy_from_slice(&plain);
        Ok(Self {
            key: key_bytes,
            volume_id: wrapped.volume_id,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// AAD du wrap : magic (4) || volume_id (8). Lie les métadonnées publiques au
/// chiffré (empêche le rejeu/échange d'enveloppes).
fn vk_aad(magic: u32, vid: VolumeId) -> [u8; 12] {
    let mut aad = [0u8; 12];
    aad[..4].copy_from_slice(&magic.to_le_bytes());
    aad[4..12].copy_from_slice(&vid.0.to_le_bytes());
    aad
}

// FIX-F7 : SHA-256 + HMAC bespoke supprimés — le wrap utilise désormais l'AEAD
// XChaCha20-Poly1305 du site crypto unique (security::crypto). Plus aucune
// primitive cryptographique réimplémentée à la main dans ce module.

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    // use super::super::master_key::MasterKey;

    fn ok<T, E: core::fmt::Debug>(res: Result<T, E>) -> T {
        match res {
            Ok(value) => value,
            Err(err) => panic!("unexpected error: {err:?}"),
        }
    }

    fn mk() -> MasterKey {
        ok(MasterKey::generate())
    }

    #[test]
    fn test_generate_ok() {
        let vk = ok(VolumeKey::generate(VolumeId(1)));
        assert_eq!(vk.raw_bytes().len(), 32);
    }

    #[test]
    fn test_derive_from_master_ok() {
        let mk = mk();
        let vk = ok(VolumeKey::derive_from_master(&mk, VolumeId(42)));
        assert_eq!(vk.raw_bytes().len(), 32);
    }

    #[test]
    fn test_derive_from_master_different_vols() {
        let mk = mk();
        let vk1 = ok(VolumeKey::derive_from_master(&mk, VolumeId(1)));
        let vk2 = ok(VolumeKey::derive_from_master(&mk, VolumeId(2)));
        assert_ne!(vk1.raw_bytes(), vk2.raw_bytes());
    }

    #[test]
    fn test_wrap_unwrap_roundtrip() {
        let mk = mk();
        let vk = ok(VolumeKey::generate(VolumeId(1)));
        let orig = *vk.raw_bytes();
        let wrap = ok(vk.wrap(&mk));
        let vk2 = ok(VolumeKey::unwrap(&wrap, &mk));
        assert_eq!(*vk2.raw_bytes(), orig);
    }

    #[test]
    fn test_wrap_wrong_magic_fails() {
        let mk = mk();
        let vk = ok(VolumeKey::generate(VolumeId(1)));
        let mut wrap = ok(vk.wrap(&mk));
        wrap.magic = 0xDEAD;
        assert!(VolumeKey::unwrap(&wrap, &mk).is_err());
    }

    #[test]
    fn test_wrap_tampered_ct_fails() {
        let mk = mk();
        let vk = ok(VolumeKey::generate(VolumeId(1)));
        let mut wrap = ok(vk.wrap(&mk));
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
        let vk = ok(VolumeKey::generate(VolumeId(1)));
        let object_key = ok(vk.derive_object_key(99));
        assert_eq!(object_key.len(), 32);
    }

    #[test]
    fn test_derive_object_key_different_blobs() {
        let vk = ok(VolumeKey::generate(VolumeId(1)));
        let object_key_1 = ok(vk.derive_object_key(1));
        let object_key_2 = ok(vk.derive_object_key(2));
        assert_ne!(object_key_1, object_key_2);
    }

    #[test]
    fn test_derive_batch_count() {
        let vk = ok(VolumeKey::generate(VolumeId(1)));
        let batch = ok(vk.derive_object_keys_batch(&[1, 2, 3, 4]));
        assert_eq!(batch.len(), 4);
    }

    #[test]
    fn test_serialise_roundtrip() {
        let mk = mk();
        let vk = ok(VolumeKey::generate(VolumeId(7)));
        let wrap = ok(vk.wrap(&mk));
        let b = wrap.to_bytes();
        let restored = ok(WrappedVolumeKey::from_bytes(&b));
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

    fn ok<T, E: core::fmt::Debug>(res: Result<T, E>) -> T {
        match res {
            Ok(value) => value,
            Err(err) => panic!("unexpected error: {err:?}"),
        }
    }

    #[test]
    fn test_cache_basic() {
        let vk = ok(VolumeKey::generate(VolumeId(1)));
        let mut c = ok(ObjectKeyCache::new(4));
        let k1 = ok(c.get_or_derive(&vk, 10));
        let k2 = ok(c.get_or_derive(&vk, 10)); // depuis cache
        assert_eq!(k1, k2);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn test_cache_eviction() {
        let vk = ok(VolumeKey::generate(VolumeId(1)));
        let mut c = ok(ObjectKeyCache::new(2));
        ok(c.get_or_derive(&vk, 1));
        ok(c.get_or_derive(&vk, 2));
        ok(c.get_or_derive(&vk, 3)); // éviction de 1
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn test_cache_invalidate() {
        let vk = ok(VolumeKey::generate(VolumeId(1)));
        let mut c = ok(ObjectKeyCache::new(4));
        ok(c.get_or_derive(&vk, 5));
        c.invalidate(5);
        assert!(c.is_empty());
    }

    #[test]
    fn test_cache_clear() {
        let vk = ok(VolumeKey::generate(VolumeId(1)));
        let mut c = ok(ObjectKeyCache::new(4));
        ok(c.get_or_derive(&vk, 1));
        ok(c.get_or_derive(&vk, 2));
        c.clear();
        assert!(c.is_empty());
    }

    #[test]
    fn test_cache_zero_capacity_no_cache() {
        let vk = ok(VolumeKey::generate(VolumeId(1)));
        let mut c = ok(ObjectKeyCache::new(0));
        ok(c.get_or_derive(&vk, 1));
        assert!(c.is_empty()); // pas de mise en cache
    }
}
