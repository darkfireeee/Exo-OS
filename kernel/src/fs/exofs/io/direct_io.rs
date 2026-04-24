//! direct_io.rs — IO directs (bypass cache) alignés sur 512 / 4096 (no_std).
//!
//! Ce module fournit :
//!  - `DirectIoConfig`   : configuration des IO directs.
//!  - `DirectIoBuffer`   : buffer aligné sur la taille de bloc.
//!  - `DirectIo`         : API de lecture/écriture directe (DMA-style).
//!  - `AlignedBlock`     : bloc aligné 512 octets.
//!  - `DirectIoStats`    : statistiques des opérations directes.
//!
//! RECUR-01 : boucles while — aucune récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add/mul.

extern crate alloc;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─── Taille de bloc valide ────────────────────────────────────────────────────

/// Tailles de bloc autorisées pour les IO directs.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u32)]
pub enum BlockSize {
    B512 = 512,
    B4096 = 4096,
}

impl BlockSize {
    pub fn as_u32(self) -> u32 {
        self as u32
    }
    pub fn as_usize(self) -> usize {
        self as u32 as usize
    }

    pub fn from_u32(v: u32) -> ExofsResult<Self> {
        match v {
            512 => Ok(BlockSize::B512),
            4096 => Ok(BlockSize::B4096),
            _ => Err(ExofsError::InvalidArgument),
        }
    }

    /// Vérifie qu'un offset est aligné sur cette taille de bloc (ARITH-02).
    pub fn is_aligned(self, offset: u64) -> bool {
        offset % (self as u64) == 0
    }

    /// Retourne le nombre de blocs nécessaires pour `bytes` octets (ARITH-02).
    pub fn blocks_for(self, bytes: u64) -> u64 {
        bytes
            .checked_add(self as u64 - 1)
            .map(|v| v / self as u64)
            .unwrap_or(u64::MAX)
    }
}

// ─── Configuration des IO directs ─────────────────────────────────────────────

/// Configuration d'une session de Direct IO.
#[derive(Clone, Copy, Debug)]
pub struct DirectIoConfig {
    pub block_size: BlockSize,
    pub bypass_cache: bool,
    pub verify_write: bool,
    pub max_blocks_per_op: u32,
}

impl DirectIoConfig {
    pub fn default_512() -> Self {
        Self {
            block_size: BlockSize::B512,
            bypass_cache: true,
            verify_write: false,
            max_blocks_per_op: 128,
        }
    }

    pub fn default_4k() -> Self {
        Self {
            block_size: BlockSize::B4096,
            bypass_cache: true,
            verify_write: false,
            max_blocks_per_op: 64,
        }
    }

    pub fn validate(&self) -> ExofsResult<()> {
        if self.max_blocks_per_op == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }
}

// ─── Bloc aligné 512 ─────────────────────────────────────────────────────────

/// Un bloc de 512 octets représentant un secteur physique.
#[derive(Clone, Copy)]
#[repr(C, align(512))]
pub struct AlignedBlock512 {
    pub data: [u8; 512],
}

impl AlignedBlock512 {
    pub fn new() -> Self {
        Self { data: [0u8; 512] }
    }
    pub fn fill(&mut self, byte: u8) {
        let mut i = 0usize;
        while i < 512 {
            self.data[i] = byte;
            i = i.wrapping_add(1);
        }
    }
}

// ─── DirectIoBuffer ───────────────────────────────────────────────────────────

/// Buffer de Direct IO contenant un vecteur de blocs alignés.
pub struct DirectIoBuffer {
    blocks: Vec<AlignedBlock512>,
    block_size: BlockSize,
    n_blocks: u32,
}

impl DirectIoBuffer {
    /// Crée un buffer de `n_blocks` blocs (OOM-02).
    pub fn new(n_blocks: u32, block_size: BlockSize) -> ExofsResult<Self> {
        if n_blocks == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        let mut blocks: Vec<AlignedBlock512> = Vec::new();
        blocks
            .try_reserve(n_blocks as usize)
            .map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0u32;
        while i < n_blocks {
            blocks.push(AlignedBlock512::new());
            i = i.saturating_add(1);
        }
        Ok(Self {
            blocks,
            block_size,
            n_blocks,
        })
    }

    pub fn n_blocks(&self) -> u32 {
        self.n_blocks
    }
    pub fn block_size(&self) -> BlockSize {
        self.block_size
    }

    pub fn total_bytes(&self) -> u64 {
        (self.n_blocks as u64).saturating_mul(self.block_size.as_u32() as u64)
    }

    /// Retourne le bloc `idx` (ARITH-02).
    pub fn block(&self, idx: u32) -> ExofsResult<&AlignedBlock512> {
        if idx >= self.n_blocks {
            return Err(ExofsError::OffsetOverflow);
        }
        Ok(&self.blocks[idx as usize])
    }

    pub fn block_mut(&mut self, idx: u32) -> ExofsResult<&mut AlignedBlock512> {
        if idx >= self.n_blocks {
            return Err(ExofsError::OffsetOverflow);
        }
        Ok(&mut self.blocks[idx as usize])
    }

    /// Copie les données d'un slice dans les blocs (RECUR-01 : while).
    pub fn write_from_slice(&mut self, data: &[u8]) -> ExofsResult<()> {
        let max_bytes = self.total_bytes() as usize;
        if data.len() > max_bytes {
            return Err(ExofsError::InvalidArgument);
        }
        let bs = self.block_size.as_usize();
        let mut written = 0usize;
        let mut blk_idx = 0u32;
        while written < data.len() {
            let left = data.len().saturating_sub(written);
            let n = left.min(bs);
            let blk = self.block_mut(blk_idx)?;
            blk.data[..n].copy_from_slice(&data[written..written.wrapping_add(n)]);
            written = written.wrapping_add(n);
            blk_idx = blk_idx.saturating_add(1);
        }
        Ok(())
    }

    /// Lit les données des blocs dans un slice (RECUR-01 : while).
    pub fn read_to_slice(&self, out: &mut [u8]) -> ExofsResult<usize> {
        let max_bytes = self.total_bytes() as usize;
        let n = out.len().min(max_bytes);
        let bs = self.block_size.as_usize();
        let mut read = 0usize;
        let mut blk_idx = 0u32;
        while read < n {
            let left = n.saturating_sub(read);
            let chunk = left.min(bs);
            let blk = self.block(blk_idx)?;
            out[read..read.wrapping_add(chunk)].copy_from_slice(&blk.data[..chunk]);
            read = read.wrapping_add(chunk);
            blk_idx = blk_idx.saturating_add(1);
        }
        Ok(n)
    }
}

// ─── Statistiques Direct IO ───────────────────────────────────────────────────

/// Statistiques des opérations de Direct IO.
#[derive(Clone, Copy, Debug, Default)]
pub struct DirectIoStats {
    pub reads_ok: u64,
    pub reads_err: u64,
    pub writes_ok: u64,
    pub writes_err: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
}

impl DirectIoStats {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn is_clean(&self) -> bool {
        self.reads_err == 0 && self.writes_err == 0
    }
    pub fn total_ops(&self) -> u64 {
        self.reads_ok
            .saturating_add(self.writes_ok)
            .saturating_add(self.reads_err)
            .saturating_add(self.writes_err)
    }
}

// ─── DirectIo ─────────────────────────────────────────────────────────────────

/// Moteur de Direct IO (sans cache).
pub struct DirectIo {
    config: DirectIoConfig,
    stats: DirectIoStats,
}

impl DirectIo {
    pub fn new(config: DirectIoConfig) -> ExofsResult<Self> {
        config.validate()?;
        Ok(Self {
            config,
            stats: DirectIoStats::new(),
        })
    }

    pub fn default_512() -> Self {
        Self::new(DirectIoConfig::default_512())
            .expect("DirectIoConfig::default_512() toujours valide")
    }

    /// Valide un accès LBA → vérifier l'alignement et la limite.
    fn validate_access(&self, lba: u64, n_blocks: u32) -> ExofsResult<()> {
        if n_blocks == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        if n_blocks > self.config.max_blocks_per_op {
            return Err(ExofsError::InvalidArgument);
        }
        // Vérifier absence de dépassement arithmétique (ARITH-02)
        let _ = lba
            .checked_add(n_blocks as u64)
            .ok_or(ExofsError::OffsetOverflow)?;
        Ok(())
    }

    /// Lecture directe depuis une source mémoire (simulate DMA read).
    ///
    /// `device_data` : vue flat du "disque" (slice contiguë en RAM).
    pub fn read_aligned(
        &mut self,
        device_data: &[u8],
        lba: u64,
        buf: &mut DirectIoBuffer,
    ) -> ExofsResult<u32> {
        self.validate_access(lba, buf.n_blocks())?;
        let bs = self.config.block_size.as_usize();
        let byte_offset = (lba as usize).saturating_mul(bs);
        let byte_len = (buf.n_blocks() as usize).saturating_mul(bs);

        let end = byte_offset
            .checked_add(byte_len)
            .ok_or(ExofsError::OffsetOverflow)?;
        if end > device_data.len() {
            self.stats.reads_err = self.stats.reads_err.saturating_add(1);
            return Err(ExofsError::IoError);
        }

        let src = &device_data[byte_offset..end];
        let mut blk = 0u32;
        while blk < buf.n_blocks() {
            let off = (blk as usize).saturating_mul(bs);
            let b = buf.block_mut(blk)?;
            b.data[..bs.min(512)].copy_from_slice(&src[off..off.wrapping_add(bs.min(512))]);
            blk = blk.saturating_add(1);
        }

        self.stats.reads_ok = self.stats.reads_ok.saturating_add(1);
        self.stats.bytes_read = self.stats.bytes_read.saturating_add(byte_len as u64);
        Ok(buf.n_blocks())
    }

    /// Écriture directe vers une destination mémoire (simulate DMA write).
    pub fn write_aligned(
        &mut self,
        device_data: &mut [u8],
        lba: u64,
        buf: &DirectIoBuffer,
    ) -> ExofsResult<u32> {
        self.validate_access(lba, buf.n_blocks())?;
        let bs = self.config.block_size.as_usize();
        let byte_offset = (lba as usize).saturating_mul(bs);
        let byte_len = (buf.n_blocks() as usize).saturating_mul(bs);

        let end = byte_offset
            .checked_add(byte_len)
            .ok_or(ExofsError::OffsetOverflow)?;
        if end > device_data.len() {
            self.stats.writes_err = self.stats.writes_err.saturating_add(1);
            return Err(ExofsError::IoError);
        }

        let dst = &mut device_data[byte_offset..end];
        let mut blk = 0u32;
        while blk < buf.n_blocks() {
            let off = (blk as usize).saturating_mul(bs);
            let b = buf.block(blk)?;
            dst[off..off.wrapping_add(bs.min(512))].copy_from_slice(&b.data[..bs.min(512)]);
            blk = blk.saturating_add(1);
        }

        self.stats.writes_ok = self.stats.writes_ok.saturating_add(1);
        self.stats.bytes_written = self.stats.bytes_written.saturating_add(byte_len as u64);
        Ok(buf.n_blocks())
    }

    pub fn stats(&self) -> &DirectIoStats {
        &self.stats
    }
    pub fn reset_stats(&mut self) {
        self.stats = DirectIoStats::new();
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_size_from_u32() {
        assert_eq!(BlockSize::from_u32(512).expect("ok"), BlockSize::B512);
        assert_eq!(BlockSize::from_u32(4096).expect("ok"), BlockSize::B4096);
        assert!(BlockSize::from_u32(1024).is_err());
    }

    #[test]
    fn test_block_size_is_aligned() {
        assert!(BlockSize::B512.is_aligned(1024));
        assert!(!BlockSize::B512.is_aligned(100));
        assert!(BlockSize::B4096.is_aligned(8192));
        assert!(!BlockSize::B4096.is_aligned(512));
    }

    #[test]
    fn test_blocks_for() {
        assert_eq!(BlockSize::B512.blocks_for(512), 1);
        assert_eq!(BlockSize::B512.blocks_for(513), 2);
        assert_eq!(BlockSize::B4096.blocks_for(8192), 2);
    }

    #[test]
    fn test_direct_io_buffer_write_read() {
        let mut buf = DirectIoBuffer::new(2, BlockSize::B512).expect("ok");
        let _data = b"hello_direct_io_test_data_here____________padding_to_1024__";
        // on écrit exactement 512 bytes
        let mut src = [0x55u8; 512];
        src[..5].copy_from_slice(b"hello");
        buf.write_from_slice(&src).expect("ok");
        let mut out = [0u8; 512];
        buf.read_to_slice(&mut out).expect("ok");
        assert_eq!(&out[..5], b"hello");
    }

    #[test]
    fn test_direct_io_read_aligned() {
        let mut device = [0u8; 4096];
        device[512..517].copy_from_slice(b"block");
        let mut dio = DirectIo::default_512();
        let mut buf = DirectIoBuffer::new(1, BlockSize::B512).expect("ok");
        dio.read_aligned(&device, 1, &mut buf).expect("ok");
        let blk = buf.block(0).expect("ok");
        assert_eq!(&blk.data[..5], b"block");
    }

    #[test]
    fn test_direct_io_write_aligned() {
        let mut device = [0u8; 4096];
        let mut dio = DirectIo::default_512();
        let mut buf = DirectIoBuffer::new(1, BlockSize::B512).expect("ok");
        buf.block_mut(0).expect("ok").data[..5].copy_from_slice(b"write");
        dio.write_aligned(&mut device, 0, &buf).expect("ok");
        assert_eq!(&device[..5], b"write");
    }

    #[test]
    fn test_direct_io_out_of_bounds() {
        let device = [0u8; 512];
        let mut dio = DirectIo::default_512();
        let mut buf = DirectIoBuffer::new(2, BlockSize::B512).expect("ok"); // 2 blocs = 1024 bytes
        assert!(dio.read_aligned(&device, 0, &mut buf).is_err());
        assert_eq!(dio.stats().reads_err, 1);
    }

    #[test]
    fn test_direct_io_stats() {
        let mut device = [0u8; 4096];
        let mut dio = DirectIo::default_512();
        let mut buf = DirectIoBuffer::new(1, BlockSize::B512).expect("ok");
        dio.read_aligned(&device, 0, &mut buf).expect("ok");
        dio.write_aligned(&mut device, 1, &buf).expect("ok");
        assert_eq!(dio.stats().reads_ok, 1);
        assert_eq!(dio.stats().writes_ok, 1);
        assert!(dio.stats().is_clean());
    }

    #[test]
    fn test_reset_stats() {
        let device = [0u8; 4096];
        let mut dio = DirectIo::default_512();
        let mut buf = DirectIoBuffer::new(1, BlockSize::B512).expect("ok");
        dio.read_aligned(&device, 0, &mut buf).expect("ok");
        dio.reset_stats();
        assert_eq!(dio.stats().reads_ok, 0);
    }

    #[test]
    fn test_buffer_zero_blocks() {
        assert!(DirectIoBuffer::new(0, BlockSize::B512).is_err());
    }

    #[test]
    fn test_aligned_block_fill() {
        let mut blk = AlignedBlock512::new();
        blk.fill(0xCC);
        let mut i = 0;
        while i < 512 {
            assert_eq!(blk.data[i], 0xCC);
            i += 1;
        }
    }

    #[test]
    fn test_max_blocks_per_op() {
        let cfg = DirectIoConfig {
            max_blocks_per_op: 2,
            ..DirectIoConfig::default_512()
        };
        let mut dio = DirectIo::new(cfg).expect("ok");
        let device = [0u8; 4096];
        let mut buf = DirectIoBuffer::new(4, BlockSize::B512).expect("ok"); // 4 > max
        assert!(dio.read_aligned(&device, 0, &mut buf).is_err());
    }

    #[test]
    fn test_buffer_total_bytes() {
        let buf = DirectIoBuffer::new(4, BlockSize::B512).expect("ok");
        assert_eq!(buf.total_bytes(), 2048);
    }

    #[test]
    fn test_block_index_out_of_range() {
        let buf = DirectIoBuffer::new(2, BlockSize::B512).expect("ok");
        assert!(buf.block(2).is_err());
        assert!(buf.block(10).is_err());
    }

    #[test]
    fn test_config_validate() {
        let mut cfg = DirectIoConfig::default_512();
        assert!(cfg.validate().is_ok());
        cfg.max_blocks_per_op = 0;
        assert!(cfg.validate().is_err());
    }
}
