//! object_writer.rs — Écriture d'objets ExoFS
//!
//! Un objet ExoFS est une entité nommée (ObjectId) composée d'un ou plusieurs blobs.
//! L'ObjectHeader (128 octets) précède le contenu sur disque.
//!
//! Pipeline d'écriture :
//!   données → découpage en blobs → écriture blobs → écriture ObjectHeader → écriture ExtentMap
//!
//! Règles spec :
//!   WRITE-02 : vérification bytes_written == expected après chaque write
//!   HDR-03   : ObjectHeader avec magic + checksum AVANT payload
//!   ONDISK-03: aucun AtomicXxx dans les structs #[repr(C)]
//!   ARITH-02 : checked_add / checked_mul systématiques
//!   OOM-02   : try_reserve avant tout Vec::push

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, BlobId, ObjectId, DiskOffset, EpochId,
};
use crate::fs::exofs::core::blob_id::{compute_blob_id, blake3_hash};
use crate::fs::exofs::storage::storage_stats::STORAGE_STATS;
use crate::fs::exofs::storage::layout::{BLOCK_SIZE, align_up};
use crate::fs::exofs::storage::blob_writer::{BlobWriter, BlobWriterConfig, BlobWriteResult};
use crate::fs::exofs::storage::compression_choice::{CompressionType, ContentHint};
use crate::fs::exofs::storage::extent_writer::{Extent, ExtentWriter};

// ─────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────

/// Magic de l'en-tête objet : "EXOB"
pub const OBJECT_HEADER_MAGIC: u32 = 0x4558_4F42;

/// Taille fixe de l'en-tête objet (128 octets — ONDISK-03)
pub const OBJECT_HEADER_SIZE: usize = 128;

/// Version du format objet
pub const OBJECT_FORMAT_VERSION: u8 = 1;

/// Taille maximale d'un objet inline (données stockées directement dans l'objet)
pub const OBJECT_INLINE_MAX: usize = 4096 - OBJECT_HEADER_SIZE;

/// Taille de découpage en blobs (512 KiB)
pub const OBJECT_BLOB_CHUNK: usize = 512 * 1024;

/// Taille maximale d'un objet (4 GiB)  
pub const OBJECT_MAX_SIZE: usize = 4 * 1024 * 1024 * 1024;

/// Nombre maximal de blobs par objet
pub const OBJECT_MAX_BLOBS: usize = 65536;

// ─────────────────────────────────────────────────────────────
// Structures disque (repr C — ONDISK-03)
// ─────────────────────────────────────────────────────────────

/// Type d'objet
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectType {
    Regular     = 0,
    Directory   = 1,
    Symlink     = 2,
    Device      = 3,
    Metadata    = 4,
}

/// En-tête objet sur disque (OBJECT_HEADER_SIZE = 128 octets)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ObjectHeaderDisk {
    /// Magic "EXOB"
    pub magic: u32,
    /// Version format
    pub version: u8,
    /// Type d'objet (ObjectType)
    pub object_type: u8,
    /// Flags (bit0=inline, bit1=multi-blob, bit2=encrypted)
    pub flags: u8,
    /// Réservé
    pub _reserved0: u8,
    /// Nombre de blobs
    pub blob_count: u32,
    /// Taille totale du contenu (logique, avant fragmentation)
    pub content_size: u64,
    /// ObjectId
    pub object_id: [u8; 32],
    /// Époque de création
    pub epoch: u64,
    /// Offset de l'ExtentMap sur disque (0 si inline)
    pub extent_map_offset: u64,
    /// Checksum Blake3 du contenu complet (blake3 sur données brutes)
    pub content_hash: [u8; 32],
    /// Checksum de cet en-tête (4 premiers octets de blake3 sur les 124 premiers bytes)
    pub header_checksum: [u8; 4],
}

const _: () = assert!(
    core::mem::size_of::<ObjectHeaderDisk>() == OBJECT_HEADER_SIZE,
    "ObjectHeaderDisk doit faire exactement 128 octets"
);

impl ObjectHeaderDisk {
    /// Calcule le checksum de l'en-tête (sur les 124 premiers octets)
    pub fn compute_checksum(raw124: &[u8; 124]) -> [u8; 4] {
        let h = blake3_hash(raw124);
        [h[0], h[1], h[2], h[3]]
    }

    /// Vérifie le checksum de cet en-tête (HDR-03)
    pub fn verify_checksum(&self) -> bool {
        let raw: [u8; OBJECT_HEADER_SIZE] = unsafe { core::mem::transmute(*self) };
        let mut raw124 = [0u8; 124];
        raw124.copy_from_slice(&raw[..124]);
        let expected = Self::compute_checksum(&raw124);
        self.header_checksum == expected
    }

    /// Retourne les octets bruts
    pub fn as_bytes(&self) -> [u8; OBJECT_HEADER_SIZE] {
        unsafe { core::mem::transmute(*self) }
    }
}

// ─────────────────────────────────────────────────────────────
// Référence à un blob dans un objet
// ─────────────────────────────────────────────────────────────

/// Référence à un blob (stockée dans l'ExtentMap ou inline)
#[derive(Debug, Clone)]
pub struct BlobRef {
    /// Identifiant du blob
    pub blob_id: BlobId,
    /// Offset disque du blob
    pub offset: DiskOffset,
    /// Taille originale du blob
    pub size: u32,
    /// Index du chunk dans l'objet
    pub chunk_index: u32,
}

// ─────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────

/// Configuration pour écriture d'un objet
#[derive(Clone)]
pub struct ObjectWriterConfig {
    pub object_type: ObjectType,
    pub epoch: EpochId,
    pub compression_hint: ContentHint,
    pub forced_algo: Option<CompressionType>,
    pub dedup_enabled: bool,
    pub blob_chunk_size: usize,
}

impl Default for ObjectWriterConfig {
    fn default() -> Self {
        Self {
            object_type: ObjectType::Regular,
            epoch: EpochId(0),
            compression_hint: ContentHint::Unknown,
            forced_algo: None,
            dedup_enabled: true,
            blob_chunk_size: OBJECT_BLOB_CHUNK,
        }
    }
}

impl ObjectWriterConfig {
    pub fn new(epoch: EpochId) -> Self {
        Self { epoch, ..Default::default() }
    }

    pub fn with_type(mut self, t: ObjectType) -> Self {
        self.object_type = t;
        self
    }

    pub fn with_hint(mut self, h: ContentHint) -> Self {
        self.compression_hint = h;
        self
    }

    pub fn no_dedup(mut self) -> Self {
        self.dedup_enabled = false;
        self
    }

    pub fn with_chunk_size(mut self, sz: usize) -> Self {
        self.blob_chunk_size = sz.max(BLOCK_SIZE).min(OBJECT_MAX_SIZE);
        self
    }
}

// ─────────────────────────────────────────────────────────────
// Résultat d'écriture
// ─────────────────────────────────────────────────────────────

/// Résultat de l'écriture d'un objet
#[derive(Debug, Clone)]
pub struct ObjectWriteResult {
    pub object_id: ObjectId,
    pub header_offset: DiskOffset,
    pub content_size: u64,
    pub disk_size: u64,
    pub blob_count: u32,
    pub epoch: EpochId,
    pub blobs: Vec<BlobRef>,
    pub content_hash: [u8; 32],
}

impl ObjectWriteResult {
    /// Octets économisés par la compression/dédup
    pub fn savings_bytes(&self, original_stored: u64) -> u64 {
        self.content_size.saturating_sub(original_stored)
    }
}

// ─────────────────────────────────────────────────────────────
// Statistiques globales
// ─────────────────────────────────────────────────────────────

pub struct ObjectWriterStats {
    pub total_objects: AtomicU64,
    pub total_bytes_written: AtomicU64,
    pub total_blobs_created: AtomicU64,
    pub multi_blob_objects: AtomicU64,
    pub write_errors: AtomicU64,
    pub header_writes: AtomicU64,
}

pub static OBJECT_WRITER_STATS: ObjectWriterStats = ObjectWriterStats {
    total_objects: AtomicU64::new(0),
    total_bytes_written: AtomicU64::new(0),
    total_blobs_created: AtomicU64::new(0),
    multi_blob_objects: AtomicU64::new(0),
    write_errors: AtomicU64::new(0),
    header_writes: AtomicU64::new(0),
};

impl ObjectWriterStats {
    pub fn snapshot(&self) -> ObjectWriterStatsSnapshot {
        ObjectWriterStatsSnapshot {
            total_objects: self.total_objects.load(Ordering::Relaxed),
            total_bytes_written: self.total_bytes_written.load(Ordering::Relaxed),
            total_blobs_created: self.total_blobs_created.load(Ordering::Relaxed),
            multi_blob_objects: self.multi_blob_objects.load(Ordering::Relaxed),
            write_errors: self.write_errors.load(Ordering::Relaxed),
            header_writes: self.header_writes.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ObjectWriterStatsSnapshot {
    pub total_objects: u64,
    pub total_bytes_written: u64,
    pub total_blobs_created: u64,
    pub multi_blob_objects: u64,
    pub write_errors: u64,
    pub header_writes: u64,
}

// ─────────────────────────────────────────────────────────────
// ObjectWriter principal
// ─────────────────────────────────────────────────────────────

/// Writer d'objets ExoFS
pub struct ObjectWriter;

impl ObjectWriter {
    /// Écrit un objet complet sur disque.
    ///
    /// Pipeline :
    /// 1. Validation
    /// 2. Calcul du content_hash (Blake3 sur toutes les données)
    /// 3. Découpage en chunks → écriture de blobs
    /// 4. Écriture de l'ObjectHeader (HDR-03 + WRITE-02)
    ///
    /// # Paramètres
    /// - `object_id`  : identifiant de l'objet
    /// - `data`       : contenu complet
    /// - `config`     : configuration d'écriture
    /// - `alloc_fn`   : allocation disque (n_blocks) → DiskOffset
    /// - `write_fn`   : écriture physique (offset, données) → bytes_written
    /// - `dedup_check`: déduplication blob (BlobId) → Option<DiskOffset>
    pub fn write_object<AllocFn, WriteFn, DedupFn>(
        object_id: ObjectId,
        data: &[u8],
        config: &ObjectWriterConfig,
        alloc_fn: AllocFn,
        write_fn: WriteFn,
        dedup_check: DedupFn,
    ) -> ExofsResult<ObjectWriteResult>
    where
        AllocFn: FnMut(u64) -> ExofsResult<DiskOffset>,
        WriteFn: FnMut(DiskOffset, &[u8]) -> ExofsResult<usize>,
        DedupFn: Fn(&BlobId) -> Option<DiskOffset>,
    {
        // ── 1. Validation ─────────────────────────────────────────────
        if data.len() > OBJECT_MAX_SIZE {
            return Err(ExofsError::InvalidSize);
        }

        // ── 2. Content hash (Blake3 sur données complètes) ────────────
        let content_hash = blake3_hash(data);

        // ── 3. Découpage en blobs ─────────────────────────────────────
        let (blobs, disk_size) = Self::write_blobs(
            data, config, alloc_fn, write_fn, dedup_check,
        )?;

        let blob_count = blobs.len() as u32;
        let content_size = data.len() as u64;

        // ── 4. Construction de l'ObjectWriteResult ────────────────────
        OBJECT_WRITER_STATS.total_objects.fetch_add(1, Ordering::Relaxed);
        OBJECT_WRITER_STATS.total_bytes_written.fetch_add(disk_size, Ordering::Relaxed);
        OBJECT_WRITER_STATS.total_blobs_created.fetch_add(blob_count as u64, Ordering::Relaxed);
        if blob_count > 1 {
            OBJECT_WRITER_STATS.multi_blob_objects.fetch_add(1, Ordering::Relaxed);
        }
        STORAGE_STATS.add_write(disk_size);

        // L'ObjectHeader sera écrit séparément via write_header()
        let first_offset = blobs.first().map(|b| b.offset).unwrap_or(DiskOffset(0));

        Ok(ObjectWriteResult {
            object_id,
            header_offset: first_offset,
            content_size,
            disk_size,
            blob_count,
            epoch: config.epoch,
            blobs,
            content_hash,
        })
    }

    /// Écrit l'ObjectHeader sur disque (séparé pour permettre l'écriture des blobs d'abord)
    ///
    /// HDR-03 : magic + checksum AVANT le payload
    /// WRITE-02 : vérification bytes_written
    pub fn write_header<AllocFn, WriteFn>(
        result: &ObjectWriteResult,
        config: &ObjectWriterConfig,
        alloc_fn: AllocFn,
        mut write_fn: WriteFn,
    ) -> ExofsResult<DiskOffset>
    where
        AllocFn: FnOnce(u64) -> ExofsResult<DiskOffset>,
        WriteFn: FnMut(DiskOffset, &[u8]) -> ExofsResult<usize>,
    {
        let n_blocks = BlobWriter::size_to_blocks(OBJECT_HEADER_SIZE as u64);
        let offset = alloc_fn(n_blocks)?;

        let mut flags: u8 = 0;
        if result.blob_count == 1 { flags |= 0b0000_0001; } // inline
        if result.blob_count > 1 { flags |= 0b0000_0010; }  // multi-blob

        let extent_map_off = if result.blob_count > 1 { result.header_offset.0 } else { 0 };

        let mut hdr = ObjectHeaderDisk {
            magic: OBJECT_HEADER_MAGIC,
            version: OBJECT_FORMAT_VERSION,
            object_type: config.object_type as u8,
            flags,
            _reserved0: 0,
            blob_count: result.blob_count,
            content_size: result.content_size,
            object_id: result.object_id.0,
            epoch: config.epoch.0,
            extent_map_offset: extent_map_off,
            content_hash: result.content_hash,
            header_checksum: [0u8; 4],
        };

        // Calcul du checksum (HDR-03)
        let raw: [u8; OBJECT_HEADER_SIZE] = hdr.as_bytes();
        let mut raw124 = [0u8; 124];
        raw124.copy_from_slice(&raw[..124]);
        hdr.header_checksum = ObjectHeaderDisk::compute_checksum(&raw124);

        let hdr_bytes = hdr.as_bytes();

        // Écriture (WRITE-02)
        let written = write_fn(offset, &hdr_bytes)?;
        if written != OBJECT_HEADER_SIZE {
            OBJECT_WRITER_STATS.write_errors.fetch_add(1, Ordering::Relaxed);
            STORAGE_STATS.inc_io_error();
            return Err(ExofsError::ShortWrite);
        }

        OBJECT_WRITER_STATS.header_writes.fetch_add(1, Ordering::Relaxed);
        Ok(offset)
    }

    /// Écrit l'ExtentMap d'un objet multi-blob sur disque
    pub fn write_extent_map<AllocFn, WriteFn>(
        blobs: &[BlobRef],
        alloc_fn: AllocFn,
        mut write_fn: WriteFn,
    ) -> ExofsResult<DiskOffset>
    where
        AllocFn: FnOnce(u64) -> ExofsResult<DiskOffset>,
        WriteFn: FnMut(DiskOffset, &[u8]) -> ExofsResult<usize>,
    {
        if blobs.is_empty() {
            return Err(ExofsError::InvalidArgument);
        }

        // Format : [count: u32 LE][blob_ref: 44B each]
        // blob_ref = [blob_id: 32B][offset: 8B][size: 4B]
        const BLOB_REF_SIZE: usize = 44;
        let count = blobs.len();
        let map_size = 4usize.checked_add(
            count.checked_mul(BLOB_REF_SIZE).ok_or(ExofsError::Overflow)?
        ).ok_or(ExofsError::Overflow)?;

        let n_blocks = BlobWriter::size_to_blocks(map_size as u64);
        let offset = alloc_fn(n_blocks)?;

        let mut buf = Vec::new();
        buf.try_reserve(align_up(map_size as u64, BLOCK_SIZE as u64) as usize)
            .map_err(|_| ExofsError::NoMemory)?;

        // count
        buf.extend_from_slice(&(count as u32).to_le_bytes());

        for bref in blobs {
            buf.extend_from_slice(&bref.blob_id.0);
            buf.extend_from_slice(&bref.offset.0.to_le_bytes());
            buf.extend_from_slice(&bref.size.to_le_bytes());
        }

        // Pad à la taille alignée
        let aligned = align_up(buf.len() as u64, BLOCK_SIZE as u64) as usize;
        buf.resize(aligned, 0u8);

        // Écriture (WRITE-02)
        let written = write_fn(offset, &buf)?;
        if written != buf.len() {
            STORAGE_STATS.inc_io_error();
            return Err(ExofsError::ShortWrite);
        }

        Ok(offset)
    }

    // ── Découpage et écriture des blobs ─────────────────────────────

    fn write_blobs<AllocFn, WriteFn, DedupFn>(
        data: &[u8],
        config: &ObjectWriterConfig,
        mut alloc_fn: AllocFn,
        mut write_fn: WriteFn,
        dedup_check: DedupFn,
    ) -> ExofsResult<(Vec<BlobRef>, u64)>
    where
        AllocFn: FnMut(u64) -> ExofsResult<DiskOffset>,
        WriteFn: FnMut(DiskOffset, &[u8]) -> ExofsResult<usize>,
        DedupFn: Fn(&BlobId) -> Option<DiskOffset>,
    {
        let chunk_size = config.blob_chunk_size;
        let n_chunks = (data.len() + chunk_size - 1) / chunk_size;
        if n_chunks > OBJECT_MAX_BLOBS {
            return Err(ExofsError::InvalidSize);
        }

        let mut refs = Vec::new();
        refs.try_reserve(n_chunks).map_err(|_| ExofsError::NoMemory)?;

        let mut total_disk = 0u64;
        let mut chunk_idx = 0u32;

        let blob_cfg = BlobWriterConfig {
            forced_algo: config.forced_algo,
            hint: config.compression_hint,
            dedup_enabled: config.dedup_enabled,
            verify_after_write: false,
            epoch: config.epoch,
        };

        let mut pos = 0;
        while pos < data.len() {
            let end = (pos + chunk_size).min(data.len());
            let chunk = &data[pos..end];

            let blob_id = compute_blob_id(chunk);
            let dedup_ref = dedup_check(&blob_id);

            let res = BlobWriter::write_blob(
                chunk,
                &blob_cfg,
                |n| alloc_fn(n),
                |off, buf| write_fn(off, buf),
                |_id| dedup_ref,
            )?;

            total_disk = total_disk
                .checked_add(res.disk_size)
                .ok_or(ExofsError::Overflow)?;

            refs.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            refs.push(BlobRef {
                blob_id: res.blob_id,
                offset: res.offset,
                size: chunk.len() as u32,
                chunk_index: chunk_idx,
            });

            chunk_idx = chunk_idx.saturating_add(1);
            pos = end;
        }

        Ok((refs, total_disk))
    }
}

// ─────────────────────────────────────────────────────────────
// UpdateObjectWriter — mise à jour partielle
// ─────────────────────────────────────────────────────────────

/// Décrit une modification partielle d'un objet
#[derive(Debug)]
pub struct ObjectPatch {
    /// Offset dans l'objet (logique, en octets)
    pub logical_offset: u64,
    /// Nouvelles données
    pub data: Vec<u8>,
}

/// Résultat d'une mise à jour partielle
#[derive(Debug)]
pub struct ObjectPatchResult {
    pub new_content_hash: [u8; 32],
    pub blobs_rewritten: u32,
    pub bytes_changed: u64,
}

/// Applique des patches sur les chunks d'un objet existant
pub struct UpdateObjectWriter;

impl UpdateObjectWriter {
    /// Réécrit les chunks affectés par un patch
    pub fn apply_patch<RewriteFn>(
        patch: &ObjectPatch,
        old_blobs: &[BlobRef],
        chunk_size: usize,
        full_data: &mut Vec<u8>,
        rewrite_fn: RewriteFn,
    ) -> ExofsResult<ObjectPatchResult>
    where
        RewriteFn: Fn(u32, &[u8]) -> ExofsResult<BlobWriteResult>,
    {
        if patch.logical_offset.checked_add(patch.data.len() as u64).is_none() {
            return Err(ExofsError::Overflow);
        }

        let patch_end = (patch.logical_offset as usize)
            .checked_add(patch.data.len())
            .ok_or(ExofsError::Overflow)?;

        if patch_end > full_data.len() {
            return Err(ExofsError::InvalidArgument);
        }

        // Appliquer le patch en mémoire
        full_data[patch.logical_offset as usize..patch_end]
            .copy_from_slice(&patch.data);

        // Identifier les chunks affectés
        let first_chunk = patch.logical_offset as usize / chunk_size;
        let last_chunk = (patch_end - 1) / chunk_size;

        let mut rewritten = 0u32;

        for chunk_idx in first_chunk..=last_chunk {
            let start = chunk_idx * chunk_size;
            let end = ((chunk_idx + 1) * chunk_size).min(full_data.len());
            let chunk_data = &full_data[start..end];

            let _ = rewrite_fn(chunk_idx as u32, chunk_data)?;
            rewritten = rewritten.saturating_add(1);
        }

        let new_hash = blake3_hash(full_data);

        Ok(ObjectPatchResult {
            new_content_hash: new_hash,
            blobs_rewritten: rewritten,
            bytes_changed: patch.data.len() as u64,
        })
    }
}

// ─────────────────────────────────────────────────────────────
// Vérification pré-écriture
// ─────────────────────────────────────────────────────────────

/// Valide la configuration avant d'écrire un objet
pub fn validate_object_write(
    data_len: usize,
    config: &ObjectWriterConfig,
) -> ExofsResult<()> {
    if data_len == 0 {
        return Err(ExofsError::InvalidArgument);
    }
    if data_len > OBJECT_MAX_SIZE {
        return Err(ExofsError::InvalidSize);
    }
    if config.blob_chunk_size == 0 || config.blob_chunk_size > OBJECT_MAX_SIZE {
        return Err(ExofsError::InvalidArgument);
    }
    let n_chunks = (data_len + config.blob_chunk_size - 1) / config.blob_chunk_size;
    if n_chunks > OBJECT_MAX_BLOBS {
        return Err(ExofsError::InvalidArgument);
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────
// Tests unitaires
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn make_object_id(n: u8) -> ObjectId {
        ObjectId([n; 32])
    }

    fn make_disk(size: usize) -> Vec<u8> {
        vec![0u8; size]
    }

    #[test]
    fn object_header_size_constant() {
        assert_eq!(core::mem::size_of::<ObjectHeaderDisk>(), OBJECT_HEADER_SIZE);
    }

    #[test]
    fn object_header_checksum_roundtrip() {
        let mut hdr = ObjectHeaderDisk {
            magic: OBJECT_HEADER_MAGIC,
            version: OBJECT_FORMAT_VERSION,
            object_type: ObjectType::Regular as u8,
            flags: 0,
            _reserved0: 0,
            blob_count: 1,
            content_size: 512,
            object_id: [0xAB; 32],
            epoch: 42,
            extent_map_offset: 0,
            content_hash: [0u8; 32],
            header_checksum: [0u8; 4],
        };
        let raw = hdr.as_bytes();
        let mut raw124 = [0u8; 124];
        raw124.copy_from_slice(&raw[..124]);
        hdr.header_checksum = ObjectHeaderDisk::compute_checksum(&raw124);
        assert!(hdr.verify_checksum());
    }

    #[test]
    fn write_object_single_chunk() {
        let oid = make_object_id(0x01);
        let data = vec![0xDE_u8; 256];
        let config = ObjectWriterConfig::new(EpochId(1))
            .no_dedup()
            .with_chunk_size(512 * 1024);

        validate_object_write(data.len(), &config).unwrap();

        let mut disk = make_disk(65536);
        let mut alloc_off = 0u64;

        let result = ObjectWriter::write_object(
            oid,
            &data,
            &config,
            |n| { let o = DiskOffset(alloc_off); alloc_off += n * BLOCK_SIZE as u64; Ok(o) },
            |off, buf| {
                let s = off.0 as usize;
                if s + buf.len() <= disk.len() { disk[s..s+buf.len()].copy_from_slice(buf); }
                Ok(buf.len())
            },
            |_| None,
        ).unwrap();

        assert_eq!(result.blob_count, 1);
        assert_eq!(result.content_size, data.len() as u64);
        assert!(!result.blobs.is_empty());
    }

    #[test]
    fn write_object_multi_chunk() {
        let oid = make_object_id(0x02);
        let data = vec![0xAB_u8; OBJECT_BLOB_CHUNK * 3 + 100];
        let config = ObjectWriterConfig::new(EpochId(2))
            .no_dedup()
            .with_chunk_size(OBJECT_BLOB_CHUNK);

        let mut disk = make_disk(8 * 1024 * 1024);
        let mut off = 0u64;

        let result = ObjectWriter::write_object(
            oid, &data, &config,
            |n| { let o = DiskOffset(off); off += n * BLOCK_SIZE as u64; Ok(o) },
            |o, buf| {
                let s = o.0 as usize;
                if s + buf.len() <= disk.len() { disk[s..s+buf.len()].copy_from_slice(buf); }
                Ok(buf.len())
            },
            |_| None,
        ).unwrap();

        assert_eq!(result.blob_count, 4);
        OBJECT_WRITER_STATS.multi_blob_objects.load(Ordering::Relaxed);
    }

    #[test]
    fn write_header_checksum_valid() {
        let oid = make_object_id(0x03);
        let data = vec![0x55_u8; 512];
        let config = ObjectWriterConfig::new(EpochId(3)).no_dedup();
        let mut disk = make_disk(65536);
        let mut off = 0u64;

        let result = ObjectWriter::write_object(
            oid, &data, &config,
            |n| { let o = DiskOffset(off); off += n * BLOCK_SIZE as u64; Ok(o) },
            |o, buf| {
                let s = o.0 as usize;
                if s + buf.len() <= disk.len() { disk[s..s+buf.len()].copy_from_slice(buf); }
                Ok(buf.len())
            },
            |_| None,
        ).unwrap();

        let hdr_offset = ObjectWriter::write_header(
            &result, &config,
            |n| { let o = DiskOffset(off); off += n * BLOCK_SIZE as u64; Ok(o) },
            |o, buf| {
                let s = o.0 as usize;
                if s + buf.len() <= disk.len() { disk[s..s+buf.len()].copy_from_slice(buf); }
                Ok(buf.len())
            },
        ).unwrap();

        // Vérification en-tête sur disque
        let raw = &disk[hdr_offset.0 as usize..hdr_offset.0 as usize + OBJECT_HEADER_SIZE];
        let hdr: &ObjectHeaderDisk = unsafe { &*(raw.as_ptr() as *const ObjectHeaderDisk) };
        assert_eq!(hdr.magic, OBJECT_HEADER_MAGIC);
        assert!(hdr.verify_checksum());
    }

    #[test]
    fn extent_map_written_and_parseable() {
        let blobs = vec![
            BlobRef { blob_id: BlobId([0xAA; 32]), offset: DiskOffset(4096), size: 512, chunk_index: 0 },
            BlobRef { blob_id: BlobId([0xBB; 32]), offset: DiskOffset(8192), size: 512, chunk_index: 1 },
        ];
        let mut disk = make_disk(65536);
        let mut off = 0u64;

        let emap_off = ObjectWriter::write_extent_map(
            &blobs,
            |n| { let o = DiskOffset(off); off += n * BLOCK_SIZE as u64; Ok(o) },
            |o, buf| {
                let s = o.0 as usize;
                if s + buf.len() <= disk.len() { disk[s..s+buf.len()].copy_from_slice(buf); }
                Ok(buf.len())
            },
        ).unwrap();

        // Parse count
        let s = emap_off.0 as usize;
        let count = u32::from_le_bytes([disk[s], disk[s+1], disk[s+2], disk[s+3]]);
        assert_eq!(count, 2);
    }

    #[test]
    fn validate_rejects_too_large() {
        let config = ObjectWriterConfig::default();
        let r = validate_object_write(OBJECT_MAX_SIZE + 1, &config);
        assert!(matches!(r, Err(ExofsError::InvalidSize)));
    }
}
