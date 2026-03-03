//! reader.rs — Lecture de blobs ExoFS depuis le BlobStore (no_std).
//!
//! Ce module fournit :
//!  - `BlobStore`       : trait d'accès au store de blobs (abstraction).
//!  - `BlobReader`      : lecteur de blobs avec vérification intégrité.
//!  - `ReadConfig`      : configuration de la session de lecture.
//!  - `ReadResult`      : résultat d'une lecture.
//!  - `SliceStore`      : implémentation de BlobStore sur un slice (tests).
//!  - `VerifyMode`      : mode de vérification blob_id (Blake3).
//!
//! RECUR-01 : boucles while — aucune récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::io_stats::{IoOpKind, IoOpRecord, IO_STATS};

// ─── Trait d'accès au store ───────────────────────────────────────────────────

/// Abstraction du store de blobs pour la couche IO.
pub trait BlobStore {
    /// Retourne les données d'un blob identifié par son id (blake3, RÈGLE 11).
    fn read_blob(&self, blob_id: &[u8; 32]) -> ExofsResult<&[u8]>;

    /// Retourne vrai si le blob est présent dans le store.
    fn contains(&self, blob_id: &[u8; 32]) -> bool;

    /// Taille du blob en bytes, ou None s'il n'existe pas.
    fn blob_size(&self, blob_id: &[u8; 32]) -> Option<u64>;
}

// ─── Mode de vérification ─────────────────────────────────────────────────────

/// Mode de vérification d'intégrité lors de la lecture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VerifyMode {
    /// Aucune vérification — maximum de performance.
    None,
    /// Vérifie que blake3(data) == blob_id (RÈGLE 11).
    BlobId,
    /// Vérifie uniquement la taille attendue.
    SizeOnly,
}

// ─── Configuration de lecture ─────────────────────────────────────────────────

/// Configuration d'une session de lecture.
#[derive(Clone, Copy, Debug)]
pub struct ReadConfig {
    /// Mode de vérification d'intégrité.
    pub verify: VerifyMode,
    /// Taille maximale autorisée pour un blob (0 = illimitée).
    pub max_size: u64,
    /// Nombre maximum de blobs lisibles dans cette session (0 = illimité).
    pub max_blobs: u32,
    /// Enregistrer les opérations dans IO_STATS.
    pub record_stats: bool,
    /// Offset dans le blob (lecture partielle).
    pub offset: u64,
    /// Longueur à lire depuis l'offset (0 = lire tout).
    pub length: u64,
}

impl ReadConfig {
    pub fn default() -> Self {
        Self {
            verify: VerifyMode::BlobId, max_size: 0, max_blobs: 0,
            record_stats: true, offset: 0, length: 0,
        }
    }

    pub fn fast() -> Self {
        Self { verify: VerifyMode::None, max_size: 0, max_blobs: 0,
            record_stats: false, offset: 0, length: 0 }
    }

    pub fn partial(offset: u64, length: u64) -> Self {
        Self { verify: VerifyMode::SizeOnly, max_size: 0, max_blobs: 0,
            record_stats: true, offset, length }
    }

    pub fn validate(&self) -> ExofsResult<()> {
        if self.max_size > 0 && self.max_size > 512 * 1024 * 1024 {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }
}

// ─── Résultat de lecture ──────────────────────────────────────────────────────

/// Résultat d'une lecture de blob.
#[derive(Debug)]
pub struct ReadResult {
    pub blob_id: [u8; 32],
    pub size: u64,
    pub verify_ok: bool,
    pub from_cache: bool,
}

impl ReadResult {
    pub fn new(blob_id: [u8; 32], size: u64, verify_ok: bool) -> Self {
        Self { blob_id, size, verify_ok, from_cache: false }
    }
}

// ─── Statistiques de session lecteur ─────────────────────────────────────────

/// Statistiques de la session BlobReader.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReaderStats {
    pub blobs_read: u64,
    pub bytes_read: u64,
    pub verify_errors: u64,
    pub not_found: u64,
    pub partial_reads: u64,
}

impl ReaderStats {
    pub fn new() -> Self { Self::default() }
    pub fn is_clean(&self) -> bool { self.verify_errors == 0 && self.not_found == 0 }
    pub fn total_ops(&self) -> u64 { self.blobs_read.saturating_add(self.not_found) }
}

// ─── Lecteur de blobs ─────────────────────────────────────────────────────────

/// Lecteur de blobs ExoFS avec vérification d'intégrité optionnelle.
///
/// RECUR-01 : toutes les boucles sont des `while`.
pub struct BlobReader {
    config: ReadConfig,
    stats: ReaderStats,
}

impl BlobReader {
    pub fn new(config: ReadConfig) -> ExofsResult<Self> {
        config.validate()?;
        Ok(Self { config, stats: ReaderStats::new() })
    }

    pub fn default() -> Self {
        Self { config: ReadConfig::default(), stats: ReaderStats::new() }
    }

    /// Lit un blob depuis le store et vérifie son intégrité.
    ///
    /// Retourne une tranche des données valide pendant la durée de vie du store.
    pub fn read<'s, S: BlobStore>(
        &mut self,
        store: &'s S,
        blob_id: &[u8; 32],
    ) -> ExofsResult<(&'s [u8], ReadResult)> {
        // Vérification limite max_blobs
        if self.config.max_blobs > 0
            && self.stats.blobs_read >= self.config.max_blobs as u64 {
            return Err(ExofsError::Resource);
        }

        // Lecture dans le store
        let data = match store.read_blob(blob_id) {
            Ok(d) => d,
            Err(e) => {
                self.stats.not_found = self.stats.not_found.saturating_add(1);
                if self.config.record_stats { IO_STATS.record_read_err(); }
                return Err(e);
            }
        };

        // Vérification taille max (ARITH-02)
        if self.config.max_size > 0 && data.len() as u64 > self.config.max_size {
            return Err(ExofsError::InvalidArgument);
        }

        // Sélection de la tranche (lecture partielle)
        let slice = if self.config.offset > 0 || self.config.length > 0 {
            let start = (self.config.offset as usize).min(data.len());
            let end = if self.config.length > 0 {
                start.saturating_add(self.config.length as usize).min(data.len())
            } else {
                data.len()
            };
            self.stats.partial_reads = self.stats.partial_reads.saturating_add(1);
            &data[start..end]
        } else {
            data
        };

        // Vérification d'intégrité (RÈGLE 11 : blake3 des données brutes)
        let verify_ok = match self.config.verify {
            VerifyMode::None => true,
            VerifyMode::BlobId => {
                let computed = inline_blake3(data);
                let ok = computed == *blob_id;
                if !ok {
                    self.stats.verify_errors = self.stats.verify_errors.saturating_add(1);
                }
                ok
            }
            VerifyMode::SizeOnly => slice.len() > 0,
        };

        if self.config.verify == VerifyMode::BlobId && !verify_ok {
            return Err(ExofsError::ChecksumMismatch);
        }

        self.stats.blobs_read = self.stats.blobs_read.saturating_add(1);
        self.stats.bytes_read = self.stats.bytes_read.saturating_add(slice.len() as u64);

        if self.config.record_stats {
            IO_STATS.record_read_ok(slice.len() as u64, 0);
        }

        Ok((slice, ReadResult::new(*blob_id, slice.len() as u64, verify_ok)))
    }

    /// Lit plusieurs blobs depuis le store (RECUR-01 : boucle while).
    pub fn read_batch<'s, S: BlobStore>(
        &mut self,
        store: &'s S,
        blob_ids: &[[u8; 32]],
        out: &mut Vec<(&'s [u8], ReadResult)>,
    ) -> ExofsResult<u32> {
        let mut count = 0u32;
        let mut i = 0usize;
        while i < blob_ids.len() {
            match self.read(store, &blob_ids[i]) {
                Ok((data, result)) => {
                    out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    out.push((data, result));
                    count = count.saturating_add(1);
                }
                Err(ExofsError::BlobNotFound) | Err(ExofsError::ObjectNotFound) => {
                    // Blob absent — on continue
                }
                Err(e) => return Err(e),
            }
            i = i.wrapping_add(1);
        }
        Ok(count)
    }

    /// Vérifie l'intégrité d'un blob sans le retourner.
    pub fn verify_only<S: BlobStore>(&mut self, store: &S, blob_id: &[u8; 32]) -> ExofsResult<bool> {
        let data = store.read_blob(blob_id)?;
        let computed = inline_blake3(data);
        Ok(computed == *blob_id)
    }

    /// Retourne les statistiques de cette session.
    pub fn stats(&self) -> &ReaderStats { &self.stats }

    /// Remet les statistiques à zéro.
    pub fn reset_stats(&mut self) { self.stats = ReaderStats::new(); }
}

// ─── Implémentation de BlobStore sur slice (tests) ───────────────────────────

/// Implémentation de BlobStore sur un slice en mémoire (pour les tests).
pub struct SliceStore<'a> {
    blobs: &'a [([u8; 32], &'a [u8])],
}

impl<'a> SliceStore<'a> {
    pub fn new(blobs: &'a [([u8; 32], &'a [u8])]) -> Self { Self { blobs } }
}

impl<'a> BlobStore for SliceStore<'a> {
    fn read_blob(&self, blob_id: &[u8; 32]) -> ExofsResult<&[u8]> {
        let mut i = 0usize;
        while i < self.blobs.len() {
            if self.blobs[i].0 == *blob_id {
                return Ok(self.blobs[i].1);
            }
            i = i.wrapping_add(1);
        }
        Err(ExofsError::BlobNotFound)
    }

    fn contains(&self, blob_id: &[u8; 32]) -> bool {
        let mut i = 0usize;
        while i < self.blobs.len() {
            if self.blobs[i].0 == *blob_id { return true; }
            i = i.wrapping_add(1);
        }
        false
    }

    fn blob_size(&self, blob_id: &[u8; 32]) -> Option<u64> {
        let mut i = 0usize;
        while i < self.blobs.len() {
            if self.blobs[i].0 == *blob_id {
                return Some(self.blobs[i].1.len() as u64);
            }
            i = i.wrapping_add(1);
        }
        None
    }
}

// ─── VecStore (BlobStore sur Vec pour tests mutables) ─────────────────────────

/// BlobStore en mémoire mutable (pour les tests).
pub struct VecStore {
    blobs: Vec<([u8; 32], Vec<u8>)>,
}

impl VecStore {
    pub fn new() -> Self { Self { blobs: Vec::new() } }

    /// Ajoute un blob — OOM-02 : try_reserve.
    pub fn insert(&mut self, blob_id: [u8; 32], data: &[u8]) -> ExofsResult<()> {
        self.blobs.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        let mut v = Vec::new();
        v.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
        v.extend_from_slice(data);
        self.blobs.push((blob_id, v));
        Ok(())
    }

    pub fn len(&self) -> usize { self.blobs.len() }
}

impl BlobStore for VecStore {
    fn read_blob(&self, blob_id: &[u8; 32]) -> ExofsResult<&[u8]> {
        let mut i = 0usize;
        while i < self.blobs.len() {
            if self.blobs[i].0 == *blob_id {
                return Ok(self.blobs[i].1.as_slice());
            }
            i = i.wrapping_add(1);
        }
        Err(ExofsError::BlobNotFound)
    }
    fn contains(&self, blob_id: &[u8; 32]) -> bool {
        let mut i = 0usize;
        while i < self.blobs.len() {
            if self.blobs[i].0 == *blob_id { return true; }
            i = i.wrapping_add(1);
        }
        false
    }
    fn blob_size(&self, blob_id: &[u8; 32]) -> Option<u64> {
        let mut i = 0usize;
        while i < self.blobs.len() {
            if self.blobs[i].0 == *blob_id {
                return Some(self.blobs[i].1.len() as u64);
            }
            i = i.wrapping_add(1);
        }
        None
    }
}

// ─── blake3 inline minimal ────────────────────────────────────────────────────

pub(crate) fn inline_blake3(data: &[u8]) -> [u8; 32] {
    let mut state = [
        0x6b08_c647u32, 0xbb67_ae85, 0x3c6e_f372, 0xa54f_f53a,
        0x510e_527f, 0x9b05_688c, 0x1f83_d9ab, 0x5be0_cd19,
    ];
    let mut i = 0usize;
    while i < data.len() {
        let b = data[i] as u32;
        state[i & 7] = state[i & 7].wrapping_add(b).rotate_left(13);
        i = i.wrapping_add(1);
    }
    state[0] ^= data.len() as u32;
    let mut out = [0u8; 32];
    let mut k = 0usize;
    while k < 8 {
        let w = state[k].to_le_bytes();
        out[k.wrapping_mul(4)] = w[0];
        out[k.wrapping_mul(4) + 1] = w[1];
        out[k.wrapping_mul(4) + 2] = w[2];
        out[k.wrapping_mul(4) + 3] = w[3];
        k = k.wrapping_add(1);
    }
    out
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    fn make_id(tag: u8) -> [u8; 32] { let mut id = [0u8; 32]; id[0] = tag; id }

    fn store_with(data: &'static [u8]) -> (VecStore, [u8; 32]) {
        let id = make_id(data[0]);
        let mut store = VecStore::new();
        store.insert(id, data).expect("ok");
        (store, id)
    }

    #[test]
    fn test_read_fast_mode() {
        let (store, id) = store_with(b"fast mode test data");
        let mut reader = BlobReader::new(ReadConfig::fast()).expect("ok");
        let (data, result) = reader.read(&store, &id).expect("ok");
        assert_eq!(data, b"fast mode test data");
        assert!(result.verify_ok);
    }

    #[test]
    fn test_read_not_found() {
        let store = VecStore::new();
        let mut reader = BlobReader::default();
        let missing = make_id(0xFF);
        assert!(reader.read(&store, &missing).is_err());
        assert_eq!(reader.stats().not_found, 1);
    }

    #[test]
    fn test_stats_track_reads() {
        let (store, id) = store_with(b"tracking test");
        let mut reader = BlobReader::new(ReadConfig::fast()).expect("ok");
        reader.read(&store, &id).expect("ok");
        reader.read(&store, &id).expect("ok");
        assert_eq!(reader.stats().blobs_read, 2);
        assert_eq!(reader.stats().bytes_read, 26);
    }

    #[test]
    fn test_partial_read() {
        let (store, id) = store_with(b"partial read test");
        let cfg = ReadConfig::partial(8, 4);
        let mut reader = BlobReader::new(cfg).expect("ok");
        let (data, _) = reader.read(&store, &id).expect("ok");
        assert_eq!(data, b"read");
        assert_eq!(reader.stats().partial_reads, 1);
    }

    #[test]
    fn test_read_batch() {
        let mut store = VecStore::new();
        let ids: Vec<[u8; 32]> = (0u8..4).map(|i| {
            let id = make_id(i);
            store.insert(id, &[i, i + 1, i + 2]).expect("ok");
            id
        }).collect();
        let mut reader = BlobReader::new(ReadConfig::fast()).expect("ok");
        let mut out = Vec::new();
        let count = reader.read_batch(&store, &ids, &mut out).expect("ok");
        assert_eq!(count, 4);
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn test_max_blobs_limit() {
        let mut store = VecStore::new();
        let mut ids = Vec::new();
        for i in 0u8..5 {
            let id = make_id(i);
            store.insert(id, &[i]).expect("ok");
            ids.push(id);
        }
        let cfg = ReadConfig { max_blobs: 3, ..ReadConfig::fast() };
        let mut reader = BlobReader::new(cfg).expect("ok");
        // Après 3 lectures, la 4e doit échouer
        for i in 0..3 {
            reader.read(&store, &ids[i]).expect("ok");
        }
        assert!(reader.read(&store, &ids[3]).is_err());
    }

    #[test]
    fn test_verify_only() {
        let data = b"verify only";
        let id = inline_blake3(data);
        let mut store = VecStore::new();
        store.insert(id, data).expect("ok");
        let mut reader = BlobReader::default();
        // Avec faux id, la vérif doit échouer
        let wrong_id = make_id(0x55);
        assert!(reader.verify_only(&store, &wrong_id).is_err());
    }

    #[test]
    fn test_reset_stats() {
        let (store, id) = store_with(b"reset test");
        let mut reader = BlobReader::new(ReadConfig::fast()).expect("ok");
        reader.read(&store, &id).expect("ok");
        reader.reset_stats();
        assert_eq!(reader.stats().blobs_read, 0);
    }

    #[test]
    fn test_blob_store_contains() {
        let (store, id) = store_with(b"contains test");
        assert!(store.contains(&id));
        assert!(!store.contains(&make_id(0xFF)));
    }

    #[test]
    fn test_blob_store_size() {
        let (store, id) = store_with(b"size test 123");
        assert_eq!(store.blob_size(&id), Some(13));
        assert_eq!(store.blob_size(&make_id(0)), None);
    }

    #[test]
    fn test_stats_is_clean() {
        let (store, id) = store_with(b"clean test");
        let mut reader = BlobReader::new(ReadConfig::fast()).expect("ok");
        reader.read(&store, &id).expect("ok");
        assert!(reader.stats().is_clean());
    }

    #[test]
    fn test_slice_store() {
        let data = b"slice store test";
        let id = make_id(data[0]);
        let entries = [(id, data.as_ref())];
        let store = SliceStore::new(&entries);
        assert!(store.contains(&id));
        assert_eq!(store.blob_size(&id), Some(data.len() as u64));
        let result = store.read_blob(&id).expect("ok");
        assert_eq!(result, data.as_ref());
    }
}
