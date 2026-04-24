//! blob_reader.rs — Pipeline complet de lecture de blobs ExoFS
//!
//! Pipeline : disque → vérif en-tête (HDR-03) → décompression → verify_blob_id (HASH-02)
//!
//! Règles spec :
//!   HDR-03   : vérifier magic + checksum d'en-tête AVANT d'accéder au payload
//!   HASH-02  : vérifier BlobId (Blake3 sur données décompressées) en fin de lecture
//!   OOM-02   : try_reserve avant tout Vec::push / resize
//!   ARITH-02 : checked_add pour toute arithmétique sur offsets

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::blob_id::verify_blob_id;
use crate::fs::exofs::core::{BlobId, DiskOffset, ExofsError, ExofsResult};
use crate::fs::exofs::crypto::secret_reader::SecretReader;
use crate::fs::exofs::storage::blob_writer::{
    blob_total_disk_size, verify_blob_header, BlobHeaderDisk, BlobWriter, BLOB_HEADER_MAGIC,
    BLOB_HEADER_SIZE,
};
use crate::fs::exofs::storage::compression_choice::CompressionType;
use crate::fs::exofs::storage::compression_reader::DecompressReader;
use crate::fs::exofs::storage::layout::BLOCK_SIZE;
use crate::fs::exofs::storage::storage_stats::STORAGE_STATS;

// ─────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────

/// Taille maximale autorisée pour un payload décompressé (512 MiB)
const MAX_DECOMPRESSED_SIZE: usize = 512 * 1024 * 1024;

/// Flag header : payload chiffré.
const BLOB_FLAG_ENCRYPTED: u8 = 0b0000_0100;

/// Taille minimale d'un blob valide sur disque
#[allow(dead_code)]
const MIN_BLOB_DISK_SIZE: usize = BLOB_HEADER_SIZE;

// ─────────────────────────────────────────────────────────────
// Statistiques
// ─────────────────────────────────────────────────────────────

/// Compteurs globaux pour le lecteur de blobs
pub struct BlobReaderStats {
    pub total_reads: AtomicU64,
    pub total_bytes_read: AtomicU64,
    pub total_bytes_decompressed: AtomicU64,
    pub header_ok: AtomicU64,
    pub header_bad_magic: AtomicU64,
    pub header_bad_checksum: AtomicU64,
    pub blob_id_ok: AtomicU64,
    pub blob_id_mismatch: AtomicU64,
    pub decompress_errors: AtomicU64,
    pub read_errors: AtomicU64,
}

pub static BLOB_READER_STATS: BlobReaderStats = BlobReaderStats {
    total_reads: AtomicU64::new(0),
    total_bytes_read: AtomicU64::new(0),
    total_bytes_decompressed: AtomicU64::new(0),
    header_ok: AtomicU64::new(0),
    header_bad_magic: AtomicU64::new(0),
    header_bad_checksum: AtomicU64::new(0),
    blob_id_ok: AtomicU64::new(0),
    blob_id_mismatch: AtomicU64::new(0),
    decompress_errors: AtomicU64::new(0),
    read_errors: AtomicU64::new(0),
};

impl BlobReaderStats {
    pub fn snapshot(&self) -> BlobReaderStatsSnapshot {
        BlobReaderStatsSnapshot {
            total_reads: self.total_reads.load(Ordering::Relaxed),
            total_bytes_read: self.total_bytes_read.load(Ordering::Relaxed),
            total_bytes_decompressed: self.total_bytes_decompressed.load(Ordering::Relaxed),
            header_ok: self.header_ok.load(Ordering::Relaxed),
            header_bad_magic: self.header_bad_magic.load(Ordering::Relaxed),
            header_bad_checksum: self.header_bad_checksum.load(Ordering::Relaxed),
            blob_id_ok: self.blob_id_ok.load(Ordering::Relaxed),
            blob_id_mismatch: self.blob_id_mismatch.load(Ordering::Relaxed),
            decompress_errors: self.decompress_errors.load(Ordering::Relaxed),
            read_errors: self.read_errors.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BlobReaderStatsSnapshot {
    pub total_reads: u64,
    pub total_bytes_read: u64,
    pub total_bytes_decompressed: u64,
    pub header_ok: u64,
    pub header_bad_magic: u64,
    pub header_bad_checksum: u64,
    pub blob_id_ok: u64,
    pub blob_id_mismatch: u64,
    pub decompress_errors: u64,
    pub read_errors: u64,
}

// ─────────────────────────────────────────────────────────────
// Mode de vérification
// ─────────────────────────────────────────────────────────────

/// Contrôle quelles vérifications effectuer à la lecture
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobVerifyMode {
    /// Vérification complète : en-tête + BlobId (recommandé)
    Full,
    /// En-tête seulement (plus rapide, moins sûr)
    HeaderOnly,
    /// Aucune vérification (mode scrub/recovery)
    None,
}

// ─────────────────────────────────────────────────────────────
// Résultat de lecture
// ─────────────────────────────────────────────────────────────

/// Résultat d'une lecture de blob
#[derive(Debug)]
pub struct BlobReadResult {
    /// BlobId extrait de l'en-tête
    pub blob_id: BlobId,
    /// Données décompressées (données RAW originales)
    pub data: Vec<u8>,
    /// Taille originale (depuis l'en-tête)
    pub original_size: u32,
    /// Taille stockée (depuis l'en-tête)
    pub stored_size: u32,
    /// Algorithme de compression appliqué
    pub algo: CompressionType,
    /// Taille totale lue sur disque (header + payload)
    pub disk_bytes_read: u64,
    /// Vérification BlobId réussie
    pub id_verified: bool,
}

// ─────────────────────────────────────────────────────────────
// BlobReader principal
// ─────────────────────────────────────────────────────────────

/// Lecteur de blobs ExoFS — pipeline complet
pub struct BlobReader;

impl BlobReader {
    /// Lit et vérifie un blob à partir d'un offset disque.
    ///
    /// Pipeline :
    /// 1. Lecture de l'en-tête (BLOB_HEADER_SIZE octets)
    /// 2. Vérification magic + checksum (HDR-03)
    /// 3. Lecture du payload (stored_size octets)
    /// 4. Décompression si nécessaire
    /// 5. Vérification BlobId sur données décompressées (HASH-02)
    ///
    /// # Paramètres
    /// - `offset` : offset disque du début du blob (header inclus)
    /// - `read_fn` : fonction de lecture physique → (offset, taille) → vec d'octets lus
    /// - `mode` : niveau de vérification
    pub fn read_blob<ReadFn>(
        offset: DiskOffset,
        read_fn: ReadFn,
        mode: BlobVerifyMode,
    ) -> ExofsResult<BlobReadResult>
    where
        ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
    {
        // ── 1. Lecture de l'en-tête ────────────────────────────────
        let hdr_raw = read_fn(offset, BLOB_HEADER_SIZE).map_err(|e| {
            BLOB_READER_STATS
                .read_errors
                .fetch_add(1, Ordering::Relaxed);
            e
        })?;

        if hdr_raw.len() < BLOB_HEADER_SIZE {
            BLOB_READER_STATS
                .read_errors
                .fetch_add(1, Ordering::Relaxed);
            return Err(ExofsError::InvalidSize);
        }

        // ── 2. Vérification en-tête (HDR-03) ──────────────────────
        let hdr = match mode {
            BlobVerifyMode::None => {
                // SAFETY: taille vérifiée ci-dessus
                unsafe { &*(hdr_raw.as_ptr() as *const BlobHeaderDisk) }
            }
            _ => match verify_blob_header(&hdr_raw) {
                Ok(h) => {
                    BLOB_READER_STATS.header_ok.fetch_add(1, Ordering::Relaxed);
                    STORAGE_STATS.inc_checksum_ok();
                    h
                }
                Err(ExofsError::BadMagic) => {
                    BLOB_READER_STATS
                        .header_bad_magic
                        .fetch_add(1, Ordering::Relaxed);
                    STORAGE_STATS.inc_checksum_error();
                    STORAGE_STATS.inc_io_error();
                    return Err(ExofsError::BadMagic);
                }
                Err(ExofsError::ChecksumMismatch) => {
                    BLOB_READER_STATS
                        .header_bad_checksum
                        .fetch_add(1, Ordering::Relaxed);
                    STORAGE_STATS.inc_checksum_error();
                    STORAGE_STATS.inc_io_error();
                    return Err(ExofsError::ChecksumMismatch);
                }
                Err(e) => return Err(e),
            },
        };

        // Extraction des champs
        let magic = hdr.magic;
        if mode != BlobVerifyMode::None && magic != BLOB_HEADER_MAGIC {
            return Err(ExofsError::BadMagic);
        }

        let original_size = hdr.original_size;
        let stored_size = hdr.stored_size;
        let algo = Self::algo_from_byte(hdr.compression_algo)?;
        let blob_id = BlobId(hdr.blob_id);

        // Sanity checks sur les tailles
        if stored_size == 0 {
            return Err(ExofsError::InvalidSize);
        }
        if original_size as usize > MAX_DECOMPRESSED_SIZE {
            return Err(ExofsError::InvalidSize);
        }
        if stored_size as usize > MAX_DECOMPRESSED_SIZE {
            return Err(ExofsError::InvalidSize);
        }

        // ── 3. Lecture du payload ──────────────────────────────────
        let payload_offset = DiskOffset(
            offset
                .0
                .checked_add(BLOB_HEADER_SIZE as u64)
                .ok_or(ExofsError::Overflow)?,
        );

        let payload = read_fn(payload_offset, stored_size as usize).map_err(|e| {
            BLOB_READER_STATS
                .read_errors
                .fetch_add(1, Ordering::Relaxed);
            e
        })?;

        if payload.len() < stored_size as usize {
            BLOB_READER_STATS
                .read_errors
                .fetch_add(1, Ordering::Relaxed);
            return Err(ExofsError::InvalidSize);
        }

        let disk_bytes = BLOB_HEADER_SIZE
            .checked_add(stored_size as usize)
            .ok_or(ExofsError::Overflow)? as u64;

        // ── 4. Déchiffrement (si activé) puis décompression ───────
        let mut clear_payload: Vec<u8> = Vec::new();
        let payload_slice = &payload[..stored_size as usize];
        if (hdr.flags & BLOB_FLAG_ENCRYPTED) != 0 {
            let key = BlobWriter::payload_key_for(&blob_id)?;
            clear_payload = SecretReader::new(&key).decrypt(payload_slice)?;
        } else {
            clear_payload
                .try_reserve(payload_slice.len())
                .map_err(|_| ExofsError::NoMemory)?;
            clear_payload.extend_from_slice(payload_slice);
        }

        let data = Self::decompress_payload(&clear_payload, algo, original_size)?;

        // ── 5. Vérification BlobId (HASH-02) ──────────────────────
        let id_verified = if mode == BlobVerifyMode::Full {
            if verify_blob_id(&blob_id, &data) {
                BLOB_READER_STATS.blob_id_ok.fetch_add(1, Ordering::Relaxed);
                true
            } else {
                BLOB_READER_STATS
                    .blob_id_mismatch
                    .fetch_add(1, Ordering::Relaxed);
                STORAGE_STATS.inc_io_error();
                return Err(ExofsError::ChecksumMismatch);
            }
        } else {
            false
        };

        // ── Statistiques ───────────────────────────────────────────
        BLOB_READER_STATS
            .total_reads
            .fetch_add(1, Ordering::Relaxed);
        BLOB_READER_STATS
            .total_bytes_read
            .fetch_add(disk_bytes, Ordering::Relaxed);
        BLOB_READER_STATS
            .total_bytes_decompressed
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        STORAGE_STATS.add_read(disk_bytes);

        Ok(BlobReadResult {
            blob_id,
            data,
            original_size,
            stored_size,
            algo,
            disk_bytes_read: disk_bytes,
            id_verified,
        })
    }

    /// Lit uniquement l'en-tête sans charger le payload
    pub fn read_header<ReadFn>(offset: DiskOffset, read_fn: ReadFn) -> ExofsResult<BlobHeaderInfo>
    where
        ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
    {
        let raw = read_fn(offset, BLOB_HEADER_SIZE)?;
        let hdr = verify_blob_header(&raw)?;
        Ok(BlobHeaderInfo {
            blob_id: BlobId(hdr.blob_id),
            original_size: hdr.original_size,
            stored_size: hdr.stored_size,
            algo: Self::algo_from_byte(hdr.compression_algo)?,
            epoch: hdr.epoch,
        })
    }

    // ── Décompression interne ───────────────────────────────────────

    fn decompress_payload(
        payload: &[u8],
        algo: CompressionType,
        original_size: u32,
    ) -> ExofsResult<Vec<u8>> {
        match algo {
            CompressionType::None => {
                // Pas de compression : copie directe
                let mut data = Vec::new();
                data.try_reserve(payload.len())
                    .map_err(|_| ExofsError::NoMemory)?;
                data.extend_from_slice(payload);
                Ok(data)
            }
            CompressionType::Lz4 | CompressionType::Zstd => {
                let result =
                    DecompressReader::decompress_raw(payload, algo, original_size as usize);
                match result {
                    Ok(d) => Ok(d),
                    Err(e) => {
                        BLOB_READER_STATS
                            .decompress_errors
                            .fetch_add(1, Ordering::Relaxed);
                        STORAGE_STATS.inc_io_error();
                        Err(e)
                    }
                }
            }
        }
    }

    fn algo_from_byte(byte: u8) -> ExofsResult<CompressionType> {
        match byte {
            0 => Ok(CompressionType::None),
            1 => Ok(CompressionType::Lz4),
            2 => Ok(CompressionType::Zstd),
            _ => Err(ExofsError::InvalidArgument),
        }
    }
}

// ─────────────────────────────────────────────────────────────
// Informations d'en-tête (sans payload)
// ─────────────────────────────────────────────────────────────

/// Informations extraites de l'en-tête d'un blob
#[derive(Debug, Clone)]
pub struct BlobHeaderInfo {
    pub blob_id: BlobId,
    pub original_size: u32,
    pub stored_size: u32,
    pub algo: CompressionType,
    pub epoch: u64,
}

impl BlobHeaderInfo {
    /// Taille totale sur disque (header + payload aligné)
    pub fn disk_size(&self) -> u64 {
        blob_total_disk_size(self.stored_size)
    }

    /// Offset du bloc suivant
    pub fn next_offset(&self, current: DiskOffset) -> ExofsResult<DiskOffset> {
        let ds = self.disk_size();
        let next = current.0.checked_add(ds).ok_or(ExofsError::Overflow)?;
        Ok(DiskOffset(next))
    }
}

// ─────────────────────────────────────────────────────────────
// BatchBlobReader — lecture multiple
// ─────────────────────────────────────────────────────────────

/// Requête de lecture batch
#[derive(Debug, Clone)]
pub struct BlobReadRequest {
    pub index: usize,
    pub offset: DiskOffset,
    pub mode: BlobVerifyMode,
}

/// Résultat batch
#[derive(Debug)]
pub struct BatchBlobReadResult {
    pub index: usize,
    pub result: ExofsResult<BlobReadResult>,
}

/// Reader de blobs en batch
pub struct BatchBlobReader;

impl BatchBlobReader {
    /// Lit plusieurs blobs en séquence
    pub fn read_all<ReadFn>(
        requests: &[BlobReadRequest],
        read_fn: &ReadFn,
    ) -> ExofsResult<Vec<BatchBlobReadResult>>
    where
        ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
    {
        let mut results = Vec::new();
        results
            .try_reserve(requests.len())
            .map_err(|_| ExofsError::NoMemory)?;

        for req in requests {
            let r = BlobReader::read_blob(req.offset, |off, sz| read_fn(off, sz), req.mode);
            results.push(BatchBlobReadResult {
                index: req.index,
                result: r,
            });
        }

        Ok(results)
    }

    /// Lit uniquement les en-têtes (scan de métadonnées)
    pub fn scan_headers<ReadFn>(
        offsets: &[DiskOffset],
        read_fn: &ReadFn,
    ) -> ExofsResult<Vec<(DiskOffset, ExofsResult<BlobHeaderInfo>)>>
    where
        ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
    {
        let mut out = Vec::new();
        out.try_reserve(offsets.len())
            .map_err(|_| ExofsError::NoMemory)?;

        for &off in offsets {
            let h = BlobReader::read_header(off, |o, sz| read_fn(o, sz));
            out.push((off, h));
        }

        Ok(out)
    }
}

// ─────────────────────────────────────────────────────────────
// Scanner séquentiel de blobs
// ─────────────────────────────────────────────────────────────

/// Parcourt les blobs d'une zone séquentiellement
pub struct BlobScanner {
    current_offset: DiskOffset,
    end_offset: DiskOffset,
    scanned: u64,
    errors: u64,
}

impl BlobScanner {
    pub fn new(start: DiskOffset, end: DiskOffset) -> Self {
        Self {
            current_offset: start,
            end_offset: end,
            scanned: 0,
            errors: 0,
        }
    }

    /// Scanne le prochain blob (lecture d'en-tête uniquement)
    pub fn next_header<ReadFn>(
        &mut self,
        read_fn: &ReadFn,
    ) -> Option<ExofsResult<(DiskOffset, BlobHeaderInfo)>>
    where
        ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
    {
        if self.current_offset.0 >= self.end_offset.0 {
            return None;
        }
        // Doit avoir assez de place pour un en-tête
        let remaining = self.end_offset.0.saturating_sub(self.current_offset.0);
        if remaining < BLOB_HEADER_SIZE as u64 {
            return None;
        }

        let off = self.current_offset;
        match BlobReader::read_header(off, |o, sz| read_fn(o, sz)) {
            Ok(info) => {
                let next = match info.next_offset(off) {
                    Ok(n) => n,
                    Err(e) => {
                        self.errors = self.errors.saturating_add(1);
                        return Some(Err(e));
                    }
                };
                self.current_offset = next;
                self.scanned = self.scanned.saturating_add(1);
                Some(Ok((off, info)))
            }
            Err(e) => {
                self.errors = self.errors.saturating_add(1);
                // Avance d'un bloc pour tenter de récupérer
                self.current_offset =
                    DiskOffset(self.current_offset.0.saturating_add(BLOCK_SIZE as u64));
                Some(Err(e))
            }
        }
    }

    /// Scanne un blob complet
    pub fn next_blob<ReadFn>(
        &mut self,
        read_fn: &ReadFn,
        mode: BlobVerifyMode,
    ) -> Option<ExofsResult<(DiskOffset, BlobReadResult)>>
    where
        ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
    {
        if self.current_offset.0 >= self.end_offset.0 {
            return None;
        }

        let off = self.current_offset;
        match BlobReader::read_blob(off, |o, sz| read_fn(o, sz), mode) {
            Ok(res) => {
                let disk_size = blob_total_disk_size(res.stored_size);
                self.current_offset = DiskOffset(off.0.saturating_add(disk_size));
                self.scanned = self.scanned.saturating_add(1);
                Some(Ok((off, res)))
            }
            Err(e) => {
                self.errors = self.errors.saturating_add(1);
                self.current_offset =
                    DiskOffset(self.current_offset.0.saturating_add(BLOCK_SIZE as u64));
                Some(Err(e))
            }
        }
    }

    /// Offset courant
    pub fn current_offset(&self) -> DiskOffset {
        self.current_offset
    }

    /// Nombre de blobs scannés avec succès
    pub fn scanned_count(&self) -> u64 {
        self.scanned
    }

    /// Nombre d'erreurs rencontrées
    pub fn error_count(&self) -> u64 {
        self.errors
    }

    /// Progression (0–100)
    pub fn progress_pct(&self) -> u64 {
        let total = self.end_offset.0.saturating_sub(self.current_offset.0);
        let done = self.current_offset.0;
        if total == 0 {
            return 100;
        }
        done.saturating_mul(100) / total.saturating_add(done)
    }
}

// ─────────────────────────────────────────────────────────────
// Vérificateur d'intégrité
// ─────────────────────────────────────────────────────────────

/// Rapport de vérification d'intégrité
#[derive(Debug, Default)]
pub struct IntegrityReport {
    pub checked: u64,
    pub ok: u64,
    pub bad_magic: u64,
    pub bad_header_checksum: u64,
    pub bad_blob_id: u64,
    pub decompress_errors: u64,
    pub read_errors: u64,
}

impl IntegrityReport {
    /// Taux de succès en pourcentage
    pub fn success_rate_pct(&self) -> u64 {
        if self.checked == 0 {
            return 100;
        }
        self.ok.saturating_mul(100) / self.checked
    }

    /// Nombre total d'erreurs
    pub fn total_errors(&self) -> u64 {
        self.bad_magic
            .saturating_add(self.bad_header_checksum)
            .saturating_add(self.bad_blob_id)
            .saturating_add(self.decompress_errors)
            .saturating_add(self.read_errors)
    }
}

/// Vérifie l'intégrité de tous les blobs dans une plage d'offsets
pub fn verify_blob_range<ReadFn>(
    start: DiskOffset,
    end: DiskOffset,
    read_fn: &ReadFn,
) -> IntegrityReport
where
    ReadFn: Fn(DiskOffset, usize) -> ExofsResult<Vec<u8>>,
{
    let mut report = IntegrityReport::default();
    let mut scanner = BlobScanner::new(start, end);

    while let Some(r) = scanner.next_blob(read_fn, BlobVerifyMode::Full) {
        report.checked = report.checked.saturating_add(1);
        match r {
            Ok(_) => {
                report.ok = report.ok.saturating_add(1);
            }
            Err(ExofsError::BadMagic) => {
                report.bad_magic = report.bad_magic.saturating_add(1);
            }
            Err(ExofsError::ChecksumMismatch) => {
                report.bad_blob_id = report.bad_blob_id.saturating_add(1);
            }
            Err(ExofsError::DecompressError) => {
                report.decompress_errors = report.decompress_errors.saturating_add(1);
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
    use crate::fs::exofs::core::EpochId;
    use crate::fs::exofs::storage::blob_writer::{BlobWriter, BlobWriterConfig};
    use alloc::vec;

    fn make_disk(size: usize) -> Vec<u8> {
        vec![0u8; size]
    }

    fn write_blob_to_disk(disk: &mut Vec<u8>, data: &[u8], offset: usize) {
        let config = BlobWriterConfig::new(EpochId(1))
            .no_dedup()
            .with_algo(CompressionType::None);
        let _ = BlobWriter::write_blob(
            data,
            &config,
            |_| Ok(DiskOffset(offset as u64)),
            |off, buf| {
                let s = off.0 as usize;
                if s + buf.len() <= disk.len() {
                    disk[s..s + buf.len()].copy_from_slice(buf);
                }
                Ok(buf.len())
            },
            |_| None,
        );
    }

    #[test]
    fn read_roundtrip_no_compression() {
        let data = b"Hello ExoFS blob reader!";
        let mut disk = make_disk(8192);
        write_blob_to_disk(&mut disk, data, 0);

        let read_fn = |off: DiskOffset, sz: usize| -> ExofsResult<Vec<u8>> {
            let s = off.0 as usize;
            let e = s + sz;
            if e > disk.len() {
                return Err(ExofsError::InvalidSize);
            }
            let mut v = Vec::new();
            v.try_reserve(sz).map_err(|_| ExofsError::NoMemory)?;
            v.extend_from_slice(&disk[s..e]);
            Ok(v)
        };

        let r = BlobReader::read_blob(DiskOffset(0), read_fn, BlobVerifyMode::Full).unwrap();
        assert_eq!(&r.data[..data.len()], data);
        assert!(r.id_verified);
        assert_eq!(r.original_size as usize, data.len());
    }

    #[test]
    fn bad_magic_detected() {
        let mut disk = make_disk(8192);
        // Écrire un magic invalide
        disk[0] = 0xFF;
        disk[1] = 0xFF;
        disk[2] = 0xFF;
        disk[3] = 0xFF;

        let read_fn = |off: DiskOffset, sz: usize| -> ExofsResult<Vec<u8>> {
            let s = off.0 as usize;
            Ok(disk[s..s + sz].to_vec())
        };

        let r = BlobReader::read_blob(DiskOffset(0), read_fn, BlobVerifyMode::Full);
        assert!(matches!(r, Err(ExofsError::BadMagic)));
    }

    #[test]
    fn read_header_only() {
        let data = b"test header only";
        let mut disk = make_disk(8192);
        write_blob_to_disk(&mut disk, data, 0);

        let read_fn = |off: DiskOffset, sz: usize| -> ExofsResult<Vec<u8>> {
            let s = off.0 as usize;
            Ok(disk[s..s + sz].to_vec())
        };

        let info = BlobReader::read_header(DiskOffset(0), read_fn).unwrap();
        assert_eq!(info.original_size as usize, data.len());
        assert_eq!(info.algo, CompressionType::None);
    }

    #[test]
    fn scanner_traverses_blobs() {
        let blobs: &[&[u8]] = &[b"blob_one", b"blob_two", b"blob_three"];
        let mut disk = make_disk(65536);
        let mut off = 0usize;
        for b in blobs {
            write_blob_to_disk(&mut disk, b, off);
            off += BlobWriter::disk_size_for(b.len()) as usize;
        }

        let end = DiskOffset(off as u64);
        let mut scanner = BlobScanner::new(DiskOffset(0), end);

        let read_fn = |o: DiskOffset, sz: usize| -> ExofsResult<Vec<u8>> {
            let s = o.0 as usize;
            Ok(disk[s..s + sz].to_vec())
        };

        let mut count = 0;
        while let Some(r) = scanner.next_header(&read_fn) {
            assert!(r.is_ok());
            count += 1;
        }
        assert_eq!(count, blobs.len());
    }

    #[test]
    fn batch_reader_all() {
        let blobs: &[&[u8]] = &[b"alpha", b"beta", b"gamma"];
        let mut disk = make_disk(65536);
        let mut offsets = Vec::new();
        let mut off = 0usize;
        for b in blobs {
            offsets.push(DiskOffset(off as u64));
            write_blob_to_disk(&mut disk, b, off);
            off += BlobWriter::disk_size_for(b.len()) as usize;
        }

        let requests: Vec<_> = offsets
            .iter()
            .enumerate()
            .map(|(i, &o)| BlobReadRequest {
                index: i,
                offset: o,
                mode: BlobVerifyMode::Full,
            })
            .collect();

        let read_fn = |o: DiskOffset, sz: usize| -> ExofsResult<Vec<u8>> {
            let s = o.0 as usize;
            Ok(disk[s..s + sz].to_vec())
        };

        let results = BatchBlobReader::read_all(&requests, &read_fn).unwrap();
        assert_eq!(results.len(), 3);
        for r in &results {
            assert!(r.result.is_ok());
        }
    }

    #[test]
    fn integrity_report_all_ok() {
        let data = b"integrity check test data";
        let mut disk = make_disk(8192);
        write_blob_to_disk(&mut disk, data, 0);
        let end = DiskOffset(BlobWriter::disk_size_for(data.len()));

        let read_fn = |o: DiskOffset, sz: usize| -> ExofsResult<Vec<u8>> {
            let s = o.0 as usize;
            Ok(disk[s..s + sz].to_vec())
        };

        let report = verify_blob_range(DiskOffset(0), end, &read_fn);
        assert_eq!(report.ok, 1);
        assert_eq!(report.total_errors(), 0);
    }
}
