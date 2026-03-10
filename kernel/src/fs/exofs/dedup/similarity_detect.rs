//! SimilarityDetector — détection de similarité entre blobs (no_std).
//!
//! Utilise le min-hashing sur des shingles (n-grammes d'octets) pour
//! estimer la similarité Jaccard entre deux blobs.
//!
//! RECUR-01 : aucune récursion — boucles for/while.
//! OOM-02   : try_reserve.
//! ARITH-02 : saturating / checked / wrapping.


use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId};
use super::chunk_fingerprint::fnv1a64;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const SHINGLE_SIZE:      usize = 4;    // Taille d'un shingle (n-gramme).
pub const MIN_HASH_COUNT:    usize = 64;   // Nombre de min-hashes par signature.
pub const SIMILARITY_THRESHOLD_PCT: u8 = 80; // Seuil par défaut.

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions utilitaires
// ─────────────────────────────────────────────────────────────────────────────

/// Hash d'une fenêtre de `SHINGLE_SIZE` octets avec une graine de permutation.
///
/// ARITH-02 : wrapping_mul / XOR.
fn hash_shingle(window: &[u8], seed: u64) -> u64 {
    let h = fnv1a64(window);
    h ^ seed.wrapping_mul(0x9e3779b97f4a7c15)
}

/// Calcule la signature min-hash d'un tampon de données.
///
/// RECUR-01 : boucle for.
/// OOM-02   : try_reserve.
fn compute_minhash(data: &[u8], n_hashes: usize) -> ExofsResult<Vec<u64>> {
    if data.len() < SHINGLE_SIZE {
        // Données trop courtes : signature dégénérée.
        let mut sig: Vec<u64> = Vec::new();
        sig.try_reserve(n_hashes).map_err(|_| ExofsError::NoMemory)?;
        for i in 0..n_hashes { sig.push(i as u64); }
        return Ok(sig);
    }
    let mut sig: Vec<u64> = Vec::new();
    sig.try_reserve(n_hashes).map_err(|_| ExofsError::NoMemory)?;
    for _i in 0..n_hashes { sig.push(u64::MAX); }

    let end = data.len() - SHINGLE_SIZE + 1;
    for pos in 0..end {
        let window = &data[pos..pos + SHINGLE_SIZE];
        for s in 0..n_hashes {
            let h = hash_shingle(window, s as u64);
            if h < sig[s] { sig[s] = h; }
        }
    }
    Ok(sig)
}

/// Estime la similarité Jaccard en pourcentage à partir de deux signatures.
///
/// ARITH-02 : division guardée.
fn jaccard_pct(sig_a: &[u64], sig_b: &[u64]) -> u8 {
    let len = sig_a.len().min(sig_b.len());
    if len == 0 { return 0; }
    let mut equal = 0u64;
    for i in 0..len {
        if sig_a[i] == sig_b[i] { equal = equal.saturating_add(1); }
    }
    ((equal * 100) / len as u64) as u8
}

// ─────────────────────────────────────────────────────────────────────────────
// BlobSignature — empreinte min-hash d'un blob
// ─────────────────────────────────────────────────────────────────────────────

/// Signature min-hash associée à un blob.
#[derive(Clone, Debug)]
pub struct BlobSignature {
    pub blob_id:  BlobId,
    pub minhash:  Vec<u64>,
    pub data_len: usize,
}

impl BlobSignature {
    /// Calcule la signature d'un blob depuis ses données brutes.
    ///
    /// OOM-02 : try_reserve dans compute_minhash.
    pub fn compute(blob_id: BlobId, data: &[u8]) -> ExofsResult<Self> {
        let minhash = compute_minhash(data, MIN_HASH_COUNT)?;
        Ok(Self { blob_id, minhash, data_len: data.len() })
    }

    /// Crée une signature depuis une liste de chunks (mode sans données brutes).
    pub fn from_chunk_hashes(blob_id: BlobId, chunk_hashes: &[[u8; 32]]) -> ExofsResult<Self> {
        if chunk_hashes.is_empty() {
            let mut sig: Vec<u64> = Vec::new();
            sig.try_reserve(MIN_HASH_COUNT).map_err(|_| ExofsError::NoMemory)?;
            for i in 0..MIN_HASH_COUNT { sig.push(i as u64); }
            return Ok(Self { blob_id, minhash: sig, data_len: 0 });
        }
        // Construire un pseudo-buffer depuis les hashes de chunks.
        let mut buf: Vec<u8> = Vec::new();
        buf.try_reserve(chunk_hashes.len() * 8).map_err(|_| ExofsError::NoMemory)?;
        for h in chunk_hashes {
            buf.extend_from_slice(&h[..8]);
        }
        let minhash = compute_minhash(&buf, MIN_HASH_COUNT)?;
        let data_len = chunk_hashes.len() * 32;
        Ok(Self { blob_id, minhash, data_len })
    }

    /// Estime la similarité Jaccard entre cette signature et une autre.
    pub fn similarity_pct(&self, other: &BlobSignature) -> u8 {
        jaccard_pct(&self.minhash, &other.minhash)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SimilarityMatch — paire de blobs similaires
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une détection de similarité.
#[derive(Debug, Clone)]
pub struct SimilarityMatch {
    pub blob_id_a:      BlobId,
    pub blob_id_b:      BlobId,
    pub similarity_pct: u8,
}

impl SimilarityMatch {
    pub fn is_above_threshold(&self, threshold: u8) -> bool {
        self.similarity_pct >= threshold
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SimilarityDetector — moteur de détection
// ─────────────────────────────────────────────────────────────────────────────

/// Détecteur de similarité entre blobs.
pub struct SimilarityDetector {
    pub threshold_pct: u8,
}

impl SimilarityDetector {
    pub fn new(threshold_pct: u8) -> ExofsResult<Self> {
        if threshold_pct > 100 { return Err(ExofsError::InvalidArgument); }
        Ok(Self { threshold_pct })
    }

    pub fn default() -> Self { Self { threshold_pct: SIMILARITY_THRESHOLD_PCT } }

    /// Compare deux blobs et retourne un `SimilarityMatch` si similaires.
    pub fn compare(
        &self,
        sig_a: &BlobSignature,
        sig_b: &BlobSignature,
    ) -> Option<SimilarityMatch> {
        let pct = sig_a.similarity_pct(sig_b);
        if pct >= self.threshold_pct {
            Some(SimilarityMatch {
                blob_id_a:      sig_a.blob_id.clone(),
                blob_id_b:      sig_b.blob_id.clone(),
                similarity_pct: pct,
            })
        } else {
            None
        }
    }

    /// Cherche tous les blobs similaires dans un ensemble de signatures.
    ///
    /// Complexité O(n²) — acceptable pour de petits ensembles.
    /// RECUR-01 : boucles imbriquées — pas de récursion.
    /// OOM-02   : try_reserve.
    pub fn find_similar_pairs(
        &self,
        signatures: &[BlobSignature],
    ) -> ExofsResult<Vec<SimilarityMatch>> {
        let mut results: Vec<SimilarityMatch> = Vec::new();
        let n = signatures.len();
        for i in 0..n {
            for j in (i + 1)..n {
                if let Some(m) = self.compare(&signatures[i], &signatures[j]) {
                    results.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    results.push(m);
                }
            }
        }
        Ok(results)
    }

    /// Trouve tous les blobs similaires à une signature de référence.
    ///
    /// RECUR-01 : boucle for.
    pub fn find_similar_to(
        &self,
        reference: &BlobSignature,
        candidates: &[BlobSignature],
    ) -> ExofsResult<Vec<SimilarityMatch>> {
        let mut results: Vec<SimilarityMatch> = Vec::new();
        for c in candidates {
            if c.blob_id.as_bytes() == reference.blob_id.as_bytes() { continue; }
            if let Some(m) = self.compare(reference, c) {
                results.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                results.push(m);
            }
        }
        Ok(results)
    }

    /// Retourne les paires trié par similarité décroissante.
    ///
    /// RECUR-01 : sort sans récursion (tri par insertion).
    pub fn find_and_rank(
        &self,
        signatures: &[BlobSignature],
    ) -> ExofsResult<Vec<SimilarityMatch>> {
        let mut pairs = self.find_similar_pairs(signatures)?;
        // Tri par insertion (pas de récursion, RECUR-01).
        for i in 1..pairs.len() {
            let mut j = i;
            while j > 0 && pairs[j - 1].similarity_pct < pairs[j].similarity_pct {
                pairs.swap(j - 1, j);
                j -= 1;
            }
        }
        Ok(pairs)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(s: u8) -> BlobId { BlobId::from_raw([s; 32]) }

    #[test] fn test_identical_data() {
        let d   = &[0xAAu8; 2048];
        let s1  = BlobSignature::compute(blob(1), d).unwrap();
        let s2  = BlobSignature::compute(blob(2), d).unwrap();
        assert_eq!(s1.similarity_pct(&s2), 100);
    }

    #[test] fn test_different_data() {
        let d1 = &[0x00u8; 2048];
        let d2 = &[0xFFu8; 2048];
        let s1 = BlobSignature::compute(blob(1), d1).unwrap();
        let s2 = BlobSignature::compute(blob(2), d2).unwrap();
        assert!(s1.similarity_pct(&s2) < 50);
    }

    #[test] fn test_similar_data_above_threshold() {
        let mut d1 = alloc::vec![0u8; 2048];
        let d2     = d1.clone();
        d1[0] = 0xFF; // légère différence
        let s1 = BlobSignature::compute(blob(1), &d1).unwrap();
        let s2 = BlobSignature::compute(blob(2), &d2).unwrap();
        let pct = s1.similarity_pct(&s2);
        assert!(pct > 50); // majoritairement similaires
    }

    #[test] fn test_find_similar_pairs() {
        let d  = &[0xBBu8; 2048];
        let s1 = BlobSignature::compute(blob(1), d).unwrap();
        let s2 = BlobSignature::compute(blob(2), d).unwrap();
        let d  = SimilarityDetector::default();
        let r  = d.find_similar_pairs(&[s1, s2]).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].similarity_pct, 100);
    }

    #[test] fn test_invalid_threshold() {
        assert!(SimilarityDetector::new(101).is_err());
    }

    #[test] fn test_from_chunk_hashes() {
        let keys: Vec<[u8; 32]> = (0..16).map(|i| [i; 32]).collect();
        let s = BlobSignature::from_chunk_hashes(blob(1), &keys).unwrap();
        assert_eq!(s.minhash.len(), MIN_HASH_COUNT);
    }

    #[test] fn test_empty_data() {
        let s = BlobSignature::compute(blob(1), &[]).unwrap();
        assert_eq!(s.minhash.len(), MIN_HASH_COUNT);
    }

    #[test] fn test_find_and_rank_sorted() {
        let d1 = &[0x00u8; 2048];
        let d2: Vec<u8> = (0..2048).map(|i| i as u8).collect();
        let d3 = &[0x00u8; 2048]; // identique à d1
        let s1 = BlobSignature::compute(blob(1), d1).unwrap();
        let s2 = BlobSignature::compute(blob(2), &d2).unwrap();
        let s3 = BlobSignature::compute(blob(3), d3).unwrap();
        let det = SimilarityDetector::new(0).unwrap(); // seuil à 0 pour tout capturer.
        let ranked = det.find_and_rank(&[s1, s2, s3]).unwrap();
        // La paire (1,3) doit être en premier (100%).
        if !ranked.is_empty() {
            assert!(ranked[0].similarity_pct >= ranked[ranked.len() - 1].similarity_pct);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SignatureStore — registre local de signatures pour comparaisons ultérieures
// ─────────────────────────────────────────────────────────────────────────────

/// Stockage local de signatures (pour comparaison différée).
pub struct SignatureStore {
    signatures: Vec<BlobSignature>,
    capacity:   usize,
}

impl SignatureStore {
    pub fn new(capacity: usize) -> ExofsResult<Self> {
        if capacity == 0 { return Err(ExofsError::InvalidArgument); }
        let mut v: Vec<BlobSignature> = Vec::new();
        v.try_reserve(capacity).map_err(|_| ExofsError::NoMemory)?;
        Ok(Self { signatures: v, capacity })
    }

    /// Insère une signature.
    ///
    /// OOM-02 : try_reserve.
    pub fn insert(&mut self, sig: BlobSignature) -> ExofsResult<()> {
        if self.signatures.len() >= self.capacity {
            return Err(ExofsError::NoMemory);
        }
        self.signatures.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.signatures.push(sig);
        Ok(())
    }

    pub fn len(&self)      -> usize { self.signatures.len() }
    pub fn is_empty(&self) -> bool  { self.signatures.is_empty() }

    /// Recherche tous les blobs similaires à une référence.
    pub fn query(
        &self,
        reference: &BlobSignature,
        detector:  &SimilarityDetector,
    ) -> ExofsResult<Vec<SimilarityMatch>> {
        detector.find_similar_to(reference, &self.signatures)
    }

    /// Retourne toutes les paires similaires du store.
    pub fn all_similar_pairs(
        &self,
        detector: &SimilarityDetector,
    ) -> ExofsResult<Vec<SimilarityMatch>> {
        detector.find_similar_pairs(&self.signatures)
    }

    pub fn clear(&mut self) {
        self.signatures.clear();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests supplémentaires
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_store {
    use super::*;

    fn blob(s: u8) -> BlobId { BlobId::from_raw([s; 32]) }

    #[test] fn test_store_insert_query() {
        let mut store = SignatureStore::new(16).unwrap();
        let d         = &[0x55u8; 2048];
        let ref_sig   = BlobSignature::compute(blob(0), d).unwrap();
        let sig2      = BlobSignature::compute(blob(1), d).unwrap();
        store.insert(sig2).unwrap();
        let det = SimilarityDetector::new(80).unwrap();
        let res = store.query(&ref_sig, &det).unwrap();
        assert_eq!(res.len(), 1);
    }

    #[test] fn test_store_capacity_limit() {
        let mut store = SignatureStore::new(1).unwrap();
        let d = &[0u8; 512];
        store.insert(BlobSignature::compute(blob(1), d).unwrap()).unwrap();
        let overflow = store.insert(BlobSignature::compute(blob(2), d).unwrap());
        assert!(overflow.is_err());
    }

    #[test] fn test_store_clear() {
        let mut store = SignatureStore::new(8).unwrap();
        let d = &[0xCCu8; 512];
        store.insert(BlobSignature::compute(blob(1), d).unwrap()).unwrap();
        store.clear();
        assert!(store.is_empty());
    }

    #[test] fn test_minhash_deterministic() {
        let d  = &[0xABu8; 4096];
        let h1 = compute_minhash(d, MIN_HASH_COUNT).unwrap();
        let h2 = compute_minhash(d, MIN_HASH_COUNT).unwrap();
        assert_eq!(h1, h2);
    }

    #[test] fn test_jaccard_identical() {
        let v: Vec<u64> = (0..MIN_HASH_COUNT as u64).collect();
        assert_eq!(jaccard_pct(&v, &v), 100);
    }

    #[test] fn test_jaccard_disjoint() {
        let a: Vec<u64> = (0..MIN_HASH_COUNT as u64).map(|i| i * 2).collect();
        let b: Vec<u64> = (0..MIN_HASH_COUNT as u64).map(|i| i * 2 + 1).collect();
        assert_eq!(jaccard_pct(&a, &b), 0);
    }
}
