// kernel/src/fs/exofs/storage/extent_reader.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Lecture d'extensions (extents) disque — ExoFS
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// ExtentReader lit un extent (plage de blocs contigus) depuis le disque,
// optionnellement via le cache de blocs.
//
// Règles ExoFS :
// - HDR-03   : vérifier l'étendue avant lecture.
// - ARITH-02 : checked_add pour les offsets.
// - OOM-02   : try_reserve avant allocations.

use crate::fs::exofs::core::{DiskOffset, ExofsError, ExofsResult};
use crate::fs::exofs::storage::block_cache::BlockCache;
use crate::fs::exofs::storage::extent_writer::Extent;
use crate::fs::exofs::storage::layout::BLOCK_SIZE;
use crate::fs::exofs::storage::storage_stats::STORAGE_STATS;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// ExtentReadResult
// ─────────────────────────────────────────────────────────────────────────────

pub struct ExtentReadResult {
    pub data: Vec<u8>,
    pub extent: Extent,
    pub bytes_read: u64,
    pub cached: bool,
    pub segments: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// ExtentReader
// ─────────────────────────────────────────────────────────────────────────────

pub struct ExtentReader {
    bytes_read: AtomicU64,
    read_ops: AtomicU64,
    errors: AtomicU64,
    cache_ops: AtomicU64,
}

impl ExtentReader {
    pub fn new() -> Self {
        Self {
            bytes_read: AtomicU64::new(0),
            read_ops: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            cache_ops: AtomicU64::new(0),
        }
    }

    /// Lit `size` octets depuis `offset`.
    pub fn read(
        &self,
        offset: DiskOffset,
        size: u64,
        read_fn: &dyn Fn(DiskOffset, &mut [u8]) -> ExofsResult<usize>,
    ) -> ExofsResult<ExtentReadResult> {
        if size == 0 {
            return Err(ExofsError::InvalidArgument);
        }

        let sz = size as usize;
        let mut buf: Vec<u8> = Vec::new();
        buf.try_reserve(sz).map_err(|_| ExofsError::NoMemory)?;
        buf.resize(sz, 0u8);

        let n = read_fn(offset, &mut buf)?;
        buf.truncate(n);

        self.bytes_read.fetch_add(n as u64, Ordering::Relaxed);
        self.read_ops.fetch_add(1, Ordering::Relaxed);
        STORAGE_STATS.add_read(n as u64);

        let extent = Extent::new(offset, n as u64);
        Ok(ExtentReadResult {
            data: buf,
            extent,
            bytes_read: n as u64,
            cached: false,
            segments: 1,
        })
    }

    /// Lit un extent en découpant par blocs.
    pub fn read_blocks(
        &self,
        offset: DiskOffset,
        size: u64,
        read_fn: &dyn Fn(DiskOffset, &mut [u8]) -> ExofsResult<usize>,
    ) -> ExofsResult<ExtentReadResult> {
        if size == 0 {
            return Err(ExofsError::InvalidArgument);
        }

        let blk = BLOCK_SIZE as usize;
        let total_sz = size as usize;
        let mut out: Vec<u8> = Vec::new();
        out.try_reserve(total_sz)
            .map_err(|_| ExofsError::NoMemory)?;

        let mut remaining = total_sz;
        let mut disk = offset.0;
        let mut seg_count = 0u64;

        while remaining > 0 {
            let to_read = remaining.min(blk);
            let mut blk_buf: Vec<u8> = Vec::new();
            blk_buf
                .try_reserve(to_read)
                .map_err(|_| ExofsError::NoMemory)?;
            blk_buf.resize(to_read, 0u8);

            let n = read_fn(DiskOffset(disk), &mut blk_buf)?;
            blk_buf.truncate(n);
            out.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
            out.extend_from_slice(&blk_buf);

            disk = disk.checked_add(n as u64).ok_or(ExofsError::Overflow)?;
            remaining = remaining.saturating_sub(n);
            seg_count = seg_count.saturating_add(1);

            if n == 0 {
                break;
            } // EOF prématuré.
        }

        let bytes_read = out.len() as u64;
        self.bytes_read.fetch_add(bytes_read, Ordering::Relaxed);
        self.read_ops.fetch_add(seg_count, Ordering::Relaxed);
        STORAGE_STATS.add_read(bytes_read);

        let extent = Extent::new(offset, bytes_read);
        Ok(ExtentReadResult {
            data: out,
            extent,
            bytes_read,
            cached: false,
            segments: seg_count,
        })
    }

    /// Lit un bloc via le cache.
    pub fn read_cached(
        &self,
        offset: DiskOffset,
        cache: &BlockCache,
        read_fn: &dyn Fn(DiskOffset, &mut [u8]) -> ExofsResult<usize>,
    ) -> ExofsResult<ExtentReadResult> {
        let blk = BLOCK_SIZE as usize;
        let mut buf: Vec<u8> = Vec::new();
        buf.try_reserve(blk).map_err(|_| ExofsError::NoMemory)?;
        buf.resize(blk, 0u8);

        let n = cache.read_block(offset, &mut buf, read_fn)?;
        buf.truncate(n);

        self.cache_ops.fetch_add(1, Ordering::Relaxed);
        let extent = Extent::new(offset, n as u64);
        Ok(ExtentReadResult {
            data: buf,
            extent,
            bytes_read: n as u64,
            cached: true,
            segments: 1,
        })
    }

    /// Lit un extent multi-blocs via le cache (bloc par bloc).
    pub fn read_cached_blocks(
        &self,
        offset: DiskOffset,
        n_blocks: u64,
        cache: &BlockCache,
        read_fn: &dyn Fn(DiskOffset, &mut [u8]) -> ExofsResult<usize>,
    ) -> ExofsResult<ExtentReadResult> {
        let blk = BLOCK_SIZE as u64;
        let mut out: Vec<u8> = Vec::new();
        let cap = n_blocks as usize * BLOCK_SIZE as usize;
        out.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;

        let mut disk = offset.0;
        let mut seg_count = 0u64;

        for _ in 0..n_blocks {
            let blk_off = DiskOffset(disk);
            let mut buf_blk: Vec<u8> = Vec::new();
            buf_blk
                .try_reserve(BLOCK_SIZE as usize)
                .map_err(|_| ExofsError::NoMemory)?;
            buf_blk.resize(BLOCK_SIZE as usize, 0u8);

            let n = cache.read_block(blk_off, &mut buf_blk, read_fn)?;
            out.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
            out.extend_from_slice(&buf_blk[..n]);

            disk = disk.checked_add(blk).ok_or(ExofsError::Overflow)?;
            seg_count = seg_count.saturating_add(1);
        }

        let bytes_read = out.len() as u64;
        self.cache_ops.fetch_add(seg_count, Ordering::Relaxed);

        let extent = Extent::new(offset, bytes_read);
        Ok(ExtentReadResult {
            data: out,
            extent,
            bytes_read,
            cached: true,
            segments: seg_count,
        })
    }

    pub fn total_bytes_read(&self) -> u64 {
        self.bytes_read.load(Ordering::Relaxed)
    }
    pub fn total_read_ops(&self) -> u64 {
        self.read_ops.load(Ordering::Relaxed)
    }
    pub fn cache_ops(&self) -> u64 {
        self.cache_ops.load(Ordering::Relaxed)
    }
    pub fn error_count(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }
}

impl Default for ExtentReader {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// scatter_read — lecture dispersée sur plusieurs extents
// ─────────────────────────────────────────────────────────────────────────────

pub struct ScatterReadItem {
    pub extent_index: usize,
    pub data: Vec<u8>,
    pub ok: bool,
}

pub fn scatter_read(
    extents: &[Extent],
    reader: &ExtentReader,
    read_fn: &dyn Fn(DiskOffset, &mut [u8]) -> ExofsResult<usize>,
) -> ExofsResult<Vec<ScatterReadItem>> {
    let mut out: Vec<ScatterReadItem> = Vec::new();
    out.try_reserve(extents.len())
        .map_err(|_| ExofsError::NoMemory)?;

    for (i, extent) in extents.iter().enumerate() {
        match reader.read(extent.offset, extent.size, read_fn) {
            Ok(r) => {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(ScatterReadItem {
                    extent_index: i,
                    data: r.data,
                    ok: true,
                });
            }
            Err(_) => {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(ScatterReadItem {
                    extent_index: i,
                    data: Vec::new(),
                    ok: false,
                });
            }
        }
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_read(_off: DiskOffset, buf: &mut [u8]) -> ExofsResult<usize> {
        buf.fill(0xCC);
        Ok(buf.len())
    }

    fn mock_read_eof(_off: DiskOffset, _buf: &mut [u8]) -> ExofsResult<usize> {
        Ok(0)
    }

    #[test]
    fn test_read_basic() {
        let r = ExtentReader::new();
        let result = r.read(DiskOffset(0), 4096, &mock_read).unwrap();
        assert_eq!(result.bytes_read, 4096);
        assert_eq!(result.data[0], 0xCC);
    }

    #[test]
    fn test_read_blocks_multi() {
        let r = ExtentReader::new();
        let result = r.read_blocks(DiskOffset(0), 12288, &mock_read).unwrap();
        assert_eq!(result.bytes_read, 12288);
        assert_eq!(result.segments, 3);
    }

    #[test]
    fn test_read_eof_terminates() {
        let r = ExtentReader::new();
        let result = r.read_blocks(DiskOffset(0), 4096, &mock_read_eof).unwrap();
        assert_eq!(result.bytes_read, 0);
    }

    #[test]
    fn test_scatter_read() {
        let extents = vec![
            Extent::new(DiskOffset(0), 512),
            Extent::new(DiskOffset(4096), 512),
        ];
        let r = ExtentReader::new();
        let items = scatter_read(&extents, &r, &mock_read).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| i.ok));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GatherReadItem — lecture sur liste d'extents vers un seul buffer
// ─────────────────────────────────────────────────────────────────────────────

impl ExtentReader {
    /// Lit une liste d'extents et concatène les données dans un seul Vec.
    pub fn gather_read(
        &self,
        extents: &[Extent],
        read_fn: &dyn Fn(DiskOffset, &mut [u8]) -> ExofsResult<usize>,
    ) -> ExofsResult<Vec<u8>> {
        let total_cap: u64 = extents.iter().fold(0u64, |a, e| a.saturating_add(e.size));
        let mut out: Vec<u8> = Vec::new();
        out.try_reserve(total_cap as usize)
            .map_err(|_| ExofsError::NoMemory)?;

        for ext in extents {
            let r = self.read(ext.offset, ext.size, read_fn)?;
            out.try_reserve(r.data.len())
                .map_err(|_| ExofsError::NoMemory)?;
            out.extend_from_slice(&r.data);
        }
        Ok(out)
    }

    /// Lit une portion d'un extent (read partiel).
    pub fn read_partial(
        &self,
        extent: &Extent,
        rel_off: u64,
        len: u64,
        read_fn: &dyn Fn(DiskOffset, &mut [u8]) -> ExofsResult<usize>,
    ) -> ExofsResult<ExtentReadResult> {
        if rel_off.checked_add(len).ok_or(ExofsError::Overflow)? > extent.size {
            return Err(ExofsError::InvalidArgument);
        }
        let abs_off = DiskOffset(
            extent
                .offset
                .0
                .checked_add(rel_off)
                .ok_or(ExofsError::Overflow)?,
        );
        self.read(abs_off, len, read_fn)
    }

    pub fn stats_snapshot(&self) -> ExtentReaderStats {
        ExtentReaderStats {
            bytes_read: self.total_bytes_read(),
            read_ops: self.total_read_ops(),
            cache_ops: self.cache_ops(),
            errors: self.error_count(),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct ExtentReaderStats {
    pub bytes_read: u64,
    pub read_ops: u64,
    pub cache_ops: u64,
    pub errors: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// ExtentIterator — itère sur un extent bloc par bloc
// ─────────────────────────────────────────────────────────────────────────────

pub struct ExtentBlockIterator {
    current_offset: u64,
    end_offset: u64,
    block_size: u64,
}

impl ExtentBlockIterator {
    pub fn new(extent: &Extent) -> Self {
        let end = extent.offset.0.saturating_add(extent.size);
        Self {
            current_offset: extent.offset.0,
            end_offset: end,
            block_size: BLOCK_SIZE as u64,
        }
    }

    pub fn next_block(&mut self) -> Option<DiskOffset> {
        if self.current_offset >= self.end_offset {
            return None;
        }
        let off = self.current_offset;
        self.current_offset = self.current_offset.saturating_add(self.block_size);
        Some(DiskOffset(off))
    }

    pub fn remaining_blocks(&self) -> u64 {
        self.end_offset.saturating_sub(self.current_offset) / self.block_size
    }

    pub fn reset(&mut self, extent: &Extent) {
        self.current_offset = extent.offset.0;
        self.end_offset = extent.offset.0.saturating_add(extent.size);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests supplémentaires
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_extra {
    use super::*;

    fn mock_read(_: DiskOffset, buf: &mut [u8]) -> ExofsResult<usize> {
        buf.fill(0xDD);
        Ok(buf.len())
    }

    #[test]
    fn test_gather_read() {
        let extents = vec![
            Extent::new(DiskOffset(0), 512),
            Extent::new(DiskOffset(4096), 512),
        ];
        let r = ExtentReader::new();
        let data = r.gather_read(&extents, &mock_read).unwrap();
        assert_eq!(data.len(), 1024);
        assert!(data.iter().all(|&b| b == 0xDD));
    }

    #[test]
    fn test_read_partial() {
        let extent = Extent::new(DiskOffset(0), 8192);
        let r = ExtentReader::new();
        let result = r.read_partial(&extent, 0, 512, &mock_read).unwrap();
        assert_eq!(result.bytes_read, 512);
    }

    #[test]
    fn test_read_partial_out_of_bounds() {
        let extent = Extent::new(DiskOffset(0), 4096);
        let r = ExtentReader::new();
        assert!(r.read_partial(&extent, 4000, 200, &mock_read).is_err());
    }

    #[test]
    fn test_block_iterator() {
        let extent = Extent::new(DiskOffset(0), 12288);
        let mut it = ExtentBlockIterator::new(&extent);
        assert_eq!(it.remaining_blocks(), 3);
        let b0 = it.next_block().unwrap();
        assert_eq!(b0, DiskOffset(0));
        let b1 = it.next_block().unwrap();
        assert_eq!(b1, DiskOffset(4096));
        assert_eq!(it.remaining_blocks(), 1);
    }
}
