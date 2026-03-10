//! object_reader.rs — Lecture et vérification d'objets ExoFS
//!
//! Pipeline de lecture :
//!   disque → vérif ObjectHeader (HDR-03) → lecture blobs → assemblage → vérif content_hash
//!
//! Règles spec :
//!   HDR-03  : vérifier magic + checksum de l'ObjectHeader AVANT tout accès au payload
//!   HASH-02 : vérifier content_hash (Blake3) sur données assemblées
//!   OOM-02  : try_reserve avant tout Vec::push / extend
//!   ARITH-02: checked_add systématique sur offsets


extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, BlobId, ObjectId, DiskOffset,
};
use crate::fs::exofs::core::blob_id::blake3_hash;
use crate::fs::exofs::storage::storage_stats::STORAGE_STATS;
use crate::fs::exofs::storage::blob_reader::{BlobReader, BlobVerifyMode};
use crate::fs::exofs::storage::object_writer::{
    ObjectHeaderDisk, ObjectType, BlobRef,
    OBJECT_HEADER_MAGIC, OBJECT_HEADER_SIZE,
};

// ─────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────

/// Taille maximale du contenu assemblé (4 GiB)
const MAX_OBJECT_SIZE: usize = 4 * 1024 * 1024 * 1024;

/// Taille d'une entrée dans l'ExtentMap sur disque : blob_id(32) + offset(8) + size(4) = 44
const BLOB_REF_DISK_SIZE: usize = 44;

// ─────────────────────────────────────────────────────────────
// Statistiques
// ─────────────────────────────────────────────────────────────

pub struct ObjectReaderStats {
    pub total_reads: AtomicU64,
    pub total_bytes_read: AtomicU64,
    pub header_ok: AtomicU64,
    pub header_bad_magic: AtomicU64,
    pub header_bad_checksum: AtomicU64,
    pub content_hash_ok: AtomicU64,
    pub content_hash_mismatch: AtomicU64,
    pub extent_map_reads: AtomicU64,
    pub read_errors: AtomicU64,
}

pub static OBJECT_READER_STATS: ObjectReaderStats = ObjectReaderStats {
    total_reads: AtomicU64::new(0),
    total_bytes_read: AtomicU64::new(0),
    header_ok: AtomicU64::new(0),
    header_bad_magic: AtomicU64::new(0),
    header_bad_checksum: AtomicU64::new(0),
    content_hash_ok: AtomicU64::new(0),
    content_hash_mismatch: AtomicU64::new(0),
    extent_map_reads: AtomicU64::new(0),
    read_errors: AtomicU64::new(0),
};

impl ObjectReaderStats {
    pub fn snapshot(&self) -> ObjectReaderStatsSnapshot {
        ObjectReaderStatsSnapshot {
            total_reads: self.total_reads.load(Ordering::Relaxed),
            total_bytes_read: self.total_bytes_read.load(Ordering::Relaxed),
            header_ok: self.header_ok.load(Ordering::Relaxed),
            header_bad_magic: self.header_bad_magic.load(Ordering::Relaxed),
            header_bad_checksum: self.header_bad_checksum.load(Ordering::Relaxed),
            content_hash_ok: self.content_hash_ok.load(Ordering::Relaxed),
            content_hash_mismatch: self.content_hash_mismatch.load(Ordering::Relaxed),
            extent_map_reads: self.extent_map_reads.load(Ordering::Relaxed),
            read_errors: self.read_errors.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ObjectReaderStatsSnapshot {
    pub total_reads: u64,
    pub total_bytes_read: u64,
    pub header_ok: u64,
    pub header_bad_magic: u64,
    pub header_bad_checksum: u64,
    pub content_hash_ok: u64,
    pub content_hash_mismatch: u64,
    pub extent_map_reads: u64,
    pub read_errors: u64,
}

// ─────────────────────────────────────────────────────────────
// Résultat de lecture
// ─────────────────────────────────────────────────────────────

/// Métadonnées extraites de l'ObjectHeader
#[derive(Debug, Clone)]
pub struct ObjectMeta {
    pub object_id: ObjectId,
    pub object_type: ObjectType,
    pub blob_count: u32,
    pub content_size: u64,
    pub epoch: u64,
    pub extent_map_offset: DiskOffset,
    pub content_hash: [u8; 32],
    pub flags: u8,
}

impl ObjectMeta {
    /// Vrai si l'objet est inline (un seul blob, pas d'ExtentMap)
    pub fn is_inline(&self) -> bool {
        self.flags & 0b0000_0001 != 0
    }

    /// Vrai si l'objet a une ExtentMap
    pub fn has_extent_map(&self) -> bool {
        self.flags & 0b0000_0010 != 0
    }
}

/// Résultat complet de lecture d'un objet
#[derive(Debug)]
pub struct ObjectReadResult {
    pub meta: ObjectMeta,
    /// Données assemblées (contenu complet de l'objet)
    pub data: Vec<u8>,
    /// Références blobs lues
    pub blobs: Vec<BlobRef>,
    /// Vrai si le content_hash a été vérifié et correspond
    pub hash_verified: bool,
    /// Octets totaux lus sur disque
    pub disk_bytes_read: u64,
}

// ─────────────────────────────────────────────────────────────
// Mode de vérification
// ─────────────────────────────────────────────────────────────

/// Niveau de vérification à la lecture d'un objet
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectVerifyMode {
    /// Vérification complète : en-tête + blobs + content_hash
    Full,
    /// En-tête + blobs uniquement (sans content_hash final)
    HeaderAndBlobs,
    /// En-tête uniquement
    HeaderOnly,
    /// Aucune vérification (recovery/scrub)
    None,
}

// ─────────────────────────────────────────────────────────────
// ObjectReader principal
// ─────────────────────────────────────────────────────────────

/// Lecteur d'objets ExoFS
pub struct ObjectReader;

impl ObjectReader {
    /// Lit un objet complet depuis son offset d'en-tête.
    ///
    /// Pipeline :
    /// 1. Lecture + vérification ObjectHeader (HDR-03)
    /// 2. Lecture ExtentMap si multi-blob
    /// 3. Lecture et décompression de chaque blob
    /// 4. Assemblage des chunks dans l'ordre
    /// 5. Vérification content_hash (HASH-02)
    pub fn read_object<ReadFn>(
        header_offset: DiskOffset,
        read_fn: ReadFn,
        mode: ObjectVerifyMode,
    ) -> ExofsResult<ObjectReadResult>
    where
        ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
    {
        // ── 1. Lecture + vérif en-tête ────────────────────────────
        let (meta, _disk_hdr) = Self::read_header(header_offset, &read_fn, mode)?;

        if mode == ObjectVerifyMode::HeaderOnly {
            return Ok(ObjectReadResult {
                meta,
                data: Vec::new(),
                blobs: Vec::new(),
                hash_verified: false,
                disk_bytes_read: OBJECT_HEADER_SIZE as u64,
            });
        }

        // ── 2. Lecture ExtentMap ou offset direct ─────────────────
        let blob_refs = if meta.blob_count == 0 {
            Vec::new()
        } else if meta.has_extent_map() {
            Self::read_extent_map(
                DiskOffset(meta.extent_map_offset.0),
                meta.blob_count as usize,
                &read_fn,
            )?
        } else {
            // Blob unique : offset de l'en-tête + OBJECT_HEADER_SIZE
            let blob_off = DiskOffset(
                header_offset.0
                    .checked_add(OBJECT_HEADER_SIZE as u64)
                    .ok_or(ExofsError::Overflow)?
            );
            alloc::vec![BlobRef {
                blob_id: BlobId([0u8; 32]),
                offset: blob_off,
                size: meta.content_size as u32,
                chunk_index: 0,
            }]
        };

        // ── 3 + 4. Lecture et assemblage des blobs ────────────────
        let blob_verify = match mode {
            ObjectVerifyMode::Full | ObjectVerifyMode::HeaderAndBlobs => BlobVerifyMode::Full,
            _ => BlobVerifyMode::None,
        };

        let (data, disk_payload, retrieved_blobs) =
            Self::read_and_assemble(&blob_refs, &read_fn, blob_verify, meta.content_size as usize)?;

        // ── 5. Vérification content_hash ──────────────────────────
        let hash_verified = if mode == ObjectVerifyMode::Full {
            let computed = blake3_hash(&data);
            if computed != meta.content_hash {
                OBJECT_READER_STATS.content_hash_mismatch.fetch_add(1, Ordering::Relaxed);
                STORAGE_STATS.inc_io_error();
                return Err(ExofsError::ChecksumMismatch);
            }
            OBJECT_READER_STATS.content_hash_ok.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        };

        // ── Statistiques ──────────────────────────────────────────
        let total_disk = (OBJECT_HEADER_SIZE as u64).saturating_add(disk_payload);
        OBJECT_READER_STATS.total_reads.fetch_add(1, Ordering::Relaxed);
        OBJECT_READER_STATS.total_bytes_read.fetch_add(total_disk, Ordering::Relaxed);
        STORAGE_STATS.add_read(total_disk);

        Ok(ObjectReadResult {
            meta,
            data,
            blobs: retrieved_blobs,
            hash_verified,
            disk_bytes_read: total_disk,
        })
    }

    /// Lit uniquement les métadonnées (en-tête)
    pub fn read_meta<ReadFn>(
        header_offset: DiskOffset,
        read_fn: ReadFn,
    ) -> ExofsResult<ObjectMeta>
    where
        ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
    {
        let (meta, _) = Self::read_header(header_offset, &read_fn, ObjectVerifyMode::Full)?;
        Ok(meta)
    }

    // ── Lecture de l'en-tête ─────────────────────────────────────────

    fn read_header<ReadFn>(
        offset: DiskOffset,
        read_fn: &ReadFn,
        mode: ObjectVerifyMode,
    ) -> ExofsResult<(ObjectMeta, [u8; OBJECT_HEADER_SIZE])>
    where
        ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
    {
        let raw = read_fn(offset, OBJECT_HEADER_SIZE).map_err(|e| {
            OBJECT_READER_STATS.read_errors.fetch_add(1, Ordering::Relaxed);
            e
        })?;

        if raw.len() < OBJECT_HEADER_SIZE {
            OBJECT_READER_STATS.read_errors.fetch_add(1, Ordering::Relaxed);
            return Err(ExofsError::InvalidSize);
        }

        // SAFETY: taille vérifiée
        let hdr: &ObjectHeaderDisk = unsafe { &*(raw.as_ptr() as *const ObjectHeaderDisk) };

        if mode != ObjectVerifyMode::None {
            if hdr.magic != OBJECT_HEADER_MAGIC {
                OBJECT_READER_STATS.header_bad_magic.fetch_add(1, Ordering::Relaxed);
                STORAGE_STATS.inc_checksum_error();
                return Err(ExofsError::BadMagic);
            }
            if !hdr.verify_checksum() {
                OBJECT_READER_STATS.header_bad_checksum.fetch_add(1, Ordering::Relaxed);
                STORAGE_STATS.inc_checksum_error();
                return Err(ExofsError::ChecksumMismatch);
            }
            OBJECT_READER_STATS.header_ok.fetch_add(1, Ordering::Relaxed);
            STORAGE_STATS.inc_checksum_ok();
        }

        let object_type = Self::object_type_from_byte(hdr.object_type)?;

        let mut raw_bytes = [0u8; OBJECT_HEADER_SIZE];
        raw_bytes.copy_from_slice(&raw[..OBJECT_HEADER_SIZE]);

        let meta = ObjectMeta {
            object_id: ObjectId(hdr.object_id),
            object_type,
            blob_count: hdr.blob_count,
            content_size: hdr.content_size,
            epoch: hdr.epoch,
            extent_map_offset: DiskOffset(hdr.extent_map_offset),
            content_hash: hdr.content_hash,
            flags: hdr.flags,
        };

        Ok((meta, raw_bytes))
    }

    // ── Lecture de l'ExtentMap ───────────────────────────────────────

    fn read_extent_map<ReadFn>(
        offset: DiskOffset,
        count: usize,
        read_fn: &ReadFn,
    ) -> ExofsResult<Vec<BlobRef>>
    where
        ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
    {
        // Taille : 4 (count) + count * 44
        let map_size = 4usize
            .checked_add(count.checked_mul(BLOB_REF_DISK_SIZE).ok_or(ExofsError::Overflow)?)
            .ok_or(ExofsError::Overflow)?;

        OBJECT_READER_STATS.extent_map_reads.fetch_add(1, Ordering::Relaxed);

        let buf = read_fn(offset, map_size)?;
        if buf.len() < map_size {
            return Err(ExofsError::InvalidSize);
        }

        let stored_count = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        if stored_count != count {
            return Err(ExofsError::InvalidArgument);
        }

        let mut refs = Vec::new();
        refs.try_reserve(count).map_err(|_| ExofsError::NoMemory)?;

        let mut pos = 4;
        for chunk_idx in 0..count {
            if pos + BLOB_REF_DISK_SIZE > buf.len() {
                return Err(ExofsError::InvalidSize);
            }

            let mut blob_id_bytes = [0u8; 32];
            blob_id_bytes.copy_from_slice(&buf[pos..pos + 32]);

            let off_bytes: [u8; 8] = buf[pos+32..pos+40].try_into()
                .map_err(|_| ExofsError::InvalidArgument)?;
            let sz_bytes: [u8; 4] = buf[pos+40..pos+44].try_into()
                .map_err(|_| ExofsError::InvalidArgument)?;

            let disk_offset = u64::from_le_bytes(off_bytes);
            let size = u32::from_le_bytes(sz_bytes);

            refs.push(BlobRef {
                blob_id: BlobId(blob_id_bytes),
                offset: DiskOffset(disk_offset),
                size,
                chunk_index: chunk_idx as u32,
            });

            pos = pos.checked_add(BLOB_REF_DISK_SIZE).ok_or(ExofsError::Overflow)?;
        }

        Ok(refs)
    }

    // ── Lecture et assemblage ────────────────────────────────────────

    fn read_and_assemble<ReadFn>(
        blob_refs: &[BlobRef],
        read_fn: &ReadFn,
        verify: BlobVerifyMode,
        expected_size: usize,
    ) -> ExofsResult<(Vec<u8>, u64, Vec<BlobRef>)>
    where
        ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
    {
        if blob_refs.is_empty() {
            return Ok((Vec::new(), 0, Vec::new()));
        }

        if expected_size > MAX_OBJECT_SIZE {
            return Err(ExofsError::InvalidSize);
        }

        let mut assembled = Vec::new();
        assembled.try_reserve(expected_size).map_err(|_| ExofsError::NoMemory)?;

        let mut total_disk = 0u64;
        let mut retrieved = Vec::new();
        retrieved.try_reserve(blob_refs.len()).map_err(|_| ExofsError::NoMemory)?;

        // Trier par chunk_index pour assembler dans le bon ordre
        let mut sorted: Vec<&BlobRef> = blob_refs.iter().collect();
        sorted.sort_by_key(|b| b.chunk_index);

        for bref in &sorted {
            let res = BlobReader::read_blob(
                bref.offset,
                |off, sz| read_fn(off, sz),
                verify,
            ).map_err(|e| {
                OBJECT_READER_STATS.read_errors.fetch_add(1, Ordering::Relaxed);
                e
            })?;

            total_disk = total_disk.saturating_add(res.disk_bytes_read);

            // Vérifier la cohérence taille
            if res.data.len() != bref.size as usize && bref.size != 0 {
                return Err(ExofsError::InvalidSize);
            }

            assembled.try_reserve(res.data.len()).map_err(|_| ExofsError::NoMemory)?;
            assembled.extend_from_slice(&res.data);

            retrieved.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            retrieved.push(BlobRef {
                blob_id: res.blob_id,
                offset: bref.offset,
                size: bref.size,
                chunk_index: bref.chunk_index,
            });
        }

        // Tronquer à la taille attendue si nécessaire (alignement blocs)
        if assembled.len() > expected_size {
            assembled.truncate(expected_size);
        }

        Ok((assembled, total_disk, retrieved))
    }

    fn object_type_from_byte(b: u8) -> ExofsResult<ObjectType> {
        match b {
            0 => Ok(ObjectType::Regular),
            1 => Ok(ObjectType::Directory),
            2 => Ok(ObjectType::Symlink),
            3 => Ok(ObjectType::Device),
            4 => Ok(ObjectType::Metadata),
            _ => Err(ExofsError::InvalidArgument),
        }
    }
}

// ─────────────────────────────────────────────────────────────
// Lecture partielle d'un objet (range read)
// ─────────────────────────────────────────────────────────────

/// Paramètre de lecture partielle
#[derive(Debug, Clone)]
pub struct ObjectRangeRead {
    pub logical_offset: u64,
    pub length: usize,
}

/// Lecteur partiel d'objet
pub struct ObjectRangeReader;

impl ObjectRangeReader {
    /// Lit un sous-ensemble d'un objet (range read).
    ///
    /// N'assemblé que les chunks nécessaires.
    pub fn read_range<ReadFn>(
        header_offset: DiskOffset,
        range: &ObjectRangeRead,
        read_fn: ReadFn,
        chunk_size: usize,
    ) -> ExofsResult<Vec<u8>>
    where
        ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
    {
        let meta = ObjectReader::read_meta(header_offset, |o, sz| read_fn(o, sz))?;

        // Vérification des bornes
        let end = range.logical_offset
            .checked_add(range.length as u64)
            .ok_or(ExofsError::Overflow)?;
        if end > meta.content_size {
            return Err(ExofsError::InvalidArgument);
        }

        // Récupérer les refs blobs
        let blob_refs = if meta.has_extent_map() {
            let emap_raw = read_fn(meta.extent_map_offset, 65536)
                .map_err(|_| ExofsError::InvalidState)?;
            if emap_raw.len() < 4 { return Err(ExofsError::InvalidSize); }
            let cnt = u32::from_le_bytes([emap_raw[0], emap_raw[1], emap_raw[2], emap_raw[3]]) as usize;
            let mut refs = Vec::new();
            refs.try_reserve(cnt).map_err(|_| ExofsError::NoMemory)?;
            let mut pos = 4;
            for i in 0..cnt {
                if pos + 44 > emap_raw.len() { break; }
                let mut bid = [0u8; 32];
                bid.copy_from_slice(&emap_raw[pos..pos+32]);
                let off = u64::from_le_bytes(emap_raw[pos+32..pos+40].try_into().unwrap_or([0u8;8]));
                let sz = u32::from_le_bytes(emap_raw[pos+40..pos+44].try_into().unwrap_or([0u8;4]));
                refs.push(BlobRef { blob_id: BlobId(bid), offset: DiskOffset(off), size: sz, chunk_index: i as u32 });
                pos += 44;
            }
            refs
        } else {
            let blob_off = DiskOffset(
                header_offset.0.checked_add(OBJECT_HEADER_SIZE as u64).ok_or(ExofsError::Overflow)?
            );
            alloc::vec![BlobRef { blob_id: BlobId([0u8;32]), offset: blob_off, size: meta.content_size as u32, chunk_index: 0 }]
        };

        // Identifier les chunks couvrant le range
        let first_chunk = range.logical_offset as usize / chunk_size;
        let last_chunk = ((end as usize) - 1) / chunk_size;

        let mut result = Vec::new();
        result.try_reserve(range.length).map_err(|_| ExofsError::NoMemory)?;

        for bref in &blob_refs {
            let ci = bref.chunk_index as usize;
            if ci < first_chunk || ci > last_chunk { continue; }

            let blob_res = BlobReader::read_blob(
                bref.offset,
                |o, sz| read_fn(o, sz),
                BlobVerifyMode::Full,
            )?;

            // Calculer l'offset dans ce chunk
            let chunk_start_logical = ci * chunk_size;
            let in_chunk_start = if ci == first_chunk {
                (range.logical_offset as usize).saturating_sub(chunk_start_logical)
            } else {
                0
            };
            let chunk_end_logical = chunk_start_logical + blob_res.data.len();
            let in_chunk_end = if ci == last_chunk {
                ((end as usize).saturating_sub(chunk_start_logical)).min(blob_res.data.len())
            } else {
                blob_res.data.len()
            };

            if in_chunk_start < in_chunk_end && in_chunk_end <= blob_res.data.len() {
                result.try_reserve(in_chunk_end - in_chunk_start).map_err(|_| ExofsError::NoMemory)?;
                result.extend_from_slice(&blob_res.data[in_chunk_start..in_chunk_end]);
            }
            let _ = chunk_end_logical;
        }

        Ok(result)
    }
}

// ─────────────────────────────────────────────────────────────
// Scanner d'objets
// ─────────────────────────────────────────────────────────────

/// Scanner séquentiel de métadonnées d'objets
pub struct ObjectScanner {
    offsets: Vec<DiskOffset>,
    pos: usize,
}

impl ObjectScanner {
    pub fn new(offsets: Vec<DiskOffset>) -> Self {
        Self { offsets, pos: 0 }
    }

    /// Lit la prochaine entrée de métadonnées
    pub fn next_meta<ReadFn>(&mut self, read_fn: &ReadFn) -> Option<ExofsResult<(DiskOffset, ObjectMeta)>>
    where
        ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
    {
        if self.pos >= self.offsets.len() { return None; }
        let off = self.offsets[self.pos];
        self.pos += 1;
        Some(ObjectReader::read_meta(off, |o, sz| read_fn(o, sz)).map(|m| (off, m)))
    }

    pub fn remaining(&self) -> usize {
        self.offsets.len().saturating_sub(self.pos)
    }
}

// ─────────────────────────────────────────────────────────────
// Vérification d'intégrité
// ─────────────────────────────────────────────────────────────

/// Rapport de vérification d'objets
#[derive(Debug, Default)]
pub struct ObjectIntegrityReport {
    pub checked: u64,
    pub ok: u64,
    pub bad_header: u64,
    pub bad_content_hash: u64,
    pub read_errors: u64,
}

impl ObjectIntegrityReport {
    pub fn success_rate_pct(&self) -> u64 {
        if self.checked == 0 { return 100; }
        self.ok.saturating_mul(100) / self.checked
    }
}

/// Vérifie une liste d'objets par leur offset de header
pub fn verify_objects<ReadFn>(
    header_offsets: &[DiskOffset],
    read_fn: &ReadFn,
) -> ObjectIntegrityReport
where
    ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
{
    let mut report = ObjectIntegrityReport::default();

    for &off in header_offsets {
        report.checked = report.checked.saturating_add(1);
        match ObjectReader::read_object(off, |o, sz| read_fn(o, sz), ObjectVerifyMode::Full) {
            Ok(_) => {
                report.ok = report.ok.saturating_add(1);
            }
            Err(ExofsError::BadMagic) | Err(ExofsError::ChecksumMismatch) => {
                report.bad_header = report.bad_header.saturating_add(1);
            }
            Err(_) => {
                report.read_errors = report.read_errors.saturating_add(1);
            }
        }
    }

    report
}

// ─────────────────────────────────────────────────────────────
// Tests unitaires
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use crate::fs::exofs::core::{EpochId, ObjectId};
    use crate::fs::exofs::storage::compression_choice::CompressionType;
    use crate::fs::exofs::storage::object_writer::{
        ObjectWriter, ObjectWriterConfig, ObjectType,
    };

    fn oid(n: u8) -> ObjectId { ObjectId([n; 32]) }

    fn write_object_to_disk(disk: &mut Vec<u8>, oid: ObjectId, data: &[u8]) -> DiskOffset {
        let config = ObjectWriterConfig::new(EpochId(1))
            .no_dedup()
            .with_type(ObjectType::Regular);
        let mut off = 0u64;

        let result = ObjectWriter::write_object(
            oid, data, &config,
            |n| { let o = DiskOffset(off); off += n * BLOCK_SIZE as u64; Ok(o) },
            |o, buf| {
                let s = o.0 as usize;
                if s + buf.len() <= disk.len() { disk[s..s+buf.len()].copy_from_slice(buf); }
                Ok(buf.len())
            },
            |_| None,
        ).unwrap();

        // Écrire l'en-tête
        let mut off2 = off;
        let hdr_off = ObjectWriter::write_header(
            &result, &config,
            |n| { let o2 = DiskOffset(off2); off2 += n * BLOCK_SIZE as u64; Ok(o2) },
            |o, buf| {
                let s = o.0 as usize;
                if s + buf.len() <= disk.len() { disk[s..s+buf.len()].copy_from_slice(buf); }
                Ok(buf.len())
            },
        ).unwrap();

        hdr_off
    }

    #[test]
    fn header_roundtrip_valid() {
        let mut disk = vec![0u8; 65536];
        let data = b"Test object content for roundtrip";
        let hdr_off = write_object_to_disk(&mut disk, oid(1), data);

        let read_fn = |o: DiskOffset, sz: usize| -> ExofsResult<Vec<u8>> {
            let s = o.0 as usize;
            Ok(disk[s..s+sz].to_vec())
        };

        let meta = ObjectReader::read_meta(hdr_off, read_fn).unwrap();
        assert_eq!(meta.content_size, data.len() as u64);
        assert_eq!(meta.blob_count, 1);
    }

    #[test]
    fn bad_magic_detected_on_object() {
        let mut disk = vec![0u8; 65536];
        // Magic invalide
        disk[0] = 0xFF; disk[1] = 0xFF; disk[2] = 0xFF; disk[3] = 0xFF;

        let read_fn = |o: DiskOffset, sz: usize| -> ExofsResult<Vec<u8>> {
            let s = o.0 as usize;
            Ok(disk[s..s+sz].to_vec())
        };

        let r = ObjectReader::read_meta(DiskOffset(0), read_fn);
        assert!(matches!(r, Err(ExofsError::BadMagic)));
    }

    #[test]
    fn header_only_mode_fast() {
        let mut disk = vec![0u8; 65536];
        let data = b"header only test";
        let hdr_off = write_object_to_disk(&mut disk, oid(2), data);

        let reads = core::sync::atomic::AtomicU64::new(0);
        let read_fn = |o: DiskOffset, sz: usize| -> ExofsResult<Vec<u8>> {
            reads.fetch_add(1, Ordering::Relaxed);
            let s = o.0 as usize;
            Ok(disk[s..s+sz].to_vec())
        };

        let r = ObjectReader::read_object(hdr_off, read_fn, ObjectVerifyMode::HeaderOnly).unwrap();
        assert!(r.data.is_empty());
        // N'a dû lire qu'une fois (juste l'en-tête)
        assert_eq!(reads.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn object_scanner_meta() {
        let mut disk = vec![0u8; 65536];
        let mut offsets = Vec::new();
        for i in 0u8..3 {
            let data = alloc::vec![i + 0x10; 64];
            let h = write_object_to_disk(&mut disk, oid(10 + i), &data);
            offsets.push(h);
        }

        let read_fn = |o: DiskOffset, sz: usize| -> ExofsResult<Vec<u8>> {
            let s = o.0 as usize;
            Ok(disk[s..s+sz].to_vec())
        };

        let mut scanner = ObjectScanner::new(offsets);
        let mut count = 0;
        while let Some(r) = scanner.next_meta(&read_fn) {
            assert!(r.is_ok());
            count += 1;
        }
        assert_eq!(count, 3);
    }

    #[test]
    fn integrity_report_all_ok() {
        let mut disk = vec![0u8; 65536];
        let data = b"integrity object test";
        let hdr_off = write_object_to_disk(&mut disk, oid(5), data);

        let read_fn = |o: DiskOffset, sz: usize| -> ExofsResult<Vec<u8>> {
            let s = o.0 as usize;
            Ok(disk[s..s+sz].to_vec())
        };

        let report = verify_objects(&[hdr_off], &read_fn);
        assert_eq!(report.ok, 1);
        assert_eq!(report.bad_header, 0);
    }
}
