//! snapshot.rs — Structure et identifiant d'un snapshot ExoFS
//!
//! Définit la représentation disque (ONDISK-03 : pas d'AtomicXxx) et mémoire,
//! ainsi que les opérations de sérialisation/vérification (HDR-03).
//!
//! Règles spec :
//!   ONDISK-03 : types plain uniquement dans #[repr(C)]
//!   HDR-03    : magic vérifié EN PREMIER, puis checksum Blake3
//!   ARITH-02  : checked_add pour toute arithmétique

extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::blob_id::blake3_hash;
use crate::fs::exofs::core::{BlobId, DiskOffset, EpochId, ExofsError, ExofsResult, SnapshotId};

// ─────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────

/// Magic on-disk d'un snapshot : "SNAP"
pub const SNAPSHOT_MAGIC: u32 = 0x534E_4150;

/// Taille fixe de l'en-tête disque (256 octets — ONDISK-03)
pub const SNAPSHOT_HEADER_SIZE: usize = 256;

/// Version courante
pub const SNAPSHOT_FORMAT_VERSION: u8 = 1;

/// Longueur max du nom d'un snapshot
pub const SNAPSHOT_NAME_LEN: usize = 128;

/// Nombre maximal de snapshots simultanés
pub const SNAPSHOT_MAX_COUNT: usize = 1024;

// ─────────────────────────────────────────────────────────────
// Flags
// ─────────────────────────────────────────────────────────────

pub mod flags {
    /// Snapshot en lecture seule (immuable)
    pub const READONLY: u32 = 1 << 0;
    /// Protégé contre la suppression
    pub const PROTECTED: u32 = 1 << 1;
    /// Associé à un stream réseau en cours
    pub const STREAMING: u32 = 1 << 2;
    /// Quota explicitement défini
    pub const QUOTA_SET: u32 = 1 << 3;
    /// Snapshot monté
    pub const MOUNTED: u32 = 1 << 4;
    /// Snapshot en cours de restauration
    pub const RESTORING: u32 = 1 << 5;
    /// Snapshot orph. (parent introuvable)
    pub const ORPHAN: u32 = 1 << 6;
    /// Snapshot de type incrémental
    pub const INCREMENTAL: u32 = 1 << 7;
}

// ─────────────────────────────────────────────────────────────
// Structure disque (ONDISK-03 — types plain uniquement)
// ─────────────────────────────────────────────────────────────

/// En-tête on-disk d'un snapshot (SNAPSHOT_HEADER_SIZE = 256 octets)
///
/// Aucun AtomicXxx — ONDISK-03.
/// Magic vérifié en premier — HDR-03.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SnapshotHeaderDisk {
    /// Magic "SNAP" — vérifié EN PREMIER (HDR-03)
    pub magic: u32,
    /// Version du format
    pub version: u8,
    /// Flags (flags::*)
    pub flags: u32,
    /// _padding
    pub _pad0: [u8; 3],
    /// Identifiant unique du snapshot
    pub id: u64,
    /// Époque de création
    pub epoch_id: u64,
    /// Identifiant du snapshot parent (0 = racine)
    pub parent_id: u64,
    /// Racine Merkle des blob ids (Blake3 concaténé — HASH-02)
    pub root_blob: [u8; 32],
    /// Timestamp de création (ticks)
    pub created_at: u64,
    /// Nombre de blobs contenus
    pub n_blobs: u64,
    /// Taille totale des données (octets)
    pub total_bytes: u64,
    /// Offset disque du catalogue de blobs (0 = inline)
    pub blob_catalog_offset: u64,
    /// Taille du catalogue de blobs sur disque
    pub blob_catalog_size: u32,
    /// Longueur du nom (en octets)
    pub name_len: u16,
    /// _padding
    pub _pad1: [u8; 6],
    /// Nom du snapshot (UTF-8 null-padded)
    pub name: [u8; SNAPSHOT_NAME_LEN],
    /// Checksum Blake3 sur les SNAPSHOT_HEADER_SIZE - 32 premiers octets
    pub checksum: [u8; 32],
}

// const _SH_SIZE: () = assert!(
//     core::mem::size_of::<SnapshotHeaderDisk>() == SNAPSHOT_HEADER_SIZE,
//     "SnapshotHeaderDisk doit faire exactement 256 octets"
// );

impl SnapshotHeaderDisk {
    /// Calcule le checksum Blake3 de l'en-tête (sur les 224 premiers octets)
    pub fn compute_checksum(&self) -> [u8; 32] {
        let body_len = SNAPSHOT_HEADER_SIZE - 32;
        let ptr = self as *const Self as *const u8;
        // SAFETY: repr(C), taille connue statiquement
        let body = unsafe { core::slice::from_raw_parts(ptr, body_len) };
        blake3_hash(body)
    }

    /// HDR-03 : vérifie magic EN PREMIER, puis checksum
    pub fn verify(&self) -> ExofsResult<()> {
        if self.magic != SNAPSHOT_MAGIC {
            return Err(ExofsError::BadMagic);
        }
        if self.version != SNAPSHOT_FORMAT_VERSION {
            return Err(ExofsError::InvalidArgument);
        }
        let expected = self.compute_checksum();
        let mut diff: u8 = 0;
        for i in 0..32 {
            diff |= expected[i] ^ self.checksum[i];
        }
        if diff != 0 {
            return Err(ExofsError::ChecksumMismatch);
        }
        Ok(())
    }

    /// Injecte le checksum calculé
    pub fn finalize(&mut self) {
        self.checksum = self.compute_checksum();
    }

    /// Retourne le tableau d'octets bruts
    pub fn as_bytes(&self) -> &[u8] {
        let ptr = self as *const Self as *const u8;
        // SAFETY: repr(C), taille fixe
        unsafe { core::slice::from_raw_parts(ptr, SNAPSHOT_HEADER_SIZE) }
    }

    /// Parse depuis un tampon brut
    pub fn from_bytes(buf: &[u8]) -> ExofsResult<Self> {
        if buf.len() < SNAPSHOT_HEADER_SIZE {
            return Err(ExofsError::InvalidSize);
        }
        // SAFETY: taille vérifiée, repr(C)
        let hdr: Self = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const Self) };
        Ok(hdr)
    }
}

// ─────────────────────────────────────────────────────────────
// Snapshot en mémoire
// ─────────────────────────────────────────────────────────────

/// Représentation en RAM d'un snapshot
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Identifiant unique
    pub id: SnapshotId,
    /// Époque de création
    pub epoch_id: EpochId,
    /// Snapshot parent (None = racine)
    pub parent_id: Option<SnapshotId>,
    /// Racine Merkle des blobs (HASH-02 : calculé sur données RAW)
    pub root_blob: BlobId,
    /// Timestamp de création
    pub created_at: u64,
    /// Nombre de blobs
    pub n_blobs: u64,
    /// Taille totale des données
    pub total_bytes: u64,
    /// Flags (flags::*)
    pub flags: u32,
    /// Offset catalogue blobs sur disque
    pub blob_catalog_offset: DiskOffset,
    /// Taille du catalogue
    pub blob_catalog_size: u32,
    /// Nom (null-padded)
    pub name: [u8; SNAPSHOT_NAME_LEN],
}

impl Snapshot {
    // ── Accesseurs de flags ───────────────────────────────────────────

    pub fn is_readonly(&self) -> bool {
        self.flags & flags::READONLY != 0
    }
    pub fn is_protected(&self) -> bool {
        self.flags & flags::PROTECTED != 0
    }
    pub fn is_streaming(&self) -> bool {
        self.flags & flags::STREAMING != 0
    }
    pub fn is_mounted(&self) -> bool {
        self.flags & flags::MOUNTED != 0
    }
    pub fn is_restoring(&self) -> bool {
        self.flags & flags::RESTORING != 0
    }
    pub fn is_orphan(&self) -> bool {
        self.flags & flags::ORPHAN != 0
    }
    pub fn is_incremental(&self) -> bool {
        self.flags & flags::INCREMENTAL != 0
    }

    pub fn set_flag(&mut self, flag: u32) {
        self.flags |= flag;
    }
    pub fn clear_flag(&mut self, flag: u32) {
        self.flags &= !flag;
    }
    pub fn toggle_flag(&mut self, flag: u32) {
        self.flags ^= flag;
    }

    // ── Nom ──────────────────────────────────────────────────────────

    /// Retourne le nom comme &str (UTF-8)
    pub fn name_str(&self) -> &str {
        let end = self
            .name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(SNAPSHOT_NAME_LEN);
        core::str::from_utf8(&self.name[..end]).unwrap_or("<invalid>")
    }

    /// Longueur effective du nom
    pub fn name_len(&self) -> u16 {
        self.name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(SNAPSHOT_NAME_LEN) as u16
    }

    // ── Conversion disque ─────────────────────────────────────────────

    /// Construit l'en-tête disque correspondant
    pub fn to_header_disk(&self) -> SnapshotHeaderDisk {
        let mut hdr = SnapshotHeaderDisk {
            magic: SNAPSHOT_MAGIC,
            version: SNAPSHOT_FORMAT_VERSION,
            flags: self.flags,
            _pad0: [0u8; 3],
            id: self.id.0,
            epoch_id: self.epoch_id.0,
            parent_id: self.parent_id.map_or(0, |p| p.0),
            root_blob: *self.root_blob.as_bytes(),
            created_at: self.created_at,
            n_blobs: self.n_blobs,
            total_bytes: self.total_bytes,
            blob_catalog_offset: self.blob_catalog_offset.0,
            blob_catalog_size: self.blob_catalog_size,
            name_len: self.name_len(),
            _pad1: [0u8; 6],
            name: [0u8; SNAPSHOT_NAME_LEN],
            checksum: [0u8; 32],
        };
        hdr.name.copy_from_slice(&self.name);
        hdr.finalize();
        hdr
    }

    /// Désérialise depuis un en-tête disque (HDR-03 : magic + checksum vérifiés)
    pub fn from_header_disk(hdr: &SnapshotHeaderDisk) -> ExofsResult<Self> {
        hdr.verify()?;

        let parent_id = if hdr.parent_id == 0 {
            None
        } else {
            Some(SnapshotId(hdr.parent_id))
        };

        let mut name = [0u8; SNAPSHOT_NAME_LEN];
        name.copy_from_slice(&hdr.name);

        Ok(Self {
            id: SnapshotId(hdr.id),
            epoch_id: EpochId(hdr.epoch_id),
            parent_id,
            root_blob: BlobId(hdr.root_blob),
            created_at: hdr.created_at,
            n_blobs: hdr.n_blobs,
            total_bytes: hdr.total_bytes,
            flags: hdr.flags,
            blob_catalog_offset: DiskOffset(hdr.blob_catalog_offset),
            blob_catalog_size: hdr.blob_catalog_size,
            name,
        })
    }

    // ── Utilitaires stats ─────────────────────────────────────────────

    /// Âge en ticks par rapport à `now`
    pub fn age_ticks(&self, now: u64) -> u64 {
        now.saturating_sub(self.created_at)
    }

    /// Retourne true si ce snapshot est un descendant de `ancestor_id`
    pub fn has_parent(&self, ancestor_id: SnapshotId) -> bool {
        self.parent_id == Some(ancestor_id)
    }
}

// ─────────────────────────────────────────────────────────────
// SnapshotChain — chaîne d'ancêtres
// ─────────────────────────────────────────────────────────────

/// Représente la chaîne de snapshots menant à un snapshot donné
#[derive(Debug)]
pub struct SnapshotChain {
    pub ids: Vec<SnapshotId>,
}

impl SnapshotChain {
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
    pub fn root(&self) -> Option<SnapshotId> {
        self.ids.last().copied()
    }
    pub fn tip(&self) -> Option<SnapshotId> {
        self.ids.first().copied()
    }
}

// ─────────────────────────────────────────────────────────────
// SnapshotRef — référence légère (pas de clone complet)
// ─────────────────────────────────────────────────────────────

/// Référence légère aux métadonnées essentielles d'un snapshot
#[derive(Debug, Clone, Copy)]
pub struct SnapshotRef {
    pub id: SnapshotId,
    pub epoch_id: EpochId,
    pub parent_id: Option<SnapshotId>,
    pub n_blobs: u64,
    pub total_bytes: u64,
    pub flags: u32,
    pub created_at: u64,
}

impl From<&Snapshot> for SnapshotRef {
    fn from(s: &Snapshot) -> Self {
        Self {
            id: s.id,
            epoch_id: s.epoch_id,
            parent_id: s.parent_id,
            n_blobs: s.n_blobs,
            total_bytes: s.total_bytes,
            flags: s.flags,
            created_at: s.created_at,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// Utilitaires
// ─────────────────────────────────────────────────────────────

/// Vérifie un tampon comme en-tête snapshot (HDR-03)
pub fn verify_snapshot_header(buf: &[u8]) -> ExofsResult<SnapshotHeaderDisk> {
    let hdr = SnapshotHeaderDisk::from_bytes(buf)?;
    hdr.verify()?;
    Ok(hdr)
}

/// Construit un nom de snapshot depuis une slice (null-padded)
pub fn make_snapshot_name(src: &[u8]) -> [u8; SNAPSHOT_NAME_LEN] {
    let mut name = [0u8; SNAPSHOT_NAME_LEN];
    let len = src.len().min(SNAPSHOT_NAME_LEN);
    name[..len].copy_from_slice(&src[..len]);
    name
}

// ─────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_disk_size() {
        assert_eq!(
            core::mem::size_of::<SnapshotHeaderDisk>(),
            SNAPSHOT_HEADER_SIZE
        );
    }

    #[test]
    fn roundtrip_header_checksum() {
        let snap = Snapshot {
            id: SnapshotId(42),
            epoch_id: EpochId(7),
            parent_id: None,
            root_blob: BlobId([0xAB; 32]),
            created_at: 1234567,
            n_blobs: 10,
            total_bytes: 4096 * 10,
            flags: flags::READONLY,
            blob_catalog_offset: DiskOffset(0),
            blob_catalog_size: 0,
            name: make_snapshot_name(b"test-snap"),
        };
        let hdr = snap.to_header_disk();
        assert!(hdr.verify().is_ok());
    }

    #[test]
    fn bad_magic_detected() {
        let snap = Snapshot {
            id: SnapshotId(1),
            epoch_id: EpochId(1),
            parent_id: None,
            root_blob: BlobId([0u8; 32]),
            created_at: 0,
            n_blobs: 0,
            total_bytes: 0,
            flags: 0,
            blob_catalog_offset: DiskOffset(0),
            blob_catalog_size: 0,
            name: [0u8; SNAPSHOT_NAME_LEN],
        };
        let mut hdr = snap.to_header_disk();
        hdr.magic = 0xDEAD_BEEF;
        assert!(matches!(hdr.verify(), Err(ExofsError::BadMagic)));
    }

    #[test]
    fn checksum_mismatch_detected() {
        let snap = Snapshot {
            id: SnapshotId(1),
            epoch_id: EpochId(1),
            parent_id: None,
            root_blob: BlobId([0u8; 32]),
            created_at: 0,
            n_blobs: 0,
            total_bytes: 0,
            flags: 0,
            blob_catalog_offset: DiskOffset(0),
            blob_catalog_size: 0,
            name: [0u8; SNAPSHOT_NAME_LEN],
        };
        let mut hdr = snap.to_header_disk();
        hdr.n_blobs = 999; // Modifier sans recalculer le checksum
        assert!(matches!(hdr.verify(), Err(ExofsError::ChecksumMismatch)));
    }

    #[test]
    fn from_header_disk_roundtrip() {
        let snap = Snapshot {
            id: SnapshotId(99),
            epoch_id: EpochId(5),
            parent_id: Some(SnapshotId(1)),
            root_blob: BlobId([0xCC; 32]),
            created_at: 88888,
            n_blobs: 5,
            total_bytes: 20480,
            flags: flags::PROTECTED | flags::READONLY,
            blob_catalog_offset: DiskOffset(4096),
            blob_catalog_size: 256,
            name: make_snapshot_name(b"prod-snapshot"),
        };
        let hdr = snap.to_header_disk();
        let snap2 = Snapshot::from_header_disk(&hdr).unwrap();
        assert_eq!(snap2.id.0, snap.id.0);
        assert_eq!(snap2.n_blobs, snap.n_blobs);
        assert_eq!(snap2.parent_id, snap.parent_id);
        assert!(snap2.is_protected());
    }

    #[test]
    fn name_str_valid() {
        let snap = Snapshot {
            id: SnapshotId(1),
            epoch_id: EpochId(1),
            parent_id: None,
            root_blob: BlobId([0u8; 32]),
            created_at: 0,
            n_blobs: 0,
            total_bytes: 0,
            flags: 0,
            blob_catalog_offset: DiskOffset(0),
            blob_catalog_size: 0,
            name: make_snapshot_name(b"hello"),
        };
        assert_eq!(snap.name_str(), "hello");
    }
}
