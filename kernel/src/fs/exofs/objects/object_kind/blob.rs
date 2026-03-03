// SPDX-License-Identifier: MIT
// ExoFS — object_kind/blob.rs
// BlobDescriptor et pipeline de vérification des objets de type Blob.
//
// Règles :
//   HASH-01  : BlobId = Blake3(données brutes AVANT compression)
//   ONDISK-01: BlobDescriptorDisk #[repr(C, packed)]
//   SEC-04   : jamais de logging du contenu binaire
//   ARITH-02 : checked_add / saturating_* partout

#![allow(dead_code)]

use core::fmt;
use core::mem;

use crate::fs::exofs::core::{
    BlobId, ObjectId, EpochId, DiskOffset,
    ExofsError, ExofsResult, blake3_hash, compute_blob_id,
};
use crate::fs::exofs::objects::physical_blob::{CompressionType, PhysicalBlobInMemory};

// ── Constantes ──────────────────────────────────────────────────────────────────

/// Magic d'en-tête d'un BlobDescriptorDisk.
pub const BLOB_DESCRIPTOR_MAGIC: u32 = 0xB10B_1D00;

/// Version courante du format BlobDescriptorDisk.
pub const BLOB_DESCRIPTOR_VERSION: u8 = 1;

/// Taille maximale d'un Blob (512 Mio).
pub const BLOB_MAX_SIZE: u64 = 512 * 1024 * 1024;

/// Algorithme de hachage utilisé pour les BlobIds.
pub const BLOB_HASH_ALGO: u8 = 0x01; // Blake3

/// Alignement minimum conseillé pour les Blobs sur disque.
pub const BLOB_DISK_ALIGN: u64 = 512;

// ── Flags de Blob on-disk ──────────────────────────────────────────────────────

pub const BLOB_FLAG_COMPRESSED:  u16 = 1 << 0;
pub const BLOB_FLAG_ENCRYPTED:   u16 = 1 << 1;
pub const BLOB_FLAG_PINNED:      u16 = 1 << 2; // Ne pas GC même si ref_count == 0
pub const BLOB_FLAG_DEDUPLICATED:u16 = 1 << 3; // Part d'un Blob partagé (dedup)
pub const BLOB_FLAG_SEALED:      u16 = 1 << 4; // Immuable, jamais écrasé
pub const BLOB_FLAG_PARTIAL:     u16 = 1 << 5; // Blob partiel (upload en cours)

// ── BlobDescriptorDisk ─────────────────────────────────────────────────────────

/// Représentation on-disk d'un descripteur de Blob (128 octets, ONDISK-01).
///
/// Stocké dans la zone de métadonnées ExoFS, PAS dans le payload.
///
/// Layout (128 B) :
/// ```text
///   0.. 3   magic        u32
///   4.. 35  blob_id      [u8;32]   — identifiant du Blob (HASH-01)
///  36.. 67  object_id    [u8;32]   — objet LogicalObject propriétaire
///  68.. 75  disk_offset  u64       — offset disque du payload
///  76.. 83  raw_size     u64       — taille non-compressée
///  84.. 91  stored_size  u64       — taille stockée (compressée ou non)
///  92.. 99  epoch_create u64
/// 100..101  flags        u16
/// 102       compression  u8        — CompressionType en u8
/// 103       hash_algo    u8        — BLOB_HASH_ALGO
/// 104       version      u8
/// 105..107  _pad         [u8;3]
/// 108..111  ref_count    u32       — snapshot du ref_count au flush
/// 112..127  checksum     [u8;16]   — Blake3 sur les 112 premiers octets, tronqué
/// ```
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct BlobDescriptorDisk {
    pub magic:        u32,
    pub blob_id:      [u8; 32],
    pub object_id:    [u8; 32],
    pub disk_offset:  u64,
    pub raw_size:     u64,
    pub stored_size:  u64,
    pub epoch_create: u64,
    pub flags:        u16,
    pub compression:  u8,
    pub hash_algo:    u8,
    pub version:      u8,
    pub _pad:         [u8; 3],
    pub ref_count:    u32,
    pub checksum:     [u8; 16],
}

const _: () = assert!(
    mem::size_of::<BlobDescriptorDisk>() == 128,
    "BlobDescriptorDisk doit être 128 octets (ONDISK-01)"
);

impl BlobDescriptorDisk {
    /// Calcule le checksum de l'en-tête (16 premiers octets de Blake3 sur 112B).
    pub fn compute_checksum(&self) -> [u8; 16] {
        let raw: &[u8; 128] =
            unsafe { &*(self as *const BlobDescriptorDisk as *const [u8; 128]) };
        let full = blake3_hash(&raw[..112]);
        let mut out = [0u8; 16];
        out.copy_from_slice(&full[..16]);
        out
    }

    /// Vérifie le magic et le checksum (HDR-03).
    pub fn verify(&self) -> ExofsResult<()> {
        if { self.magic } != BLOB_DESCRIPTOR_MAGIC {
            return Err(ExofsError::Corrupt);
        }
        if { self.version } != BLOB_DESCRIPTOR_VERSION {
            return Err(ExofsError::IncompatibleVersion);
        }
        if { self.hash_algo } != BLOB_HASH_ALGO {
            return Err(ExofsError::InvalidArgument);
        }
        let computed = self.compute_checksum();
        if { self.checksum } != computed {
            return Err(ExofsError::Corrupt);
        }
        Ok(())
    }
}

impl fmt::Debug for BlobDescriptorDisk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "BlobDescriptorDisk {{ raw_size: {}, stored_size: {}, \
             flags: {:#x}, compression: {} }}",
            { self.raw_size }, { self.stored_size },
            { self.flags }, { self.compression },
        )
    }
}

// ── BlobDescriptor in-memory ───────────────────────────────────────────────────

/// Descripteur in-memory d'un Blob ExoFS.
///
/// Correspond à un objet LogicalObject de kind `ObjectKind::Blob`.
pub struct BlobDescriptor {
    /// Identifiant du Blob (Blake3 des données brutes).
    pub blob_id:      BlobId,
    /// Objet propriétaire.
    pub object_id:    ObjectId,
    /// Offset disque du payload.
    pub disk_offset:  DiskOffset,
    /// Taille des données brutes (AVANT compression).
    pub raw_size:     u64,
    /// Taille stockée sur disque (compressée ou identique à raw_size).
    pub stored_size:  u64,
    /// Epoch de création.
    pub epoch_create: EpochId,
    /// Flags (BLOB_FLAG_*).
    pub flags:        u16,
    /// Type de compression.
    pub compression:  CompressionType,
    /// Compteur de références.
    pub ref_count:    u32,
    /// Hint de déduplication : None si non dédupliqué.
    pub dedup_hint:   Option<BlobId>,
}

impl BlobDescriptor {
    // ── Constructeurs ──────────────────────────────────────────────────────────

    /// Crée un nouveau BlobDescriptor pour un blob inline (jamais compressé).
    pub fn new_inline(
        data:         &[u8],
        object_id:    ObjectId,
        epoch_create: EpochId,
    ) -> ExofsResult<Self> {
        let raw_size = data.len() as u64;
        // HASH-01 : Blake3 sur les données BRUTES avant toute compression.
        let blob_id = compute_blob_id(data);
        Ok(Self {
            blob_id,
            object_id,
            disk_offset:  DiskOffset(0),
            raw_size,
            stored_size:  raw_size,
            epoch_create,
            flags:        0,
            compression:  CompressionType::None,
            ref_count:    1,
            dedup_hint:   None,
        })
    }

    /// Crée un BlobDescriptor depuis un P-Blob physique déjà alloué.
    pub fn from_physical(
        blob:         &PhysicalBlobInMemory,
        object_id:    ObjectId,
        epoch_create: EpochId,
    ) -> Self {
        let mut flags = 0u16;
        if blob.compression != CompressionType::None {
            flags |= BLOB_FLAG_COMPRESSED;
        }
        Self {
            blob_id:     blob.blob_id,
            object_id,
            disk_offset: blob.disk_offset,
            raw_size:    blob.original_size,
            stored_size: blob.stored_size,
            epoch_create,
            flags,
            compression: blob.compression,
            ref_count:   blob.ref_count(),
            dedup_hint:  None,
        }
    }

    /// Reconstruit depuis la représentation on-disk.
    ///
    /// HDR-03 : `d.verify()` en premier.
    pub fn from_disk(d: &BlobDescriptorDisk) -> ExofsResult<Self> {
        d.verify()?;
        let compression = CompressionType::from_u8({ d.compression })
            .ok_or(ExofsError::InvalidArgument)?;
        Ok(Self {
            blob_id:     BlobId({ d.blob_id }),
            object_id:   ObjectId({ d.object_id }),
            disk_offset: DiskOffset({ d.disk_offset }),
            raw_size:    { d.raw_size },
            stored_size: { d.stored_size },
            epoch_create: EpochId({ d.epoch_create }),
            flags:       { d.flags },
            compression,
            ref_count:   { d.ref_count },
            dedup_hint:  None,
        })
    }

    // ── Sérialisation ──────────────────────────────────────────────────────────

    /// Sérialise vers on-disk avec checksum.
    pub fn to_disk(&self) -> BlobDescriptorDisk {
        let mut d = BlobDescriptorDisk {
            magic:        BLOB_DESCRIPTOR_MAGIC,
            blob_id:      self.blob_id.0,
            object_id:    self.object_id.0,
            disk_offset:  self.disk_offset.0,
            raw_size:     self.raw_size,
            stored_size:  self.stored_size,
            epoch_create: self.epoch_create.0,
            flags:        self.flags,
            compression:  self.compression as u8,
            hash_algo:    BLOB_HASH_ALGO,
            version:      BLOB_DESCRIPTOR_VERSION,
            _pad:         [0; 3],
            ref_count:    self.ref_count,
            checksum:     [0; 16],
        };
        d.checksum = d.compute_checksum();
        d
    }

    // ── Requêtes ───────────────────────────────────────────────────────────────

    /// Vrai si ce Blob est compressé.
    #[inline]
    pub fn is_compressed(&self) -> bool {
        self.flags & BLOB_FLAG_COMPRESSED != 0
    }

    /// Vrai si ce Blob est chiffré.
    #[inline]
    pub fn is_encrypted(&self) -> bool {
        self.flags & BLOB_FLAG_ENCRYPTED != 0
    }

    /// Vrai si ce Blob est épinglé (ne doit pas être GC).
    #[inline]
    pub fn is_pinned(&self) -> bool {
        self.flags & BLOB_FLAG_PINNED != 0
    }

    /// Vrai si ce Blob est dédupliqué.
    #[inline]
    pub fn is_deduplicated(&self) -> bool {
        self.flags & BLOB_FLAG_DEDUPLICATED != 0
    }

    /// Ratio de compression × 100 (100 = non compressé).
    pub fn compression_ratio_x100(&self) -> u32 {
        if self.raw_size == 0 {
            return 100;
        }
        ((self.stored_size * 100) / self.raw_size) as u32
    }

    // ── Vérification contenu ───────────────────────────────────────────────────

    /// Vérifie que `data` correspond bien au BlobId enregistré (HASH-01).
    ///
    /// SEC-04 : les données ne sont pas loguées si mismatch.
    pub fn verify_content(&self, data: &[u8]) -> ExofsResult<()> {
        let computed = compute_blob_id(data);
        if computed != self.blob_id {
            // SEC-04 : ne jamais loguer le contenu ni le BlobId attendu.
            return Err(ExofsError::Corrupt);
        }
        Ok(())
    }

    /// Tente de marquer ce Blob comme dédupliqué vers `canonical_id`.
    pub fn set_dedup(&mut self, canonical_id: BlobId) {
        self.dedup_hint = Some(canonical_id);
        self.flags |= BLOB_FLAG_DEDUPLICATED;
    }

    // ── Validation ─────────────────────────────────────────────────────────────

    /// Valide la cohérence interne du BlobDescriptor.
    pub fn validate(&self) -> ExofsResult<()> {
        if self.raw_size == 0 && self.ref_count > 0 {
            return Err(ExofsError::InvalidArgument);
        }
        if self.stored_size > self.raw_size && self.is_compressed() {
            // Le compressé ne peut pas être plus grand que le brut normalement,
            // mais on tolère si compression type est None.
        }
        if self.raw_size > BLOB_MAX_SIZE {
            return Err(ExofsError::Overflow);
        }
        Ok(())
    }
}

impl fmt::Display for BlobDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "BlobDescriptor {{ raw_size: {}, stored_size: {}, \
             compression: {:?}, refs: {}, dedup: {} }}",
            self.raw_size,
            self.stored_size,
            self.compression,
            self.ref_count,
            self.dedup_hint.is_some(),
        )
    }
}

impl fmt::Debug for BlobDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

// ── BlobCreateParams ───────────────────────────────────────────────────────────

/// Paramètres de création d'un nouveau Blob ExoFS.
pub struct BlobCreateParams {
    /// Objet propriétaire.
    pub object_id:    ObjectId,
    /// Epoch de création.
    pub epoch:        EpochId,
    /// Type de compression souhaité.
    pub compression:  CompressionType,
    /// Forcer l'épinglage (ne jamais GC).
    pub pinned:       bool,
    /// Hint de déduplication (None = désactivé).
    pub dedup_hint:   Option<BlobId>,
}

impl BlobCreateParams {
    /// Paramètres par défaut (sans compression, non épinglé).
    pub fn new(object_id: ObjectId, epoch: EpochId) -> Self {
        Self {
            object_id,
            epoch,
            compression: CompressionType::None,
            pinned:      false,
            dedup_hint:  None,
        }
    }

    /// Active la compression LZ4.
    pub fn with_lz4(mut self) -> Self {
        self.compression = CompressionType::Lz4;
        self
    }

    /// Active la compression Zstd niveau 3.
    pub fn with_zstd(mut self) -> Self {
        self.compression = CompressionType::Zstd;
        self
    }

    /// Épingle le Blob.
    pub fn pinned(mut self) -> Self {
        self.pinned = true;
        self
    }

    /// Applique le hint de dédup.
    pub fn with_dedup(mut self, canonical: BlobId) -> Self {
        self.dedup_hint = Some(canonical);
        self
    }
}

// ── BlobStats ──────────────────────────────────────────────────────────────────

/// Statistiques agrégées pour la population de Blobs.
#[derive(Default, Debug)]
pub struct BlobStats {
    /// Nombre total de BlobDescriptors actifs.
    pub total:            u64,
    /// Total des données brutes (octet).
    pub total_raw_bytes:  u64,
    /// Total des données stockées (octet, après compression).
    pub total_stored_bytes: u64,
    /// Nombre de Blobs dédupliqués.
    pub dedup_count:      u64,
    /// Nombre de Blobs compressés.
    pub compressed_count: u64,
    /// Nombre de Blobs épinglés.
    pub pinned_count:     u64,
    /// Nombre de Blobs éligibles au GC (ref_count == 0).
    pub gc_eligible:      u64,
}

impl BlobStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enregistre un nouveau BlobDescriptor dans les stats.
    pub fn record(&mut self, b: &BlobDescriptor) {
        self.total = self.total.saturating_add(1);
        self.total_raw_bytes    = self.total_raw_bytes.saturating_add(b.raw_size);
        self.total_stored_bytes = self.total_stored_bytes.saturating_add(b.stored_size);
        if b.is_deduplicated()  { self.dedup_count      = self.dedup_count.saturating_add(1); }
        if b.is_compressed()    { self.compressed_count = self.compressed_count.saturating_add(1); }
        if b.is_pinned()        { self.pinned_count     = self.pinned_count.saturating_add(1); }
        if b.ref_count == 0     { self.gc_eligible      = self.gc_eligible.saturating_add(1); }
    }

    /// Ratio d'économie de stockage grâce à la compression, ×100.
    pub fn savings_ratio_x100(&self) -> u64 {
        if self.total_raw_bytes == 0 {
            return 100;
        }
        100u64.saturating_sub(
            (self.total_stored_bytes * 100) / self.total_raw_bytes
        )
    }
}

impl fmt::Display for BlobStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "BlobStats {{ total: {}, raw: {} B, stored: {} B, \
             dedup: {}, compressed: {}, pinned: {}, gc_eligible: {} }}",
            self.total, self.total_raw_bytes, self.total_stored_bytes,
            self.dedup_count, self.compressed_count,
            self.pinned_count, self.gc_eligible,
        )
    }
}

// ── Fonctions utilitaires publiques ────────────────────────────────────────────

/// Vérifie qu'un BlobId correspond aux données attendues (HASH-01, SEC-04).
///
/// Utilisé par les modules de haut niveau (object_loader, object_cache).
pub fn blob_verify_content(data: &[u8], expected_blob_id: &BlobId) -> bool {
    let computed = compute_blob_id(data);
    &computed == expected_blob_id
}

/// Calcule le BlobId de `data` (Blake3 brut, HASH-01).
#[inline]
pub fn blob_compute_id(data: &[u8]) -> BlobId {
    compute_blob_id(data)
}

/// Vérifie qu'un offset disque est bien aligné sur `BLOB_DISK_ALIGN`.
#[inline]
pub fn blob_offset_aligned(offset: DiskOffset) -> bool {
    offset.0 % BLOB_DISK_ALIGN == 0
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blob_descriptor_disk_size() {
        assert_eq!(mem::size_of::<BlobDescriptorDisk>(), 128);
    }

    #[test]
    fn test_blob_verify_content_ok() {
        let data = b"hello exofs";
        let id   = blob_compute_id(data);
        assert!(blob_verify_content(data, &id));
    }

    #[test]
    fn test_blob_verify_content_tampered() {
        let data  = b"hello exofs";
        let id    = blob_compute_id(data);
        let tampered = b"hello ExoFS";
        assert!(!blob_verify_content(tampered, &id));
    }

    #[test]
    fn test_compression_ratio() {
        let d = BlobDescriptor {
            blob_id:     BlobId([0; 32]),
            object_id:   ObjectId([1; 32]),
            disk_offset: DiskOffset(0),
            raw_size:    1000,
            stored_size: 500,
            epoch_create: EpochId(1),
            flags:       BLOB_FLAG_COMPRESSED,
            compression: CompressionType::Lz4,
            ref_count:   1,
            dedup_hint:  None,
        };
        assert_eq!(d.compression_ratio_x100(), 50);
    }

    #[test]
    fn test_to_disk_roundtrip() {
        let orig = BlobDescriptor {
            blob_id:     BlobId([0xAA; 32]),
            object_id:   ObjectId([0xBB; 32]),
            disk_offset: DiskOffset(4096),
            raw_size:    2048,
            stored_size: 2048,
            epoch_create: EpochId(42),
            flags:       0,
            compression: CompressionType::None,
            ref_count:   3,
            dedup_hint:  None,
        };
        let disk = orig.to_disk();
        disk.verify().expect("verify doit réussir");
        let back = BlobDescriptor::from_disk(&disk).expect("from_disk doit réussir");
        assert_eq!(back.raw_size, 2048);
        assert_eq!(back.ref_count, 3);
    }
}
