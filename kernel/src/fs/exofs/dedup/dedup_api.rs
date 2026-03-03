//! DedupApi — API publique haut niveau du moteur de déduplication (no_std).
//!
//! Orchestre l'ensemble du pipeline de déduplication :
//!  1. Vérification de la politique (DedupPolicy).
//!  2. Calcul du BlobId = blake3(données brutes) — RÈGLE 11.
//!  3. Découpage en chunks (CdcChunker).
//!  4. Fingerprinting de chaque chunk (ChunkFingerprint).
//!  5. Enregistrement dans l'index (ChunkIndex) et le registre (BlobRegistry).
//!  6. Suivi du partage (BlobSharing).
//!  7. Mise à jour des statistiques (DedupStats).
//!
//! RECUR-01 : aucune récursion.
//! OOM-02   : try_reserve.
//! ARITH-02 : saturating / checked / wrapping.

#![allow(dead_code)]

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId};
use super::content_hash::{CONTENT_HASH};
use super::chunker_cdc::CdcChunker;
use super::chunking::Chunker;
use super::chunk_fingerprint::ChunkFingerprint;
use super::chunk_index::CHUNK_INDEX;
use super::blob_registry::BLOB_REGISTRY;
use super::blob_sharing::BLOB_SHARING;
use super::dedup_stats::DEDUP_STATS;
use super::dedup_policy::{DedupPolicy, DedupMode};

// ─────────────────────────────────────────────────────────────────────────────
// DedupResult — résultat d'une opération de déduplication
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat complet d'une déduplication de blob.
#[derive(Debug, Clone)]
pub struct DedupResult {
    pub blob_id:          BlobId,
    pub is_new:           bool,   // Blob jamais vu auparavant.
    pub n_chunks:         u32,
    pub new_chunks:       u32,    // Chunks réellement écrits.
    pub deduped_chunks:   u32,    // Chunks déjà connus.
    pub logical_bytes:    u64,    // Taille brute du blob.
    pub physical_bytes:   u64,    // Octets réellement stockés.
    pub saved_bytes:      u64,    // Économies réalisées.
    pub dedup_ratio_pct:  u8,
}

impl DedupResult {
    fn new(
        blob_id:        BlobId,
        is_new:         bool,
        n_chunks:       u32,
        new_chunks:     u32,
        logical_bytes:  u64,
        physical_bytes: u64,
    ) -> Self {
        let deduped     = n_chunks.saturating_sub(new_chunks);
        let saved       = logical_bytes.saturating_sub(physical_bytes);
        let ratio       = if logical_bytes == 0 { 0 }
                          else { ((saved * 100) / logical_bytes).min(100) as u8 };
        Self {
            blob_id,
            is_new,
            n_chunks,
            new_chunks,
            deduped_chunks:  deduped,
            logical_bytes,
            physical_bytes,
            saved_bytes:     saved,
            dedup_ratio_pct: ratio,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupApi
// ─────────────────────────────────────────────────────────────────────────────

/// Moteur de déduplication à politique configurable.
pub struct DedupApi {
    pub policy:  DedupPolicy,
    chunker:     CdcChunker,
}

impl DedupApi {
    pub fn new(policy: DedupPolicy) -> ExofsResult<Self> {
        policy.validate()?;
        let chunker = CdcChunker::default_chunker();
        Ok(Self { policy, chunker })
    }

    pub fn with_default_policy() -> ExofsResult<Self> {
        Self::new(DedupPolicy::default())
    }

    // ── Pipeline principal ────────────────────────────────────────────────────

    /// Déduplique un blob (données brutes).
    ///
    /// RÈGLE 11 : BlobId = blake3(données avant compression/chiffrement).
    /// RECUR-01 : boucle for — pas de récursion.
    /// OOM-02   : try_reserve dans les sous-fonctions.
    pub fn dedup_blob(&self, data: &[u8]) -> ExofsResult<DedupResult> {
        let logical_bytes = data.len() as u64;

        // 1. Vérification de la politique.
        if !self.policy.should_dedup(logical_bytes) {
            // Blob trop petit ou politique désactivée → pas de dédup.
            let blob_id = CONTENT_HASH.compute_blob_id(data)?;
            return Ok(DedupResult::new(blob_id, true, 0, 0, logical_bytes, logical_bytes));
        }

        // 2. Calcul du BlobId (RÈGLE 11 : avant compression/chiffrement).
        let blob_id = CONTENT_HASH.compute_blob_id(data)?;

        // 3. Vérifier si le blob est déjà connu.
        if BLOB_REGISTRY.contains(&blob_id) {
            BLOB_REGISTRY.register(blob_id.clone(), logical_bytes, Vec::new())?;
            let entry = BLOB_REGISTRY.lookup(&blob_id);
            let n_chunks = entry.map(|e| e.chunk_count).unwrap_or(0);
            DEDUP_STATS.record_blob(true, logical_bytes, 0, n_chunks as u64, n_chunks as u64);
            return Ok(DedupResult::new(blob_id, false, n_chunks, 0, logical_bytes, 0));
        }

        // 4. Découpage en chunks CDC.
        let chunks = self.chunker.chunk(data)?;
        let n_total = chunks.len() as u32;

        // 5. Enregistrement dans l'index et le registre de partage.
        let mut new_chunks    = 0u32;
        let mut phys_bytes    = 0u64;
        let mut chunk_keys:  Vec<[u8; 32]> = Vec::new();
        chunk_keys.try_reserve(chunks.len()).map_err(|_| ExofsError::NoMemory)?;

        for chunk in &chunks {
            let fp    = ChunkFingerprint::compute(&chunk.data, super::chunk_fingerprint::FingerprintAlgorithm::Double)?;
            let key   = chunk.blake3;
            chunk_keys.push(key);
            let is_new_chunk = CHUNK_INDEX.insert(fp, blob_id.clone(), chunk.boundary.length)?;
            if is_new_chunk {
                new_chunks  = new_chunks.saturating_add(1);
                phys_bytes  = phys_bytes.saturating_add(chunk.boundary.length as u64);
            }
            BLOB_SHARING.add_ref(key, blob_id.clone())?;
        }

        // 6. Enregistrement dans le registre de blobs.
        BLOB_REGISTRY.register(blob_id.clone(), logical_bytes, chunk_keys)?;

        // 7. Mise à jour des statistiques.
        let deduped_n = n_total.saturating_sub(new_chunks);
        DEDUP_STATS.record_blob(
            deduped_n > 0,
            logical_bytes,
            phys_bytes,
            n_total as u64,
            deduped_n as u64,
        );

        Ok(DedupResult::new(
            blob_id, true, n_total, new_chunks, logical_bytes, phys_bytes,
        ))
    }

    /// Déduplique un lot de blobs.
    ///
    /// RECUR-01 : boucle for.
    /// OOM-02   : try_reserve.
    pub fn dedup_batch(&self, blobs: &[&[u8]]) -> ExofsResult<Vec<DedupResult>> {
        let mut results: Vec<DedupResult> = Vec::new();
        results.try_reserve(blobs.len()).map_err(|_| ExofsError::NoMemory)?;
        for data in blobs {
            let r = self.dedup_blob(data)?;
            results.push(r);
        }
        Ok(results)
    }

    /// Supprime un blob et libère les chunks orphelins.
    ///
    /// RECUR-01 : boucle for.
    pub fn delete_blob(&self, blob_id: &BlobId) -> ExofsResult<u32> {
        let entry = BLOB_REGISTRY.lookup(blob_id)
            .ok_or(ExofsError::ObjectNotFound)?;
        let keys = entry.chunk_keys.clone();
        // Décrémente les refs de chunks.
        let mut freed = 0u32;
        for key in &keys {
            BLOB_SHARING.remove_ref(key, blob_id);
            if CHUNK_INDEX.decrement_ref(key) {
                freed = freed.saturating_add(1);
            }
        }
        BLOB_REGISTRY.deregister(blob_id);
        Ok(freed)
    }

    /// Vérifie si un blob est présent dans le registre.
    pub fn is_known(&self, blob_id: &BlobId) -> bool {
        BLOB_REGISTRY.contains(blob_id)
    }

    /// Calcule le ratio de déduplication global actuel.
    pub fn dedup_ratio_pct(&self) -> u8 {
        DEDUP_STATS.dedup_ratio_pct()
    }

    /// Vérifie l'intégrité du sous-système de déduplication.
    pub fn verify_integrity(&self) -> ExofsResult<()> {
        BLOB_REGISTRY.verify_integrity()?;
        BLOB_SHARING.verify_integrity()?;
        CHUNK_INDEX.verify_integrity()?;
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupBatchReport — rapport sur un lot de déduplications
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct DedupBatchReport {
    pub total_blobs:       u32,
    pub new_blobs:         u32,
    pub deduped_blobs:     u32,
    pub total_chunks:      u32,
    pub deduped_chunks:    u32,
    pub logical_bytes:     u64,
    pub physical_bytes:    u64,
    pub saved_bytes:       u64,
    pub avg_dedup_pct:     u8,
}

impl DedupBatchReport {
    /// Agrège les résultats d'un lot.
    ///
    /// ARITH-02 : saturating_add.
    pub fn from_results(results: &[DedupResult]) -> Self {
        let mut new_blobs  = 0u32;
        let mut dedup_b    = 0u32;
        let mut tot_chunks = 0u32;
        let mut dedup_c    = 0u32;
        let mut log_bytes  = 0u64;
        let mut phys_bytes = 0u64;
        let mut ratio_sum  = 0u64;
        for r in results {
            if r.is_new      { new_blobs  = new_blobs .saturating_add(1); }
            if !r.is_new || r.deduped_chunks > 0 { dedup_b = dedup_b.saturating_add(1); }
            tot_chunks = tot_chunks.saturating_add(r.n_chunks);
            dedup_c    = dedup_c   .saturating_add(r.deduped_chunks);
            log_bytes  = log_bytes .saturating_add(r.logical_bytes);
            phys_bytes = phys_bytes.saturating_add(r.physical_bytes);
            ratio_sum  = ratio_sum .saturating_add(r.dedup_ratio_pct as u64);
        }
        let n           = results.len() as u64;
        let avg_ratio   = if n == 0 { 0 } else { (ratio_sum / n) as u8 };
        let saved       = log_bytes.saturating_sub(phys_bytes);
        DedupBatchReport {
            total_blobs:    results.len() as u32,
            new_blobs,
            deduped_blobs:  dedup_b,
            total_chunks:   tot_chunks,
            deduped_chunks: dedup_c,
            logical_bytes:  log_bytes,
            physical_bytes: phys_bytes,
            saved_bytes:    saved,
            avg_dedup_pct:  avg_ratio,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::dedup_policy::DedupPolicy;

    fn api() -> DedupApi { DedupApi::with_default_policy().unwrap() }

    #[test] fn test_dedup_new_blob() {
        let api  = api();
        let data = &[0x42u8; 8192];
        let r    = api.dedup_blob(data).unwrap();
        assert!(r.is_new);
        assert_eq!(r.logical_bytes, 8192);
        assert!(r.n_chunks > 0);
    }

    #[test] fn test_dedup_small_blob_skipped() {
        let api  = api();
        let data = &[0u8; 100]; // < POLICY_DEFAULT_MIN_BLOB_SIZE
        let r    = api.dedup_blob(data).unwrap();
        assert_eq!(r.n_chunks, 0); // aucun chunk → skipped
    }

    #[test] fn test_dedup_batch() {
        let api   = api();
        let d1    = &[0xAAu8; 8192] as &[u8];
        let d2    = &[0xBBu8; 16384] as &[u8];
        let res   = api.dedup_batch(&[d1, d2]).unwrap();
        assert_eq!(res.len(), 2);
    }

    #[test] fn test_batch_report() {
        let api   = api();
        let d1    = &[0xCCu8; 8192] as &[u8];
        let d2    = &[0xDDu8; 8192] as &[u8];
        let res   = api.dedup_batch(&[d1, d2]).unwrap();
        let report = DedupBatchReport::from_results(&res);
        assert_eq!(report.total_blobs, 2);
    }

    #[test] fn test_dedup_ratio_returns_u8() {
        let api = api();
        let r   = api.dedup_ratio_pct();
        assert!(r <= 100);
    }

    #[test] fn test_dedup_result_new() {
        let bid = BlobId::from_raw([0x01u8; 32]);
        let r   = DedupResult::new(bid, true, 4, 4, 4096, 4096);
        assert_eq!(r.deduped_chunks, 0);
        assert_eq!(r.saved_bytes, 0);
        assert_eq!(r.dedup_ratio_pct, 0);
    }

    #[test] fn test_dedup_result_with_savings() {
        let bid = BlobId::from_raw([0x02u8; 32]);
        let r   = DedupResult::new(bid, false, 4, 0, 4096, 0);
        assert_eq!(r.deduped_chunks, 4);
        assert_eq!(r.saved_bytes, 4096);
        assert_eq!(r.dedup_ratio_pct, 100);
    }

    #[test] fn test_is_known() {
        let api  = api();
        let data = &[0x77u8; 8192];
        let res  = api.dedup_blob(data).unwrap();
        assert!(api.is_known(&res.blob_id));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupHealthCheck — contrôle de santé du moteur de déduplication
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'un contrôle de santé.
#[derive(Debug, Clone)]
pub struct DedupHealthStatus {
    pub registry_ok:    bool,
    pub sharing_ok:     bool,
    pub index_ok:       bool,
    pub stats_ok:       bool,
    pub overall_ok:     bool,
}

impl DedupHealthStatus {
    pub fn all_ok() -> Self {
        Self { registry_ok: true, sharing_ok: true, index_ok: true, stats_ok: true, overall_ok: true }
    }
}

impl DedupApi {
    /// Contrôle de santé complet.
    pub fn health_check(&self) -> DedupHealthStatus {
        let registry_ok = BLOB_REGISTRY.verify_integrity().is_ok();
        let sharing_ok  = BLOB_SHARING.verify_integrity().is_ok();
        let index_ok    = CHUNK_INDEX.verify_integrity().is_ok();
        let stats       = DEDUP_STATS.snapshot();
        let stats_ok    = stats.is_consistent();
        let overall_ok  = registry_ok && sharing_ok && index_ok && stats_ok;
        DedupHealthStatus { registry_ok, sharing_ok, index_ok, stats_ok, overall_ok }
    }

    /// Retourne un rapport synthétique de l'état du moteur.
    pub fn summary(&self) -> DedupApiSummary {
        let stats    = DEDUP_STATS.snapshot();
        let reg_s    = BLOB_REGISTRY.stats();
        let idx_s    = CHUNK_INDEX.stats();
        DedupApiSummary {
            total_blobs:         reg_s.total_blobs as u64,
            total_unique_chunks: idx_s.total_entries as u64,
            shared_chunks:       idx_s.shared_chunks as u64,
            dedup_ratio_pct:     stats.dedup_ratio_pct,
            saved_bytes:         stats.saved_bytes,
        }
    }
}

/// Résumé de l'état du moteur.
#[derive(Debug, Clone, Copy)]
pub struct DedupApiSummary {
    pub total_blobs:         u64,
    pub total_unique_chunks: u64,
    pub shared_chunks:       u64,
    pub dedup_ratio_pct:     u8,
    pub saved_bytes:         u64,
}

#[cfg(test)]
mod tests_health {
    use super::*;

    #[test] fn test_health_check_clean_state() {
        let api = DedupApi::with_default_policy().unwrap();
        let h   = api.health_check();
        assert!(h.overall_ok);
    }

    #[test] fn test_summary_after_dedup() {
        let api  = DedupApi::with_default_policy().unwrap();
        let data = &[0x99u8; 8192];
        api.dedup_blob(data).unwrap();
        let s = api.summary();
        assert!(s.total_blobs > 0);
    }

    #[test] fn test_verify_integrity_passes() {
        let api = DedupApi::with_default_policy().unwrap();
        assert!(api.verify_integrity().is_ok());
    }

    #[test] fn test_dedup_disabled_policy() {
        let api = DedupApi::new(super::super::dedup_policy::DedupPolicy::disabled()).unwrap();
        let d   = &[0x11u8; 8192];
        let r   = api.dedup_blob(d).unwrap();
        assert_eq!(r.n_chunks, 0);
    }
}
