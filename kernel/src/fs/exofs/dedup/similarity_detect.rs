//! SimilarityDetector — détection de blobs similaires pour la déduplication delta (no_std).

use alloc::vec::Vec;
use crate::fs::exofs::core::{BlobId, FsError};
use crate::scheduler::sync::spinlock::SpinLock;
use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU64, Ordering};

/// Taille maximale de la signature de similarité (min-hash).
const MINHASH_COUNT: usize = 128;
/// Nombre de bandes LSH.
const LSH_BANDS: usize = 16;
const LSH_ROWS:  usize = MINHASH_COUNT / LSH_BANDS;

/// Résultat d'une comparaison de similarité.
#[derive(Clone, Debug)]
pub struct SimilarityMatch {
    pub candidate: BlobId,
    pub similarity_percent: u8,
}

/// Détecteur de similarité par MinHash + LSH.
pub struct SimilarityDetector {
    lsh_buckets: SpinLock<BTreeMap<u64, Vec<BlobId>>>,
    signatures:  SpinLock<BTreeMap<BlobId, [u64; MINHASH_COUNT]>>,
    total_sigs:  AtomicU64,
    total_hits:  AtomicU64,
}

pub static SIMILARITY_DETECTOR: SimilarityDetector = SimilarityDetector::new_const();

impl SimilarityDetector {
    pub const fn new_const() -> Self {
        Self {
            lsh_buckets: SpinLock::new(BTreeMap::new()),
            signatures:  SpinLock::new(BTreeMap::new()),
            total_sigs:  AtomicU64::new(0),
            total_hits:  AtomicU64::new(0),
        }
    }

    /// Calcule la signature MinHash d'un blob.
    pub fn compute_signature(chunks_hashes: &[u64]) -> [u64; MINHASH_COUNT] {
        let mut sig = [u64::MAX; MINHASH_COUNT];
        for (i, &h) in chunks_hashes.iter().enumerate() {
            let _ = i;
            // MINHASH_COUNT fonctions de hash simulées par perturbation.
            for k in 0..MINHASH_COUNT {
                let perm = h.wrapping_mul(0x9E3779B185EBCA87u64.wrapping_mul(k as u64 + 1))
                            ^ h.wrapping_shr(17).wrapping_mul(k as u64 + 3);
                if perm < sig[k] { sig[k] = perm; }
            }
        }
        sig
    }

    /// Indexe un blob par sa signature MinHash.
    pub fn index_blob(
        &self,
        blob_id: BlobId,
        sig: [u64; MINHASH_COUNT],
    ) -> Result<(), FsError> {
        // Stocke la signature.
        {
            let mut sigs = self.signatures.lock();
            sigs.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
            sigs.insert(blob_id, sig);
        }
        self.total_sigs.fetch_add(1, Ordering::Relaxed);

        // Insère dans les buckets LSH.
        let mut buckets = self.lsh_buckets.lock();
        for band in 0..LSH_BANDS {
            let band_hash = band_hash(&sig, band);
            let entry = buckets.entry(band_hash).or_insert_with(Vec::new);
            entry.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
            entry.push(blob_id);
        }
        Ok(())
    }

    /// Cherche des blobs similaires à un candidat.
    pub fn find_similar(
        &self,
        sig: &[u64; MINHASH_COUNT],
        threshold_percent: u8,
    ) -> Result<Vec<SimilarityMatch>, FsError> {
        let mut candidates: Vec<BlobId> = Vec::new();
        {
            let buckets = self.lsh_buckets.lock();
            for band in 0..LSH_BANDS {
                let bh = band_hash(sig, band);
                if let Some(list) = buckets.get(&bh) {
                    for &bid in list {
                        if !candidates.contains(&bid) {
                            candidates.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                            candidates.push(bid);
                        }
                    }
                }
            }
        }

        let sigs = self.signatures.lock();
        let mut out = Vec::new();
        for cand in candidates {
            if let Some(cand_sig) = sigs.get(&cand) {
                let sim = jaccard_estimate(sig, cand_sig);
                if sim >= threshold_percent {
                    out.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                    out.push(SimilarityMatch {
                        candidate: cand,
                        similarity_percent: sim,
                    });
                    self.total_hits.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        Ok(out)
    }

    pub fn total_signatures(&self) -> u64 { self.total_sigs.load(Ordering::Relaxed) }
    pub fn total_hits(&self) -> u64 { self.total_hits.load(Ordering::Relaxed) }
}

fn band_hash(sig: &[u64; MINHASH_COUNT], band: usize) -> u64 {
    let start = band * LSH_ROWS;
    let mut h: u64 = band as u64;
    for i in 0..LSH_ROWS {
        h = h.wrapping_mul(0x9E3779B185EBCA87).wrapping_add(sig[start + i]);
    }
    h
}

fn jaccard_estimate(a: &[u64; MINHASH_COUNT], b: &[u64; MINHASH_COUNT]) -> u8 {
    let mut eq = 0u32;
    for i in 0..MINHASH_COUNT {
        if a[i] == b[i] { eq += 1; }
    }
    (eq as u64 * 100 / MINHASH_COUNT as u64) as u8
}
