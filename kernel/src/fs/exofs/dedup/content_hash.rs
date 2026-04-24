//! ContentHash — calcul et registre d'empreintes de contenu (no_std).
//!
//! RÈGLE 11 : le BlobId d'un objet = Blake3(données AVANT compression/chiffrement).
//! Le ContentHash est identique au BlobId pour les données brutes.
//!
//! OOM-02 / ARITH-02 / RECUR-01 respectés.

use super::chunk_fingerprint::{blake3_hash, fnv1a64, xxhash64};
use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Capacité maximale du cache interne de hashes.
pub const CONTENT_HASH_CACHE_CAPACITY: usize = 8192;
/// Seuil de données à partir duquel on utilise un double hash.
pub const DOUBLE_HASH_THRESHOLD: usize = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// Algorithme de hachage de contenu
// ─────────────────────────────────────────────────────────────────────────────

/// Algorithme de hachage de contenu sélectionné.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HashAlgorithm {
    /// Blake3 seul (cryptographique, 256 bits).
    Blake3 = 0,
    /// XxHash64 (rapide, non-cryptographique).
    XxHash64 = 1,
    /// Double : FNV-1a (rapide) + Blake3 (validation).
    Double = 2,
}

impl HashAlgorithm {
    /// Retourne `true` si l'algorithme est adapté à la déduplication sécurisée.
    pub fn is_secure(self) -> bool {
        !matches!(self, Self::XxHash64)
    }

    /// Nom lisible.
    pub fn name(self) -> &'static str {
        match self {
            Self::Blake3 => "blake3",
            Self::XxHash64 => "xxhash64",
            Self::Double => "double",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ContentHashResult
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat du calcul d'empreinte de contenu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentHashResult {
    /// Empreinte Blake3 (256 bits).
    pub blake3: [u8; 32],
    /// Hash rapide XxHash64.
    pub xxhash: u64,
    /// Hash FNV-1a 64 bits.
    pub fnv: u64,
    /// Algorithme utilisé pour le calcul.
    pub algorithm: HashAlgorithm,
}

impl ContentHashResult {
    /// Calcule les deux empreintes depuis des données brutes.
    pub fn compute(data: &[u8]) -> Self {
        let blake3 = blake3_hash(data);
        let xxhash = xxhash64(data, 0);
        let fnv = fnv1a64(data);
        let algo = if data.len() >= DOUBLE_HASH_THRESHOLD {
            HashAlgorithm::Double
        } else {
            HashAlgorithm::Blake3
        };
        Self {
            blake3,
            xxhash,
            fnv,
            algorithm: algo,
        }
    }

    /// Retourne le BlobId correspondant (RÈGLE 11).
    pub fn blob_id(&self) -> BlobId {
        BlobId::from_raw(self.blake3)
    }

    /// Retourne `true` si le fast_hash correspond (filtrage préliminaire rapide).
    pub fn fast_hash_matches(&self, other: &Self) -> bool {
        self.xxhash == other.xxhash && self.fnv == other.fnv
    }

    /// Comparaison à temps constant sur le blake3.
    ///
    /// ARITH-02 : XOR accumulé.
    pub fn constant_time_eq(&self, other: &Self) -> bool {
        let mut d: u8 = 0;
        for (a, b) in self.blake3.iter().zip(other.blake3.iter()) {
            d |= a ^ b;
        }
        d == 0
    }

    /// Retourne les 8 premiers octets du blake3 comme u64 (index de shard).
    pub fn shard_key(&self) -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(&self.blake3[..8]);
        u64::from_le_bytes(b)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HashCacheEntry — entrée du cache
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct HashCacheEntry {
    result: ContentHashResult,
    #[allow(dead_code)]
    data_len: usize,
    accesses: u32,
}

// ─────────────────────────────────────────────────────────────────────────────
// ContentHash — registre global (spinlock-free via UnsafeCell)
// ─────────────────────────────────────────────────────────────────────────────

use core::cell::UnsafeCell;

/// Registre global de hashes de contenu.
///
/// Thread-safe via spinlock AtomicU64.
pub struct ContentHash {
    cache: UnsafeCell<BTreeMap<[u8; 32], HashCacheEntry>>,
    lock: AtomicU64,
    computations: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    evictions: AtomicU64,
    total_bytes: AtomicU64,
}

unsafe impl Sync for ContentHash {}
unsafe impl Send for ContentHash {}

impl ContentHash {
    /// Constructeur `const` pour static.
    pub const fn new_const() -> Self {
        Self {
            cache: UnsafeCell::new(BTreeMap::new()),
            lock: AtomicU64::new(0),
            computations: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
        }
    }

    // Spinlock acquire.
    fn acquire(&self) {
        while self
            .lock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }
    // Spinlock release.
    fn release(&self) {
        self.lock.store(0, Ordering::Release);
    }

    /// Calcule ou retrouve l'empreinte d'un bloc de données.
    ///
    /// OOM-02 : éviction si cache plein avant insertion.
    pub fn hash_data(&self, data: &[u8]) -> ExofsResult<ContentHashResult> {
        let result = ContentHashResult::compute(data);
        self.acquire();
        // SAFETY: accès exclusif sous spinlock.
        let cache = unsafe { &mut *self.cache.get() };
        if let Some(entry) = cache.get_mut(&result.blake3) {
            entry.accesses = entry.accesses.saturating_add(1);
            let r = entry.result;
            self.release();
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(r);
        }
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
        // Éviction LRU simplifiée (supprime la première entrée).
        if cache.len() >= CONTENT_HASH_CACHE_CAPACITY {
            if let Some(&oldest) = cache.keys().next() {
                cache.remove(&oldest);
                self.evictions.fetch_add(1, Ordering::Relaxed);
            }
        }
        cache.insert(
            result.blake3,
            HashCacheEntry {
                result,
                data_len: data.len(),
                accesses: 1,
            },
        );
        self.release();
        self.computations.fetch_add(1, Ordering::Relaxed);
        self.total_bytes
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        Ok(result)
    }

    /// Calcule le BlobId d'un blob (RÈGLE 11).
    pub fn compute_blob_id(&self, data: &[u8]) -> ExofsResult<BlobId> {
        Ok(self.hash_data(data)?.blob_id())
    }

    /// Recherche par blake3 sans recalcul.
    pub fn lookup_by_blake3(&self, blake3: &[u8; 32]) -> Option<ContentHashResult> {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let cache = unsafe { &*self.cache.get() };
        let r = cache.get(blake3).map(|e| e.result);
        self.release();
        r
    }

    /// Vérifie si un blob avec ce blake3 est déjà connu.
    pub fn is_known(&self, blake3: &[u8; 32]) -> bool {
        self.lookup_by_blake3(blake3).is_some()
    }

    /// Supprime une entrée du cache.
    pub fn evict(&self, blake3: &[u8; 32]) {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let cache = unsafe { &mut *self.cache.get() };
        cache.remove(blake3);
        self.release();
        self.evictions.fetch_add(1, Ordering::Relaxed);
    }

    /// Vide entièrement le cache.
    pub fn clear(&self) {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let cache = unsafe { &mut *self.cache.get() };
        cache.clear();
        self.release();
    }

    // Compteurs.
    pub fn computation_count(&self) -> u64 {
        self.computations.load(Ordering::Relaxed)
    }
    pub fn cache_hit_count(&self) -> u64 {
        self.cache_hits.load(Ordering::Relaxed)
    }
    pub fn cache_miss_count(&self) -> u64 {
        self.cache_misses.load(Ordering::Relaxed)
    }
    pub fn eviction_count(&self) -> u64 {
        self.evictions.load(Ordering::Relaxed)
    }
    pub fn total_bytes_hashed(&self) -> u64 {
        self.total_bytes.load(Ordering::Relaxed)
    }

    /// Taille actuelle du cache.
    pub fn cache_len(&self) -> usize {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let l = unsafe { (*self.cache.get()).len() };
        self.release();
        l
    }

    /// Taux de hit (0..=100).
    pub fn hit_rate_pct(&self) -> u64 {
        let hits = self.cache_hit_count();
        let misses = self.cache_miss_count();
        let total = hits.saturating_add(misses);
        if total == 0 {
            0
        } else {
            hits.saturating_mul(100) / total
        }
    }

    /// Vérifie l'intégrité d'un bloc en recalculant son empreinte.
    pub fn verify_integrity(&self, data: &[u8], expected_blake3: &[u8; 32]) -> bool {
        let computed = blake3_hash(data);
        // Comparaison à temps constant.
        let mut diff: u8 = 0;
        for (a, b) in computed.iter().zip(expected_blake3.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }

    /// Calcule les empreintes pour un batch de blocs.
    ///
    /// OOM-02 : try_reserve.
    pub fn hash_batch(&self, items: &[&[u8]]) -> ExofsResult<Vec<ContentHashResult>> {
        let mut out: Vec<ContentHashResult> = Vec::new();
        out.try_reserve(items.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for &data in items {
            out.push(self.hash_data(data)?);
        }
        Ok(out)
    }

    /// Détecte les doublons dans un batch (retourne les indices des doublons).
    ///
    /// OOM-02 : try_reserve.
    /// RECUR-01 : O(n²) itératif.
    pub fn find_duplicates_in_batch(&self, items: &[&[u8]]) -> ExofsResult<Vec<(usize, usize)>> {
        let hashes = self.hash_batch(items)?;
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        pairs
            .try_reserve(items.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for i in 0..hashes.len() {
            for j in (i + 1)..hashes.len() {
                if hashes[i].constant_time_eq(&hashes[j]) {
                    pairs.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    pairs.push((i, j));
                }
            }
        }
        Ok(pairs)
    }
}

/// Instance globale.
pub static CONTENT_HASH: ContentHash = ContentHash::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// HashSummary
// ─────────────────────────────────────────────────────────────────────────────

/// Résumé des statistiques du registre.
#[derive(Debug, Clone, Copy)]
pub struct HashSummary {
    pub computations: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub evictions: u64,
    pub total_bytes: u64,
    pub hit_rate_pct: u64,
    pub cache_size: usize,
}

impl ContentHash {
    /// Produit un résumé statistique.
    pub fn summary(&self) -> HashSummary {
        HashSummary {
            computations: self.computation_count(),
            cache_hits: self.cache_hit_count(),
            cache_misses: self.cache_miss_count(),
            evictions: self.eviction_count(),
            total_bytes: self.total_bytes_hashed(),
            hit_rate_pct: self.hit_rate_pct(),
            cache_size: self.cache_len(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ch() -> ContentHash {
        ContentHash::new_const()
    }

    #[test]
    fn test_compute_deterministic() {
        let r1 = ContentHashResult::compute(b"hello world");
        let r2 = ContentHashResult::compute(b"hello world");
        assert_eq!(r1.blake3, r2.blake3);
        assert_eq!(r1.xxhash, r2.xxhash);
    }

    #[test]
    fn test_compute_different_data() {
        let r1 = ContentHashResult::compute(b"aaa");
        let r2 = ContentHashResult::compute(b"bbb");
        assert_ne!(r1.blake3, r2.blake3);
    }

    #[test]
    fn test_blob_id_from_hash() {
        let r = ContentHashResult::compute(b"data");
        let id = r.blob_id();
        assert_eq!(id.as_bytes(), &r.blake3);
    }

    #[test]
    fn test_fast_hash_matches() {
        let r1 = ContentHashResult::compute(b"same");
        let r2 = ContentHashResult::compute(b"same");
        assert!(r1.fast_hash_matches(&r2));
    }

    #[test]
    fn test_constant_time_eq() {
        let r1 = ContentHashResult::compute(b"x");
        let r2 = ContentHashResult::compute(b"x");
        assert!(r1.constant_time_eq(&r2));
        let r3 = ContentHashResult::compute(b"y");
        assert!(!r1.constant_time_eq(&r3));
    }

    #[test]
    fn test_hash_data_returns_result() {
        let ch = ch();
        let r = ch.hash_data(b"test blob data").unwrap();
        assert_ne!(r.blake3, [0u8; 32]);
    }

    #[test]
    fn test_cache_hit() {
        let ch = ch();
        ch.hash_data(b"cached").unwrap();
        ch.hash_data(b"cached").unwrap();
        assert!(ch.cache_hit_count() >= 1);
    }

    #[test]
    fn test_is_known() {
        let ch = ch();
        let r = ch.hash_data(b"known data").unwrap();
        assert!(ch.is_known(&r.blake3));
        assert!(!ch.is_known(&[0u8; 32]));
    }

    #[test]
    fn test_verify_integrity_ok() {
        let ch = ch();
        let data = b"integrity check data";
        let b3 = blake3_hash(data);
        assert!(ch.verify_integrity(data, &b3));
    }

    #[test]
    fn test_verify_integrity_fail() {
        let ch = ch();
        let data = b"some data";
        assert!(!ch.verify_integrity(data, &[0xFF; 32]));
    }

    #[test]
    fn test_hash_batch() {
        let ch = ch();
        let items: &[&[u8]] = &[b"a", b"b", b"c"];
        let results = ch.hash_batch(items).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_find_duplicates() {
        let ch = ch();
        let items: &[&[u8]] = &[b"dup", b"unique", b"dup"];
        let dups = ch.find_duplicates_in_batch(items).unwrap();
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0], (0, 2));
    }

    #[test]
    fn test_evict() {
        let ch = ch();
        let r = ch.hash_data(b"evict me").unwrap();
        ch.evict(&r.blake3);
        assert!(!ch.is_known(&r.blake3));
    }

    #[test]
    fn test_clear() {
        let ch = ch();
        ch.hash_data(b"x").unwrap();
        ch.clear();
        assert_eq!(ch.cache_len(), 0);
    }

    #[test]
    fn test_summary() {
        let ch = ch();
        ch.hash_data(b"summary test").unwrap();
        let s = ch.summary();
        assert!(s.computations >= 1);
    }

    #[test]
    fn test_shard_key() {
        let r = ContentHashResult::compute(b"shard");
        let k = r.shard_key();
        assert_eq!(k, u64::from_le_bytes(r.blake3[..8].try_into().unwrap()));
    }
}
