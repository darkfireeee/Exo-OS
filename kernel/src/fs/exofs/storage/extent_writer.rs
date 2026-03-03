// kernel/src/fs/exofs/storage/extent_writer.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Écriture d'extensions (extents) disque — ExoFS
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// ExtentWriter écrit un buffer de données dans un ou plusieurs blocs disque
// contigus. Il gère l'alignement sur BLOCK_SIZE, le découpage en segments
// si nécessaire et la vérification WRITE-02 après chaque écriture.
//
// Règles ExoFS :
// - WRITE-02 : bytes_written == expected après chaque write.
// - ARITH-02 : checked_add pour tous les offsets.
// - OOM-02   : try_reserve avant Vec::push.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult, DiskOffset};
use crate::fs::exofs::storage::layout::{BLOCK_SIZE, align_up};
use crate::fs::exofs::storage::storage_stats::STORAGE_STATS;
use crate::fs::exofs::storage::block_cache::BlockCache;

// ─────────────────────────────────────────────────────────────────────────────
// Extent — plage de blocs disque contigus
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Extent {
    pub offset: DiskOffset,
    pub size:   u64,
}

impl Extent {
    pub fn new(offset: DiskOffset, size: u64) -> Self { Self { offset, size } }

    pub fn end(&self) -> Option<DiskOffset> {
        self.offset.0.checked_add(self.size).map(DiskOffset)
    }

    pub fn block_count(&self) -> u64 {
        (self.size.saturating_add(BLOCK_SIZE as u64 - 1)) / BLOCK_SIZE as u64
    }

    pub fn is_block_aligned(&self) -> bool {
        (self.offset.0 % BLOCK_SIZE as u64 == 0) && (self.size % BLOCK_SIZE as u64 == 0)
    }

    pub fn overlaps(&self, other: &Extent) -> bool {
        let self_end  = self.offset.0.saturating_add(self.size);
        let other_end = other.offset.0.saturating_add(other.size);
        self.offset.0 < other_end && other.offset.0 < self_end
    }

    pub fn contains_offset(&self, off: DiskOffset) -> bool {
        off.0 >= self.offset.0 && off.0 < self.offset.0.saturating_add(self.size)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WriteSegment — sous-partie d'une écriture découpée
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct WriteSegment {
    pub offset: DiskOffset,
    pub data:   Vec<u8>,
}

// ─────────────────────────────────────────────────────────────────────────────
// ExtentWriteResult
// ─────────────────────────────────────────────────────────────────────────────

pub struct ExtentWriteResult {
    pub extent:        Extent,
    pub bytes_written: u64,
    pub segments:      u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// ExtentWriter
// ─────────────────────────────────────────────────────────────────────────────

pub struct ExtentWriter {
    bytes_written: AtomicU64,
    write_ops:     AtomicU64,
    errors:        AtomicU64,
}

impl ExtentWriter {
    pub fn new() -> Self {
        Self {
            bytes_written: AtomicU64::new(0),
            write_ops:     AtomicU64::new(0),
            errors:        AtomicU64::new(0),
        }
    }

    /// Écrit `data` à `offset`. WRITE-02 : vérifie le nombre d'octets écrits.
    pub fn write(
        &self,
        offset:   DiskOffset,
        data:     &[u8],
        write_fn: &dyn Fn(&[u8], DiskOffset) -> ExofsResult<usize>,
    ) -> ExofsResult<ExtentWriteResult> {
        if data.is_empty() { return Err(ExofsError::InvalidArgument); }

        let expected = data.len() as u64;
        let n        = write_fn(data, offset)?;

        // WRITE-02 : vérification stricte.
        if (n as u64) != expected {
            self.errors.fetch_add(1, Ordering::Relaxed);
            STORAGE_STATS.inc_io_error();
            return Err(ExofsError::ShortWrite);
        }

        self.bytes_written.fetch_add(n as u64, Ordering::Relaxed);
        self.write_ops.fetch_add(1, Ordering::Relaxed);
        STORAGE_STATS.add_write(n as u64);

        let extent = Extent::new(offset, n as u64);
        Ok(ExtentWriteResult { extent, bytes_written: n as u64, segments: 1 })
    }

    /// Écrit `data` en découpant en segments de `BLOCK_SIZE`.
    pub fn write_blocks(
        &self,
        start_offset: DiskOffset,
        data:         &[u8],
        write_fn:     &dyn Fn(&[u8], DiskOffset) -> ExofsResult<usize>,
    ) -> ExofsResult<ExtentWriteResult> {
        if data.is_empty() { return Err(ExofsError::InvalidArgument); }

        let blk      = BLOCK_SIZE as usize;
        let mut pos  = 0usize;
        let mut disk = start_offset.0;
        let mut total_written = 0u64;
        let mut seg_count     = 0u64;

        while pos < data.len() {
            let chunk_len = (data.len() - pos).min(blk);
            let chunk     = &data[pos..pos + chunk_len];

            let off = DiskOffset(disk);
            let n   = write_fn(chunk, off)?;

            // WRITE-02.
            if n != chunk_len {
                self.errors.fetch_add(1, Ordering::Relaxed);
                return Err(ExofsError::ShortWrite);
            }

            total_written = total_written.checked_add(n as u64).ok_or(ExofsError::Overflow)?;
            disk          = disk.checked_add(n as u64).ok_or(ExofsError::Overflow)?;
            pos          += n;
            seg_count     = seg_count.saturating_add(1);
        }

        self.bytes_written.fetch_add(total_written, Ordering::Relaxed);
        self.write_ops.fetch_add(seg_count, Ordering::Relaxed);
        STORAGE_STATS.add_write(total_written);

        let extent = Extent::new(start_offset, total_written);
        Ok(ExtentWriteResult { extent, bytes_written: total_written, segments: seg_count })
    }

    /// Écrit via le cache en mode dirty (write-back).
    pub fn write_cached(
        &self,
        offset: DiskOffset,
        data:   &[u8],
        cache:  &BlockCache,
    ) -> ExofsResult<ExtentWriteResult> {
        let blk = BLOCK_SIZE as usize;
        if data.len() != blk { return Err(ExofsError::InvalidArgument); }

        cache.write_block_dirty(offset, data)?;
        let extent = Extent::new(offset, blk as u64);
        Ok(ExtentWriteResult { extent, bytes_written: blk as u64, segments: 1 })
    }

    /// Aligne un offset vers le bloc suivant.
    pub fn align_to_block(offset: u64) -> u64 {
        align_up(offset, BLOCK_SIZE as u64)
    }

    /// Découpe `data` en segments alignés sur BLOCK_SIZE.
    pub fn split_into_segments(
        start_offset: DiskOffset,
        data:         &[u8],
    ) -> ExofsResult<Vec<WriteSegment>> {
        let blk  = BLOCK_SIZE as usize;
        let n_seg = data.len().saturating_add(blk - 1) / blk;
        let mut segs: Vec<WriteSegment> = Vec::new();
        segs.try_reserve(n_seg).map_err(|_| ExofsError::NoMemory)?;

        let mut pos  = 0usize;
        let mut disk = start_offset.0;

        while pos < data.len() {
            let len   = (data.len() - pos).min(blk);
            let chunk = &data[pos..pos + len];

            let mut seg_data: Vec<u8> = Vec::new();
            seg_data.try_reserve(blk).map_err(|_| ExofsError::NoMemory)?;
            seg_data.extend_from_slice(chunk);
            // Padding au bloc complet avec zéros.
            while seg_data.len() < blk { seg_data.push(0u8); }

            segs.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            segs.push(WriteSegment { offset: DiskOffset(disk), data: seg_data });

            disk = disk.checked_add(blk as u64).ok_or(ExofsError::Overflow)?;
            pos += len;
        }

        Ok(segs)
    }

    pub fn total_bytes_written(&self) -> u64 { self.bytes_written.load(Ordering::Relaxed) }
    pub fn total_write_ops(&self)     -> u64 { self.write_ops.load(Ordering::Relaxed) }
    pub fn error_count(&self)         -> u64 { self.errors.load(Ordering::Relaxed) }
}

impl Default for ExtentWriter { fn default() -> Self { Self::new() } }

// ─────────────────────────────────────────────────────────────────────────────
// ExtentMap — table des extents alloués pour un objet
// ─────────────────────────────────────────────────────────────────────────────

pub struct ExtentMap {
    extents: Vec<Extent>,
}

impl ExtentMap {
    pub fn new() -> Self { Self { extents: Vec::new() } }

    pub fn add(&mut self, extent: Extent) -> ExofsResult<()> {
        self.extents.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.extents.push(extent);
        Ok(())
    }

    pub fn total_bytes(&self) -> u64 {
        self.extents.iter().fold(0u64, |a, e| a.saturating_add(e.size))
    }

    pub fn count(&self)  -> usize { self.extents.len() }
    pub fn extents(&self) -> &[Extent] { &self.extents }

    pub fn find_extent(&self, off: DiskOffset) -> Option<&Extent> {
        self.extents.iter().find(|e| e.contains_offset(off))
    }

    pub fn is_empty(&self) -> bool { self.extents.is_empty() }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_write(data: &[u8], _off: DiskOffset) -> ExofsResult<usize> { Ok(data.len()) }

    fn mock_write_short(_data: &[u8], _off: DiskOffset) -> ExofsResult<usize> { Ok(0) }

    #[test]
    fn test_write_basic() {
        let w   = ExtentWriter::new();
        let buf = vec![0xABu8; 4096];
        let r   = w.write(DiskOffset(0), &buf, &mock_write).unwrap();
        assert_eq!(r.bytes_written, 4096);
        assert_eq!(r.extent.size, 4096);
    }

    #[test]
    fn test_write_short_detects() {
        let w   = ExtentWriter::new();
        let buf = vec![0u8; 4096];
        assert!(w.write(DiskOffset(0), &buf, &mock_write_short).is_err());
        assert_eq!(w.error_count(), 1);
    }

    #[test]
    fn test_write_blocks_multi() {
        let w   = ExtentWriter::new();
        let buf = vec![0x42u8; 12288]; // 3 blocs
        let r   = w.write_blocks(DiskOffset(0), &buf, &mock_write).unwrap();
        assert_eq!(r.bytes_written, 12288);
        assert_eq!(r.segments, 3);
    }

    #[test]
    fn test_split_into_segments() {
        let data = vec![1u8; 5000]; // > 1 bloc
        let segs = ExtentWriter::split_into_segments(DiskOffset(0), &data).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].data.len(), 4096);
    }

    #[test]
    fn test_extent_map() {
        let mut m = ExtentMap::new();
        m.add(Extent::new(DiskOffset(0), 4096)).unwrap();
        m.add(Extent::new(DiskOffset(4096), 8192)).unwrap();
        assert_eq!(m.total_bytes(), 12288);
        assert!(m.find_extent(DiskOffset(4096)).is_some());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MultiExtentWriter — écrit des données sur plusieurs extents discontinus
// ─────────────────────────────────────────────────────────────────────────────

pub struct MultiExtentWriteResult {
    pub extents:       Vec<Extent>,
    pub total_written: u64,
    pub segments:      u64,
}

impl MultiExtentWriteResult {
    pub fn all_extents_size(&self) -> u64 {
        self.extents.iter().fold(0u64, |a, e| a.saturating_add(e.size))
    }
}

impl ExtentWriter {
    /// Écrit `data` sur une liste d'extents alloués (dispersé).
    ///
    /// `extents` = liste des extents disque déjà alloués, ordonnés.
    /// `data` est découpé séquentiellement selon les tailles des extents.
    pub fn write_scattered(
        &self,
        extents:  &[Extent],
        data:     &[u8],
        write_fn: &dyn Fn(&[u8], DiskOffset) -> ExofsResult<usize>,
    ) -> ExofsResult<MultiExtentWriteResult> {
        if extents.is_empty() || data.is_empty() {
            return Err(ExofsError::InvalidArgument);
        }

        let total_cap: u64 = extents.iter().fold(0u64, |a, e| a.saturating_add(e.size));
        if (data.len() as u64) > total_cap {
            return Err(ExofsError::InvalidSize);
        }

        let mut pos         = 0usize;
        let mut seg_count   = 0u64;
        let mut total_write = 0u64;
        let mut result_exts: Vec<Extent> = Vec::new();
        result_exts.try_reserve(extents.len()).map_err(|_| ExofsError::NoMemory)?;

        for ext in extents {
            if pos >= data.len() { break; }
            let chunk_len = (ext.size as usize).min(data.len() - pos);
            let chunk     = &data[pos..pos + chunk_len];

            let n = write_fn(chunk, ext.offset)?;
            if n != chunk_len {
                self.errors.fetch_add(1, Ordering::Relaxed);
                return Err(ExofsError::ShortWrite);
            }

            result_exts.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            result_exts.push(Extent::new(ext.offset, n as u64));

            total_write = total_write.checked_add(n as u64).ok_or(ExofsError::Overflow)?;
            pos        += n;
            seg_count   = seg_count.saturating_add(1);
        }

        self.bytes_written.fetch_add(total_write, Ordering::Relaxed);
        self.write_ops.fetch_add(seg_count, Ordering::Relaxed);
        STORAGE_STATS.add_write(total_write);

        Ok(MultiExtentWriteResult { extents: result_exts, total_written: total_write, segments: seg_count })
    }

    /// Écrit un bloc de zéros (zeroing d'un extent).
    pub fn zero_extent(
        &self,
        extent:   &Extent,
        write_fn: &dyn Fn(&[u8], DiskOffset) -> ExofsResult<usize>,
    ) -> ExofsResult<u64> {
        let blk = BLOCK_SIZE as usize;
        let zero_blk: Vec<u8> = vec![0u8; blk];
        let mut remaining = extent.size;
        let mut disk      = extent.offset.0;
        let mut total     = 0u64;

        while remaining > 0 {
            let to_write = (remaining as usize).min(blk);
            let n        = write_fn(&zero_blk[..to_write], DiskOffset(disk))?;
            if n != to_write { return Err(ExofsError::ShortWrite); }
            disk      = disk.checked_add(n as u64).ok_or(ExofsError::Overflow)?;
            remaining = remaining.saturating_sub(n as u64);
            total     = total.saturating_add(n as u64);
        }

        STORAGE_STATS.add_write(total);
        Ok(total)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ExtentWriterStats — snapshot des compteurs
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Copy)]
pub struct ExtentWriterStats {
    pub bytes_written: u64,
    pub write_ops:     u64,
    pub errors:        u64,
}

impl ExtentWriter {
    pub fn stats(&self) -> ExtentWriterStats {
        ExtentWriterStats {
            bytes_written: self.total_bytes_written(),
            write_ops:     self.total_write_ops(),
            errors:        self.error_count(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests supplémentaires
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_extra {
    use super::*;

    fn mock_write(data: &[u8], _: DiskOffset) -> ExofsResult<usize> { Ok(data.len()) }

    #[test]
    fn test_write_scattered() {
        let w  = ExtentWriter::new();
        let exts = vec![Extent::new(DiskOffset(0), 4096), Extent::new(DiskOffset(8192), 4096)];
        let data = vec![0x11u8; 8192];
        let r    = w.write_scattered(&exts, &data, &mock_write).unwrap();
        assert_eq!(r.total_written, 8192);
        assert_eq!(r.segments, 2);
    }

    #[test]
    fn test_zero_extent() {
        let w    = ExtentWriter::new();
        let ext  = Extent::new(DiskOffset(0), 8192);
        let n    = w.zero_extent(&ext, &mock_write).unwrap();
        assert_eq!(n, 8192);
    }

    #[test]
    fn test_extent_overlaps() {
        let a = Extent::new(DiskOffset(0), 8192);
        let b = Extent::new(DiskOffset(4096), 4096);
        assert!(a.overlaps(&b));
        let c = Extent::new(DiskOffset(8192), 4096);
        assert!(!a.overlaps(&c));
    }

    #[test]
    fn test_extent_block_aligned() {
        let e1 = Extent::new(DiskOffset(4096), 8192);
        assert!(e1.is_block_aligned());
        let e2 = Extent::new(DiskOffset(1000), 4096);
        assert!(!e2.is_block_aligned());
    }

    #[test]
    fn test_stats_snapshot() {
        let w   = ExtentWriter::new();
        let buf = vec![0u8; 1024];
        w.write(DiskOffset(0), &buf, &mock_write).unwrap();
        let s = w.stats();
        assert_eq!(s.bytes_written, 1024);
        assert_eq!(s.errors, 0);
    }
}
