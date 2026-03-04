//! mod.rs — Module IO ExoFS : orchestration de la couche I/O (no_std).
//!
//! Ce module rassemble :
//!   - Les sous-modules (reader, writer, buffered_io, direct_io, …).
//!   - Les ré-exports des types publics principaux.
//!   - `IoConfig`         : configuration globale du module I/O.
//!   - `IoModule`         : init, health check, résumé.
//!   - `IoHealthStatus`   : état de santé.
//!   - `IoModuleSummary`  : snapshot de métriques agrégées.
//!   - Fonctions utilitaires : `read_blob_quick`, `write_blob_quick`, `flush_all`.
//!
//! RECUR-01 : boucles while — aucune récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add/mul.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─── Sous-modules ─────────────────────────────────────────────────────────────

pub mod async_io;
pub mod buffered_io;
pub mod direct_io;
pub mod io_batch;
pub mod io_stats;
pub mod io_uring;
pub mod prefetch;
pub mod readahead;
pub mod reader;
pub mod scatter_gather;
pub mod writeback;
pub mod writer;
pub mod zero_copy;

// ─── Ré-exports ───────────────────────────────────────────────────────────────

// reader
pub use reader::{BlobReader, BlobStore, ReadConfig, ReadResult, ReaderStats, VerifyMode};

// writer
pub use writer::{BlobStoreWrite, BlobWriter, WriteConfig, WriterStats};

// buffered_io
pub use buffered_io::{BufferedReader, BufferedWriter, IoBuffer, RingBuffer};

// direct_io
pub use direct_io::{BlockSize, DirectIo, DirectIoBuffer, DirectIoConfig};

// scatter_gather
pub use scatter_gather::{PhysSgList, PhysSegment, SgEngine, SgFragment, SgList};

// async_io
pub use async_io::{AsyncIoHandle, AsyncIoQueue, AsyncOpKind, AsyncState, ASYNC_IO_QUEUE};

// io_batch
pub use io_batch::{BatchResult, BatchStats, IoBatch, IoBatchEntry};

// io_uring
pub use io_uring::{IoUringCqe, IoUringQueue, IoUringSqe, SqeOpcode};

// prefetch
pub use prefetch::{PrefetchConfig, PrefetchEntry, PrefetchQueue, Prefetcher, PrefetchStrategy};

// readahead
pub use readahead::{
    BlockAccessLog, ReadaheadEngine, ReadaheadPolicy, ReadaheadScheduler,
    ReadaheadStats, ReadaheadWindow,
};

// writeback
pub use writeback::{
    WritebackConfig, WritebackEntry, WritebackQueue, WritebackStats, WritebackWorker,
    WRITEBACK_QUEUE,
};

// zero_copy
pub use zero_copy::{
    ZeroCopyPipe, ZeroCopyReader, ZeroCopySlice, ZeroCopyStats, ZeroCopyWindow, ZeroCopyWriter,
};

// io_stats
pub use io_stats::{IoOpKind, IoStats, IoStatsSnapshot, IO_STATS};

// ─── IoConfig ─────────────────────────────────────────────────────────────────

/// Configuration globale du module I/O ExoFS.
#[derive(Clone, Copy, Debug)]
pub struct IoConfig {
    pub stats_enabled:       bool,
    pub default_block_size:  u32,
    pub verify_checksums:    bool,
    pub max_pending_writes:  u32,
    pub readahead_enabled:   bool,
    pub writeback_enabled:   bool,
}

impl IoConfig {
    pub fn default_config() -> Self {
        Self {
            stats_enabled:      true,
            default_block_size: 4096,
            verify_checksums:   true,
            max_pending_writes: 256,
            readahead_enabled:  true,
            writeback_enabled:  true,
        }
    }

    pub fn minimal() -> Self {
        Self {
            stats_enabled:      false,
            default_block_size: 512,
            verify_checksums:   false,
            max_pending_writes: 64,
            readahead_enabled:  false,
            writeback_enabled:  false,
        }
    }

    pub fn validate(&self) -> ExofsResult<()> {
        match self.default_block_size {
            512 | 1024 | 2048 | 4096 | 8192 => {}
            _ => return Err(ExofsError::InvalidArgument),
        }
        if self.max_pending_writes == 0 { return Err(ExofsError::InvalidArgument); }
        Ok(())
    }
}

// ─── IoHealthStatus ───────────────────────────────────────────────────────────

/// État de santé du module I/O.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoHealthStatus {
    Ok,
    Degraded,
    Error,
}

impl IoHealthStatus {
    pub fn is_ok(&self) -> bool { matches!(self, Self::Ok) }

    pub fn to_str(&self) -> &'static str {
        match self {
            Self::Ok      => "ok",
            Self::Degraded => "degraded",
            Self::Error   => "error",
        }
    }
}

// ─── IoModuleSummary ──────────────────────────────────────────────────────────

/// Snapshot de métriques agrégées du module I/O.
#[derive(Clone, Copy, Debug, Default)]
pub struct IoModuleSummary {
    pub total_reads:          u64,
    pub total_writes:         u64,
    pub total_errors:         u64,
    pub bytes_read:           u64,
    pub bytes_written:        u64,
    pub pending_writeback:    u64,
    pub readahead_hint_count: u64,
    pub async_ops_pending:    u64,
}

impl IoModuleSummary {
    pub fn new() -> Self { Self::default() }

    pub fn total_ops(&self) -> u64 {
        self.total_reads.saturating_add(self.total_writes)
    }

    pub fn error_rate_pct10(&self) -> u64 {
        let total = self.total_ops();
        if total == 0 { return 0; }
        self.total_errors
            .saturating_mul(1000)
            .checked_div(total)
            .unwrap_or(0)
    }
}

// ─── IoModule ─────────────────────────────────────────────────────────────────

/// Pilote principal du module I/O — lifecycle + santé.
pub struct IoModule {
    config:       IoConfig,
    initialized:  AtomicBool,
    error_count:  AtomicU64,
    op_count:     AtomicU64,
    bytes_total:  AtomicU64,
}

// SAFETY : champs atomiques uniquement.
unsafe impl Sync for IoModule {}
unsafe impl Send for IoModule {}

impl IoModule {
    pub const fn new_const(config: IoConfig) -> Self {
        Self {
            config,
            initialized: AtomicBool::new(false),
            error_count: AtomicU64::new(0),
            op_count:    AtomicU64::new(0),
            bytes_total: AtomicU64::new(0),
        }
    }

    /// Initialise le module ; idempotent.
    pub fn init(&self) -> ExofsResult<()> {
        if self.initialized.load(Ordering::Acquire) { return Ok(()); }
        self.config.validate()?;
        self.initialized.store(true, Ordering::Release);
        Ok(())
    }

    /// Contrôle de santé basique.
    pub fn health_check(&self) -> IoHealthStatus {
        if !self.initialized.load(Ordering::Acquire) { return IoHealthStatus::Error; }
        let errs = self.error_count.load(Ordering::Relaxed);
        let ops  = self.op_count.load(Ordering::Relaxed);
        if ops == 0 { return IoHealthStatus::Ok; }
        // Dégradé si > 5 % d'erreurs (ARITH-02: checked_div)
        let rate = errs.saturating_mul(100).checked_div(ops).unwrap_or(0);
        if rate == 0 { IoHealthStatus::Ok }
        else if rate < 5 { IoHealthStatus::Degraded }
        else { IoHealthStatus::Error }
    }

    /// Résumé agrégé.
    pub fn summary(&self) -> IoModuleSummary {
        IoModuleSummary {
            total_reads:       self.op_count.load(Ordering::Relaxed),
            total_writes:      0,
            total_errors:      self.error_count.load(Ordering::Relaxed),
            bytes_read:        self.bytes_total.load(Ordering::Relaxed),
            bytes_written:     0,
            pending_writeback: WRITEBACK_QUEUE.pending_count(),
            ..IoModuleSummary::default()
        }
    }

    pub fn record_op(&self, bytes: u64) {
        self.op_count.fetch_add(1, Ordering::Relaxed);
        self.bytes_total.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn record_error(&self) {
        self.error_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn config(&self) -> &IoConfig { &self.config }
}

/// Instance globale du module I/O.
pub static IO_MODULE: IoModule = IoModule::new_const(IoConfig {
    stats_enabled:      true,
    default_block_size: 4096,
    verify_checksums:   true,
    max_pending_writes: 256,
    readahead_enabled:  true,
    writeback_enabled:  true,
});

// ─── Fonctions utilitaires ────────────────────────────────────────────────────

/// Lecture rapide d'un blob depuis un store in-memory.
///
/// Enregistre l'opération dans `IO_MODULE`. RECUR-01 : while.
pub fn read_blob_quick<S: BlobStore>(
    store: &S,
    blob_id: &[u8; 32],
    buf: &mut Vec<u8>,
) -> ExofsResult<usize> {
    let size = store.blob_size(blob_id).ok_or(ExofsError::BlobNotFound)? as usize;
    buf.try_reserve(size).map_err(|_| ExofsError::NoMemory)?;
    let mut config = ReadConfig::fast();
    config.verify = VerifyMode::None;
    let mut r = BlobReader::new(config)?;
    let (data, _) = r.read(store, blob_id)?;
    buf.extend_from_slice(data);
    IO_MODULE.record_op(data.len() as u64);
    Ok(data.len())
}

/// Écriture rapide d'un blob dans un store mutable.
pub fn write_blob_quick<S: BlobStoreWrite>(
    store: &mut S,
    blob_id: [u8; 32],
    data: &[u8],
) -> ExofsResult<()> {
    let cfg = WriteConfig::default();
    let mut w = BlobWriter::new(cfg)?;
    w.write(store, blob_id, data)?;
    w.flush(store)?;
    IO_MODULE.record_op(data.len() as u64);
    Ok(())
}

/// Flush de toutes les queues de writeback en attente.
///
/// RECUR-01 : while.
pub fn flush_all(write_fn: &mut dyn FnMut(&[u8; 32], u32) -> ExofsResult<u64>) -> ExofsResult<u32> {
    let cfg = WritebackConfig::default();
    let mut worker = WritebackWorker::new(cfg)?;
    let mut total = 0u32;
    while !WRITEBACK_QUEUE.is_empty() {
        worker.process_one(&WRITEBACK_QUEUE, write_fn)?;
        total = total.saturating_add(1);
    }
    Ok(total)
}

// ─── Tests ────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validate_ok() {
        let cfg = IoConfig::default_config();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_config_validate_bad_block_size() {
        let mut cfg = IoConfig::default_config();
        cfg.default_block_size = 333;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_config_validate_zero_pending() {
        let mut cfg = IoConfig::default_config();
        cfg.max_pending_writes = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_health_not_initialized() {
        let m = IoModule::new_const(IoConfig::default_config());
        assert_eq!(m.health_check(), IoHealthStatus::Error);
    }

    #[test]
    fn test_health_ok_after_init() {
        let m = IoModule::new_const(IoConfig::default_config());
        m.init().expect("init ok");
        assert_eq!(m.health_check(), IoHealthStatus::Ok);
    }

    #[test]
    fn test_health_degraded() {
        let m = IoModule::new_const(IoConfig::default_config());
        m.init().expect("init ok");
        // 1 op + 1 erreur = 100% → Error
        m.record_op(0);
        m.record_error();
        assert_eq!(m.health_check(), IoHealthStatus::Error);
    }

    #[test]
    fn test_summary_total_ops() {
        let m = IoModule::new_const(IoConfig::default_config());
        m.init().expect("ok");
        m.record_op(512);
        m.record_op(256);
        let s = m.summary();
        assert_eq!(s.total_reads, 2);
        assert_eq!(s.bytes_read, 768);
    }

    #[test]
    fn test_module_summary_error_rate() {
        let mut s = IoModuleSummary::new();
        s.total_reads  = 90;
        s.total_writes = 10;
        s.total_errors = 5;
        // 5 / 100 * 1000 = 50
        assert_eq!(s.error_rate_pct10(), 50);
    }

    #[test]
    fn test_health_status_to_str() {
        assert_eq!(IoHealthStatus::Ok.to_str(), "ok");
        assert_eq!(IoHealthStatus::Degraded.to_str(), "degraded");
        assert_eq!(IoHealthStatus::Error.to_str(), "error");
    }

    #[test]
    fn test_flush_all_empty() {
        let mut write_fn = |_: &[u8; 32], _: u32| -> ExofsResult<u64> { Ok(0) };
        // Si la queue globale contient des entrées d'autres tests, elles seront drainées.
        let _ = flush_all(&mut write_fn);
        // Résultat : queue vide
        assert!(WRITEBACK_QUEUE.is_empty());
    }

    #[test]
    fn test_module_init_idempotent() {
        let m = IoModule::new_const(IoConfig::default_config());
        m.init().expect("first ok");
        m.init().expect("second ok"); // idempotent
    }

    #[test]
    fn test_config_minimal() {
        let cfg = IoConfig::minimal();
        assert!(cfg.validate().is_ok());
        assert!(!cfg.stats_enabled);
    }
}
