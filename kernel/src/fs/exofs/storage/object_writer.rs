// kernel/src/fs/exofs/storage/object_writer.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Écriture d'un objet logique sur disque
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Un objet est écrit avec un ObjectHeader précédant les données :
//   [ObjectHeader (128B)] [données payload]
//
// RÈGLE HASH-01 : BlobId calculé sur les données RAW avant compression.
// RÈGLE ONDISK-01 : ObjectHeader types plain uniquement.
// RÈGLE WRITE-01  : vérification bytes_written == expected.

use core::mem::size_of;
use alloc::vec::Vec;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, ObjectId, BlobId, DiskOffset, EpochId,
    OBJECT_HEADER_MAGIC, blake3_hash, compute_blob_id,
};
use crate::fs::exofs::core::flags::ObjectFlags;
use crate::fs::exofs::core::stats::EXOFS_STATS;
use crate::fs::exofs::storage::block_allocator::BlockAllocator;

// ─────────────────────────────────────────────────────────────────────────────
// ObjectHeader — 128 octets, en-tête on-disk de tout objet
// ─────────────────────────────────────────────────────────────────────────────

/// En-tête physique d'un objet sur disque.
///
/// RÈGLE ONDISK-01 : types plain uniquement, pas d'AtomicU64.
#[derive(Copy, Clone)]
#[repr(C, packed)]
pub struct ObjectHeader {
    /// Magic : 0x4F424A45 ("OBJE").
    pub magic:        u32,
    /// Version format.
    pub version:      u16,
    /// Flags (ObjectFlags bits).
    pub flags:        u16,
    /// ObjectId (32 octets).
    pub object_id:    [u8; 32],
    /// BlobId (32 octets) — Blake3 des données RAW avant compression.
    pub blob_id:      [u8; 32],
    /// EpochId de création de cet objet.
    pub epoch_id:     u64,
    /// Taille des données payload en octets.
    pub payload_size: u64,
    /// _pad pour atteindre 128 octets.
    pub _pad:         [u8; 8],
    /// Checksum Blake3 des 96 premiers octets de l'en-tête.
    pub checksum:     [u8; 32],
}

const _: () = assert!(
    size_of::<ObjectHeader>() == 128,
    "ObjectHeader doit être exactement 128 octets"
);

impl ObjectHeader {
    /// Crée un ObjectHeader valide.
    pub fn new(
        object_id:    ObjectId,
        blob_id:      BlobId,
        flags:        ObjectFlags,
        epoch_id:     EpochId,
        payload_size: u64,
    ) -> Self {
        let mut hdr = Self {
            magic:        OBJECT_HEADER_MAGIC,
            version:      crate::fs::exofs::core::FORMAT_VERSION_MAJOR,
            flags:        flags.0,
            object_id:    object_id.0,
            blob_id:      blob_id.0,
            epoch_id:     epoch_id.0,
            payload_size,
            _pad:         [0u8; 8],
            checksum:     [0u8; 32],
        };
        // Calcul du checksum sur les 96 premiers octets.
        let ptr = &hdr as *const Self as *const u8;
        // SAFETY: ObjectHeader est #[repr(C, packed)], taille 128.
        let body = unsafe { core::slice::from_raw_parts(ptr, 96) };
        hdr.checksum = blake3_hash(body);
        hdr
    }

    /// Vérifie le magic EN PREMIER, puis le checksum.
    pub fn verify(&self) -> ExofsResult<()> {
        let magic = { self.magic };
        if magic != OBJECT_HEADER_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        let ptr = self as *const Self as *const u8;
        // SAFETY: ObjectHeader est #[repr(C, packed)].
        let body = unsafe { core::slice::from_raw_parts(ptr, 96) };
        let expected = blake3_hash(body);
        let stored = self.checksum;
        let mut acc: u8 = 0;
        for i in 0..32 {
            acc |= expected[i] ^ stored[i];
        }
        if acc != 0 {
            Err(ExofsError::ChecksumMismatch)
        } else {
            Ok(())
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ObjectWriteRequest — paramètres d'une écriture d'objet
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une écriture d'objet.
#[derive(Debug)]
pub struct ObjectWriteResult {
    /// Offset disque où l'objet a été écrit.
    pub disk_offset:  DiskOffset,
    /// BlobId calculé sur les données RAW.
    pub blob_id:      BlobId,
    /// Nombre total d'octets écrits (header + données).
    pub bytes_written: u64,
}

/// Écrit un objet sur disque via l'allocateur et la fonction d'écriture injectée.
///
/// # Protocole
/// 1. Calculer le BlobId sur `raw_data` AVANT compression (RÈGLE HASH-01).
/// 2. Allouer un Extent de taille (128 + payload).
/// 3. Sérialiser ObjectHeader + données dans le buffer.
/// 4. Écrire le buffer via write_fn.
/// 5. Vérifier bytes_written == expected (RÈGLE WRITE-01).
///
/// RÈGLE OOM-02 : try_reserve avant resize.
pub fn write_object(
    allocator:    &BlockAllocator,
    object_id:    ObjectId,
    flags:        ObjectFlags,
    epoch_id:     EpochId,
    raw_data:     &[u8],
    write_fn:     &dyn Fn(&[u8], DiskOffset) -> ExofsResult<usize>,
) -> ExofsResult<ObjectWriteResult> {
    // RÈGLE HASH-01 : BlobId calculé sur raw_data AVANT compression.
    let blob_id = compute_blob_id(raw_data);

    let payload_size = raw_data.len() as u64;
    let total_size = (size_of::<ObjectHeader>() as u64)
        .checked_add(payload_size)
        .ok_or(ExofsError::OffsetOverflow)?;

    // Allocation d'un bloc contigu.
    let extent = allocator.alloc_extent(total_size)?;

    // Sérialisation.
    let header = ObjectHeader::new(object_id, blob_id, flags, epoch_id, payload_size);
    let header_size = size_of::<ObjectHeader>();
    let total_write = header_size
        .checked_add(raw_data.len())
        .ok_or(ExofsError::OffsetOverflow)?;

    // Buffer d'écriture (RÈGLE OOM-02 : try_reserve).
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(total_write).map_err(|_| ExofsError::NoMemory)?;
    buf.resize(total_write, 0u8);

    // Copie de l'en-tête.
    // SAFETY: header est #[repr(C, packed)], taille 128 B.
    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(&header as *const ObjectHeader as *const u8, header_size)
    };
    buf[..header_size].copy_from_slice(hdr_bytes);
    buf[header_size..].copy_from_slice(raw_data);

    // Écriture physique.
    let written = write_fn(&buf, extent.offset)?;
    // RÈGLE WRITE-01 : vérification.
    if written != total_write {
        return Err(ExofsError::PartialWrite);
    }

    // Statistiques.
    EXOFS_STATS.inc_objects_written();
    EXOFS_STATS.add_io_write(total_write as u64);

    Ok(ObjectWriteResult {
        disk_offset:   extent.offset,
        blob_id,
        bytes_written: total_write as u64,
    })
}
