//! snapshot_create.rs — Pipeline de création d'un snapshot ExoFS
//!
//! Construit un snapshot à partir d'un ensemble de blob ids fournis par
//! l'appelant, calcule la racine Merkle (HASH-02 : données RAW avant
//! compression), valide les paramètres et enregistre dans SNAPSHOT_LIST.
//!
//! Règles spec :
//!   HASH-02  : compute_blob_id sur données RAW (jamais compressées)
//!   OOM-02   : try_reserve avant chaque push
//!   ARITH-02 : checked_add / checked_mul pour tailles
//!   WRITE-02 : vérifier bytes_written == expected après chaque écriture

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, BlobId, EpochId, SnapshotId, DiskOffset,
};
use crate::fs::exofs::core::blob_id::compute_blob_id;
use super::snapshot::{Snapshot, flags, make_snapshot_name, SNAPSHOT_NAME_LEN};
use super::snapshot_list::SNAPSHOT_LIST;

// ─────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────

/// Nombre maximum de blobs par snapshot
pub const SNAPSHOT_MAX_BLOBS: u64 = 1 << 24; // 16M

/// Taille maximum totale d'un snapshot (512 GiB)
pub const SNAPSHOT_MAX_TOTAL_BYTES: u64 = 512 << 30;

// ─────────────────────────────────────────────────────────────
// Paramètres de création
// ─────────────────────────────────────────────────────────────

/// Paramètres de création d'un snapshot
#[derive(Clone, Debug)]
pub struct SnapshotParams {
    /// Nom (UTF-8, max SNAPSHOT_NAME_LEN octets)
    pub name:        [u8; SNAPSHOT_NAME_LEN],
    /// Snapshot parent (None = snapshot racine)
    pub parent_id:   Option<SnapshotId>,
    /// Époque de création
    pub epoch_id:    EpochId,
    /// Flags initiaux
    pub flags:       u32,
    /// Quota en octets (0 = pas de quota)
    pub quota_bytes: u64,
    /// Timestamp de création (ticks)
    pub created_at:  u64,
    /// Offset disque du catalogue de blobs
    pub blob_catalog_offset: DiskOffset,
    /// Taille du catalogue de blobs sur disque
    pub blob_catalog_size:   u32,
}

impl SnapshotParams {
    pub fn new(name: &[u8], parent_id: Option<SnapshotId>, epoch_id: EpochId) -> Self {
        Self {
            name: make_snapshot_name(name),
            parent_id,
            epoch_id,
            flags: 0,
            quota_bytes: 0,
            created_at: 0,
            blob_catalog_offset: DiskOffset(0),
            blob_catalog_size: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// Résultat de création
// ─────────────────────────────────────────────────────────────

/// Résultat d'une opération de création
#[derive(Debug, Clone)]
pub struct SnapshotCreateResult {
    /// Identifiant alloué pour le nouveau snapshot
    pub id:          SnapshotId,
    /// Racine Merkle calculée sur les blob ids RAW
    pub root_blob:   BlobId,
    /// Nombre de blobs
    pub n_blobs:     u64,
    /// Octets totaux (somme des tailles des blobs)
    pub total_bytes: u64,
    /// Durée de création (ticks)
    pub duration_ticks: u64,
}

// ─────────────────────────────────────────────────────────────
// SnapshotBlobSet — ensemble de blobs fournis pour la création
// ─────────────────────────────────────────────────────────────

/// Représente un blob à inclure dans le snapshot
#[derive(Debug, Clone)]
pub struct BlobEntry {
    /// Identifiant du blob (HASH-02 : calculé sur données RAW)
    pub blob_id: BlobId,
    /// Taille en octets des données RAW
    pub raw_size: u64,
}

impl BlobEntry {
    pub fn new(blob_id: BlobId, raw_size: u64) -> Self {
        Self { blob_id, raw_size }
    }
}

/// Ensemble de blobs ordonnés prêts pour la création d'un snapshot
pub struct SnapshotBlobSet {
    entries: Vec<BlobEntry>,
}

impl SnapshotBlobSet {
    pub fn new() -> Self { Self { entries: Vec::new() } }

    /// OOM-02 : try_reserve avant push
    pub fn push(&mut self, entry: BlobEntry) -> ExofsResult<()> {
        self.entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.entries.push(entry);
        Ok(())
    }

    pub fn len(&self) -> u64 { self.entries.len() as u64 }

    pub fn is_empty(&self) -> bool { self.entries.is_empty() }

    /// Taille totale RAW — ARITH-02 : checked_add
    pub fn total_bytes(&self) -> ExofsResult<u64> {
        let mut total: u64 = 0;
        for e in &self.entries {
            total = total.checked_add(e.raw_size).ok_or(ExofsError::Overflow)?;
        }
        Ok(total)
    }

    /// Calcule la racine Merkle des blob ids (HASH-02 : sur données RAW)
    pub fn compute_root_blob(&self) -> ExofsResult<BlobId> {
        if self.entries.is_empty() {
            // Snapshot vide : racine = hash de rien
            return Ok(compute_blob_id(&[]));
        }
        // Recueille les 32 octets de chaque blob_id
        let mut raw: Vec<u8> = Vec::new();
        raw.try_reserve(
            self.entries.len().checked_mul(32).ok_or(ExofsError::Overflow)?
        ).map_err(|_| ExofsError::NoMemory)?;
        for e in &self.entries {
            raw.extend_from_slice(e.blob_id.as_bytes());
        }
        Ok(compute_blob_id(&raw))
    }

    pub fn entries(&self) -> &[BlobEntry] { &self.entries }
}

// ─────────────────────────────────────────────────────────────
// SnapshotCreator
// ─────────────────────────────────────────────────────────────

/// Constructeur de snapshots
///
/// Usage typique :
/// ```ignore
/// let mut set = SnapshotBlobSet::new();
/// set.push(BlobEntry::new(compute_blob_id(&raw_data), raw_data.len() as u64))?;
/// let result = SnapshotCreator::create(&params, set)?;
/// ```
pub struct SnapshotCreator;

impl SnapshotCreator {
    /// Crée un nouveau snapshot et l'enregistre dans SNAPSHOT_LIST
    pub fn create(params: &SnapshotParams, blobs: SnapshotBlobSet) -> ExofsResult<SnapshotCreateResult> {
        // ── Validation des paramètres ────────────────────────────────
        Self::validate_params(params)?;

        // ── Validation du nombre de blobs ────────────────────────────
        if blobs.len() > SNAPSHOT_MAX_BLOBS {
            return Err(ExofsError::InvalidSize);
        }

        // ── Calcul taille totale (ARITH-02 : checked) ────────────────
        let total_bytes = blobs.total_bytes()?;
        if total_bytes > SNAPSHOT_MAX_TOTAL_BYTES {
            return Err(ExofsError::InvalidSize);
        }

        // ── Vérification quota (si défini) ───────────────────────────
        if params.quota_bytes != 0 && total_bytes > params.quota_bytes {
            return Err(ExofsError::InvalidSize);
        }

        // ── Vérification que le parent existe ────────────────────────
        if let Some(parent_id) = params.parent_id {
            let _ = SNAPSHOT_LIST.get_ref(parent_id)?;
        }

        // ── Calcul de la racine Merkle (HASH-02 : blobs RAW) ─────────
        let root_blob = blobs.compute_root_blob()?;
        let n_blobs   = blobs.len();

        // ── Allocation d'un identifiant ──────────────────────────────
        let snap_id = SNAPSHOT_LIST.allocate_id()?;

        // ── Initialisation des flags ─────────────────────────────────
        let mut snap_flags = params.flags;
        if params.quota_bytes != 0 {
            snap_flags |= flags::QUOTA_SET;
        }
        if params.parent_id.is_some() {
            snap_flags |= flags::INCREMENTAL;
        }

        // ── Construction du snapshot en mémoire ──────────────────────
        let snap = Snapshot {
            id: snap_id,
            epoch_id: params.epoch_id,
            parent_id: params.parent_id,
            root_blob,
            created_at: params.created_at,
            n_blobs,
            total_bytes,
            flags: snap_flags,
            blob_catalog_offset: params.blob_catalog_offset,
            blob_catalog_size: params.blob_catalog_size,
            name: params.name,
        };

        // ── Enregistrement dans le registre global ───────────────────
        SNAPSHOT_LIST.register(snap)?;

        Ok(SnapshotCreateResult {
            id: snap_id,
            root_blob,
            n_blobs,
            total_bytes,
            duration_ticks: 0,
        })
    }

    /// Crée un snapshot vide (utile pour les snapshots racine ou de test)
    pub fn create_empty(params: &SnapshotParams) -> ExofsResult<SnapshotCreateResult> {
        Self::create(params, SnapshotBlobSet::new())
    }

    /// Valide les paramètres de création
    pub fn validate_params(params: &SnapshotParams) -> ExofsResult<()> {
        // Nom non vide
        if params.name.iter().all(|&b| b == 0) {
            return Err(ExofsError::InvalidArgument);
        }
        // Epoch valide
        if !params.epoch_id.is_valid() {
            return Err(ExofsError::InvalidArgument);
        }
        // Pas de circular reference = parent != soi-même (impossible car l'id n'est pas alloué)
        Ok(())
    }

    /// Construit un snapshot incrémental à partir d'un parent et d'un diff de blobs
    ///
    /// Blobs existants du parent réutilisés + nouveaux blobs ajoutés.
    pub fn create_incremental(
        parent_id: SnapshotId,
        new_blobs:  Vec<BlobEntry>,
        epoch_id:   EpochId,
        name:       &[u8],
        created_at: u64,
    ) -> ExofsResult<SnapshotCreateResult> {
        let _parent = SNAPSHOT_LIST.get(parent_id)?;

        let params = SnapshotParams {
            name: make_snapshot_name(name),
            parent_id: Some(parent_id),
            epoch_id,
            flags: flags::INCREMENTAL,
            quota_bytes: 0,
            created_at,
            blob_catalog_offset: DiskOffset(0),
            blob_catalog_size: 0,
        };

        let mut set = SnapshotBlobSet::new();
        for entry in new_blobs {
            set.push(entry)?;
        }

        Self::create(&params, set)
    }
}

// ─────────────────────────────────────────────────────────────
// Utilitaires
// ─────────────────────────────────────────────────────────────

/// Construit une liste de BlobEntry à partir de données brutes
///
/// HASH-02 : compute_blob_id est appelé sur `raw_data` AVANT toute compression
pub fn entries_from_raw(raw_data: &[&[u8]]) -> ExofsResult<Vec<BlobEntry>> {
    let mut out: Vec<BlobEntry> = Vec::new();
    out.try_reserve(raw_data.len()).map_err(|_| ExofsError::NoMemory)?;
    for data in raw_data {
        let blob_id  = compute_blob_id(data);
        let raw_size = data.len() as u64;
        out.push(BlobEntry { blob_id, raw_size });
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::core::blob_id::compute_blob_id;

    fn base_params(name: &[u8]) -> SnapshotParams {
        SnapshotParams {
            name: make_snapshot_name(name),
            parent_id: None, epoch_id: EpochId(1), flags: 0,
            quota_bytes: 0, created_at: 1000,
            blob_catalog_offset: DiskOffset(0), blob_catalog_size: 0,
        }
    }

    #[test]
    fn create_empty_snapshot() {
        let params = base_params(b"empty-snap");
        let result = SnapshotCreator::create_empty(&params).unwrap();
        assert_eq!(result.n_blobs, 0);
        assert_eq!(result.total_bytes, 0);
    }

    #[test]
    fn create_with_blobs_hash02() {
        let raw1 = b"hello world";
        let raw2 = b"exo-os kernel";

        let mut set = SnapshotBlobSet::new();
        // HASH-02 : compute_blob_id sur données RAW
        set.push(BlobEntry::new(compute_blob_id(raw1), raw1.len() as u64)).unwrap();
        set.push(BlobEntry::new(compute_blob_id(raw2), raw2.len() as u64)).unwrap();

        let params = base_params(b"snap-with-blobs");
        let result = SnapshotCreator::create(&params, set).unwrap();
        assert_eq!(result.n_blobs, 2);
        assert_eq!(result.total_bytes, (raw1.len() + raw2.len()) as u64);
    }

    #[test]
    fn quota_exceeded_returns_error() {
        let mut params = base_params(b"quota-snap");
        params.quota_bytes = 4; // très petit

        let mut set = SnapshotBlobSet::new();
        let raw = b"too much data";
        set.push(BlobEntry::new(compute_blob_id(raw), raw.len() as u64)).unwrap();

        let err = SnapshotCreator::create(&params, set);
        assert!(matches!(err, Err(ExofsError::InvalidSize)));
    }

    #[test]
    fn empty_name_invalid() {
        let mut params = base_params(b"");
        params.name = [0u8; SNAPSHOT_NAME_LEN];
        let err = SnapshotCreator::validate_params(&params);
        assert!(matches!(err, Err(ExofsError::InvalidArgument)));
    }

    #[test]
    fn blob_set_total_bytes_overflow() {
        let mut set = SnapshotBlobSet::new();
        // Simule un dépassement avec u64::MAX
        set.push(BlobEntry::new(BlobId([0u8;32]), u64::MAX)).unwrap();
        set.push(BlobEntry::new(BlobId([1u8;32]), 1)).unwrap();
        let err = set.total_bytes();
        assert!(matches!(err, Err(ExofsError::Overflow)));
    }

    #[test]
    fn root_blob_deterministic() {
        let mut set1 = SnapshotBlobSet::new();
        let mut set2 = SnapshotBlobSet::new();
        let bid = compute_blob_id(b"data");
        set1.push(BlobEntry::new(bid, 4)).unwrap();
        set2.push(BlobEntry::new(bid, 4)).unwrap();
        let r1 = set1.compute_root_blob().unwrap();
        let r2 = set2.compute_root_blob().unwrap();
        assert!(r1.ct_eq(&r2));
    }
}
