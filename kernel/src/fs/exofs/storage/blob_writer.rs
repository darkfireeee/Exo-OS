// kernel/src/fs/exofs/storage/blob_writer.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Écriture d'un P-Blob (Physical Blob) sur disque
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Un P-Blob est la représentation physique du contenu d'un objet Blob.
// Il peut être dédupliqué (plusieurs objets logiques pointent vers le même P-Blob).
//
// RÈGLE HASH-01 : BlobId = Blake3(données RAW avant compression).
// RÈGLE DEDUP-01 : vérifier l'existence du blob AVANT d'écrire.
// RÈGLE OOM-02   : try_reserve avant allocation.

use core::mem::size_of;
use alloc::vec::Vec;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, BlobId, DiskOffset, EpochId,
    compute_blob_id, OBJECT_HEADER_MAGIC, blake3_hash,
    FORMAT_VERSION_MAJOR,
};
use crate::fs::exofs::storage::block_allocator::BlockAllocator;
use crate::fs::exofs::core::stats::EXOFS_STATS;

// ─────────────────────────────────────────────────────────────────────────────
// BlobHeader — 64 octets, en-tête on-disk d'un P-Blob
// ─────────────────────────────────────────────────────────────────────────────

/// En-tête d'un P-Blob sur disque.
#[derive(Copy, Clone)]
#[repr(C, packed)]
pub struct BlobHeader {
    /// Magic "BLOB" : 0x424C4F42.
    pub magic:        u32,
    /// Version format.
    pub version:      u16,
    /// Flags.
    pub flags:        u16,
    /// BlobId (32 octets) — Blake3 des données RAW.
    pub blob_id:      [u8; 32],
    /// Taille des données payload.
    pub payload_size: u64,
    /// Nombre de références actuelles (plain u32 on-disk).
    pub ref_count:    u32,
    /// _pad.
    pub _pad:         [u8; 4],
    /// Checksum.
    pub checksum:     [u8; 32],
}

/// Magic BLOB.
const BLOB_MAGIC: u32 = 0x424C_4F42;

const _: () = assert!(
    size_of::<BlobHeader>() == 96,
    "BlobHeader doit être exactement 96 octets"
);

impl BlobHeader {
    pub fn new(blob_id: BlobId, payload_size: u64, ref_count: u32, flags: u16) -> Self {
        let mut hdr = Self {
            magic: BLOB_MAGIC,
            version: FORMAT_VERSION_MAJOR,
            flags,
            blob_id: blob_id.0,
            payload_size,
            ref_count,
            _pad: [0u8; 4],
            checksum: [0u8; 32],
        };
        // Checksum sur les 64 premiers octets.
        let ptr = &hdr as *const Self as *const u8;
        // SAFETY: BlobHeader est #[repr(C, packed)].
        let body = unsafe { core::slice::from_raw_parts(ptr, 64) };
        hdr.checksum = blake3_hash(body);
        hdr
    }

    pub fn verify(&self) -> ExofsResult<()> {
        let magic = { self.magic };
        if magic != BLOB_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        let ptr = self as *const Self as *const u8;
        // SAFETY: BlobHeader est #[repr(C, packed)].
        let body = unsafe { core::slice::from_raw_parts(ptr, 64) };
        let expected = blake3_hash(body);
        let stored = self.checksum;
        let mut acc: u8 = 0;
        for i in 0..32 { acc |= expected[i] ^ stored[i]; }
        if acc != 0 { Err(ExofsError::ChecksumMismatch) } else { Ok(()) }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Résultat d'écriture de blob
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une écriture de P-Blob.
#[derive(Debug)]
pub struct BlobWriteResult {
    pub disk_offset:   DiskOffset,
    pub blob_id:       BlobId,
    pub bytes_written: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Écriture d'un P-Blob
// ─────────────────────────────────────────────────────────────────────────────

/// Écrit un P-Blob sur disque.
///
/// RÈGLE HASH-01 : BlobId = compute_blob_id(raw_data).
/// RÈGLE OOM-02   : try_reserve avant allocation du buffer.
pub fn write_blob(
    allocator:  &BlockAllocator,
    raw_data:   &[u8],
    ref_count:  u32,
    flags:      u16,
    write_fn:   &dyn Fn(&[u8], DiskOffset) -> ExofsResult<usize>,
) -> ExofsResult<BlobWriteResult> {
    // RÈGLE HASH-01 : BlobId sur données RAW avant compression.
    let blob_id = compute_blob_id(raw_data);

    let payload_size = raw_data.len() as u64;
    let header_size  = size_of::<BlobHeader>();
    let total_size   = (header_size as u64)
        .checked_add(payload_size)
        .ok_or(ExofsError::OffsetOverflow)?;

    // Allocation.
    let extent = allocator.alloc_extent(total_size)?;

    // Sérialisation.
    let header = BlobHeader::new(blob_id, payload_size, ref_count, flags);
    let total_write = header_size
        .checked_add(raw_data.len())
        .ok_or(ExofsError::OffsetOverflow)?;

    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(total_write).map_err(|_| ExofsError::NoMemory)?;
    buf.resize(total_write, 0u8);

    // SAFETY: BlobHeader est #[repr(C, packed)].
    let hdr_bytes = unsafe {
        core::slice::from_raw_parts(&header as *const BlobHeader as *const u8, header_size)
    };
    buf[..header_size].copy_from_slice(hdr_bytes);
    buf[header_size..].copy_from_slice(raw_data);

    let written = write_fn(&buf, extent.offset)?;
    if written != total_write {
        return Err(ExofsError::PartialWrite);
    }

    EXOFS_STATS.inc_blobs_created();
    EXOFS_STATS.add_io_write(total_write as u64);

    Ok(BlobWriteResult {
        disk_offset:   extent.offset,
        blob_id,
        bytes_written: total_write as u64,
    })
}
