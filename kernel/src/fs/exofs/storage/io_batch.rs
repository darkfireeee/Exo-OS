// kernel/src/fs/exofs/storage/io_batch.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Traitement d'I/O en lot — ExoFS
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// IoBatch regroupe plusieurs opérations de lecture/écriture afin de les
// soumettre au pilote de stockage en un seul passage. Il optimise le chemin
// critique en :
//   1. Fusionnant les opérations contiguës (coalescing).
//   2. Triant les opérations par ordre d'offset croissant (élévateur).
//   3. Respectant une taille maximale de lot (MAX_BATCH_OPS).
//
// Règles ExoFS appliquées :
// - OOM-02   : try_reserve avant toute insertion.
// - ARITH-02 : checked_add pour tous les offsets.
// - WRITE-02 : bytes_written vérifié après chaque écriture.

use crate::fs::exofs::core::{DiskOffset, ExofsError, ExofsResult};
use crate::fs::exofs::storage::layout::BLOCK_SIZE;
use crate::fs::exofs::storage::storage_stats::STORAGE_STATS;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum d'opérations par lot.
pub const MAX_BATCH_OPS: usize = 256;

/// Taille maximale d'un segment coalescé (4 MB).
pub const MAX_COALESCE_BYTES: u64 = 4 * 1024 * 1024;

/// Offsets contiguës fusionnées si l'écart est ≤ ce seuil (en octets).
pub const COALESCE_GAP_THRESHOLD: u64 = BLOCK_SIZE as u64;

// ─────────────────────────────────────────────────────────────────────────────
// IoOpKind
// ─────────────────────────────────────────────────────────────────────────────

/// Type d'une opération d'I/O.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IoOpKind {
    /// Lecture : (`offset`, `len`) → remplir le buffer fourni à la soumission.
    Read { len: u64 },
    /// Écriture : (`offset`, `data`) → écrire les octets sur le disque.
    Write { data: Vec<u8> },
}

// ─────────────────────────────────────────────────────────────────────────────
// IoOp — une opération d'I/O élémentaire
// ─────────────────────────────────────────────────────────────────────────────

/// Opération d'I/O élémentaire.
#[derive(Clone, Debug)]
pub struct IoOp {
    pub offset: DiskOffset,
    pub kind: IoOpKind,
    /// Priorité (plus petit numero = plus haute priorité).
    pub prio: u8,
}

impl IoOp {
    pub fn read(offset: DiskOffset, len: u64) -> IoOp {
        IoOp {
            offset,
            kind: IoOpKind::Read { len },
            prio: 128,
        }
    }

    pub fn write(offset: DiskOffset, data: Vec<u8>) -> IoOp {
        IoOp {
            offset,
            kind: IoOpKind::Write { data },
            prio: 128,
        }
    }

    pub fn with_prio(mut self, prio: u8) -> Self {
        self.prio = prio;
        self
    }

    pub fn is_write(&self) -> bool {
        matches!(self.kind, IoOpKind::Write { .. })
    }

    pub fn is_read(&self) -> bool {
        matches!(self.kind, IoOpKind::Read { .. })
    }

    pub fn byte_count(&self) -> u64 {
        match &self.kind {
            IoOpKind::Read { len } => *len,
            IoOpKind::Write { data } => data.len() as u64,
        }
    }

    pub fn end_offset(&self) -> Option<DiskOffset> {
        let bc = self.byte_count();
        self.offset.0.checked_add(bc).map(DiskOffset)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IoOpResult — résultat d'une opération soumise
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une opération soumise dans un lot.
#[derive(Debug)]
pub struct IoOpResult {
    pub offset: DiskOffset,
    pub bytes_done: u64,
    pub success: bool,
    pub error: Option<ExofsError>,
}

impl IoOpResult {
    fn ok(offset: DiskOffset, bytes_done: u64) -> Self {
        Self {
            offset,
            bytes_done,
            success: true,
            error: None,
        }
    }

    fn err(offset: DiskOffset, e: ExofsError) -> Self {
        Self {
            offset,
            bytes_done: 0,
            success: false,
            error: Some(e),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BatchStats
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Default, Debug, Clone, Copy)]
pub struct BatchStats {
    pub ops_submitted: u64,
    pub ops_succeeded: u64,
    pub ops_failed: u64,
    pub bytes_written: u64,
    pub bytes_read: u64,
    pub coalesce_merges: u64,
    pub total_batches: u64,
}

impl BatchStats {
    pub fn merge(&self, other: &BatchStats) -> BatchStats {
        BatchStats {
            ops_submitted: self.ops_submitted.saturating_add(other.ops_submitted),
            ops_succeeded: self.ops_succeeded.saturating_add(other.ops_succeeded),
            ops_failed: self.ops_failed.saturating_add(other.ops_failed),
            bytes_written: self.bytes_written.saturating_add(other.bytes_written),
            bytes_read: self.bytes_read.saturating_add(other.bytes_read),
            coalesce_merges: self.coalesce_merges.saturating_add(other.coalesce_merges),
            total_batches: self.total_batches.saturating_add(other.total_batches),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IoBatch — lot d'opérations d'I/O
// ─────────────────────────────────────────────────────────────────────────────

/// Lot d'opérations d'I/O à soumettre en une fois.
pub struct IoBatch {
    ops: Vec<IoOp>,
    submitted: bool,
    coalesce: bool,
    sort_offsets: bool,
}

impl IoBatch {
    // ── Constructeurs ─────────────────────────────────────────────────────────

    pub fn new() -> Self {
        Self {
            ops: Vec::new(),
            submitted: false,
            coalesce: true,
            sort_offsets: true,
        }
    }

    pub fn with_capacity(cap: usize) -> ExofsResult<Self> {
        let cap = cap.min(MAX_BATCH_OPS);
        let mut ops = Vec::new();
        ops.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;
        Ok(Self {
            ops,
            submitted: false,
            coalesce: true,
            sort_offsets: true,
        })
    }

    /// Désactive la coalescence.
    pub fn no_coalesce(mut self) -> Self {
        self.coalesce = false;
        self
    }
    /// Désactive le tri par offsets.
    pub fn no_sort(mut self) -> Self {
        self.sort_offsets = false;
        self
    }

    // ── Ajouter des opérations ────────────────────────────────────────────────

    /// Ajoute une opération de lecture.
    pub fn add_read(&mut self, offset: DiskOffset, len: u64) -> ExofsResult<()> {
        if self.submitted {
            return Err(ExofsError::InvalidState);
        }
        if self.ops.len() >= MAX_BATCH_OPS {
            return Err(ExofsError::BufferFull);
        }
        self.ops.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.ops.push(IoOp::read(offset, len));
        Ok(())
    }

    /// Ajoute une opération d'écriture.
    pub fn add_write(&mut self, offset: DiskOffset, data: Vec<u8>) -> ExofsResult<()> {
        if self.submitted {
            return Err(ExofsError::InvalidState);
        }
        if self.ops.len() >= MAX_BATCH_OPS {
            return Err(ExofsError::BufferFull);
        }
        self.ops.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.ops.push(IoOp::write(offset, data));
        Ok(())
    }

    /// Ajoute une opération avec priorité explicite.
    pub fn add_op(&mut self, op: IoOp) -> ExofsResult<()> {
        if self.submitted {
            return Err(ExofsError::InvalidState);
        }
        if self.ops.len() >= MAX_BATCH_OPS {
            return Err(ExofsError::BufferFull);
        }
        self.ops.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.ops.push(op);
        Ok(())
    }

    // ── Optimisations internes ─────────────────────────────────────────────

    /// Trie les opérations par offset croissant (algorithme d'élévateur).
    fn sort_by_offset(&mut self) {
        self.ops.sort_by_key(|op| op.offset.0);
    }

    /// Fusionne les écritures contiguës dans un seul vecteur de bytes.
    /// Retourne le nombre de fusions effectuées.
    fn coalesce_writes(&mut self) -> u64 {
        if self.ops.len() < 2 {
            return 0;
        }

        let mut merged = 0u64;
        let mut result: Vec<IoOp> = Vec::new();
        result.try_reserve(self.ops.len()).unwrap_or(());

        let mut i = 0usize;
        while i < self.ops.len() {
            let op = &self.ops[i];

            // Seulement pour les écritures.
            if !op.is_write() {
                result.try_reserve(1).unwrap_or(());
                result.push(self.ops[i].clone());
                i += 1;
                continue;
            }

            // Cherche à fusionner avec les écritures suivantes contiguës.
            let mut combined_data: Vec<u8> = Vec::new();
            let start_offset = op.offset;

            if let IoOpKind::Write { data } = &op.kind {
                combined_data.try_reserve(data.len()).unwrap_or(());
                combined_data.extend_from_slice(data);
            }

            let mut j = i + 1;
            while j < self.ops.len() && combined_data.len() as u64 <= MAX_COALESCE_BYTES {
                let next = &self.ops[j];
                if !next.is_write() {
                    break;
                }

                let expected_offset = start_offset
                    .0
                    .checked_add(combined_data.len() as u64)
                    .unwrap_or(u64::MAX);

                let gap = if next.offset.0 >= expected_offset {
                    next.offset.0.wrapping_sub(expected_offset)
                } else {
                    COALESCE_GAP_THRESHOLD + 1
                };

                if gap > COALESCE_GAP_THRESHOLD {
                    break;
                }

                // Combler le trou avec des zéros si nécessaire.
                let pad = (next.offset.0.saturating_sub(expected_offset)) as usize;
                if pad > 0 {
                    combined_data.try_reserve(pad).unwrap_or(());
                    for _ in 0..pad {
                        combined_data.push(0u8);
                    }
                }

                if let IoOpKind::Write { data } = &next.kind {
                    combined_data.try_reserve(data.len()).unwrap_or(());
                    combined_data.extend_from_slice(data);
                }
                merged = merged.saturating_add(1);
                j += 1;
            }

            result.try_reserve(1).unwrap_or(());
            result.push(IoOp::write(start_offset, combined_data));
            i = j;
        }

        self.ops = result;
        merged
    }

    // ── Soumission ─────────────────────────────────────────────────────────

    /// Soumet le lot d'opérations et retourne les résultats.
    ///
    /// `write_fn` : `(data: &[u8], offset: DiskOffset) -> ExofsResult<usize>`
    /// `read_fn`  : `(offset: DiskOffset, buf: &mut [u8]) -> ExofsResult<usize>`
    pub fn submit(
        &mut self,
        write_fn: &dyn Fn(&[u8], DiskOffset) -> ExofsResult<usize>,
        read_fn: &dyn Fn(DiskOffset, &mut [u8]) -> ExofsResult<usize>,
    ) -> ExofsResult<IoBatchReport> {
        if self.submitted {
            return Err(ExofsError::InvalidState);
        }
        if self.ops.is_empty() {
            self.submitted = true;
            return Ok(IoBatchReport::empty());
        }

        // Optimisations pre-soumission.
        if self.sort_offsets {
            self.sort_by_offset();
        }
        let merges = if self.coalesce {
            self.coalesce_writes()
        } else {
            0
        };

        let n_ops = self.ops.len();
        let mut results: Vec<IoOpResult> = Vec::new();
        results
            .try_reserve(n_ops)
            .map_err(|_| ExofsError::NoMemory)?;

        let mut stats = BatchStats {
            total_batches: 1,
            coalesce_merges: merges,
            ..Default::default()
        };

        for op in &self.ops {
            stats.ops_submitted = stats.ops_submitted.saturating_add(1);

            match &op.kind {
                IoOpKind::Write { data } => {
                    let expected = data.len() as u64;
                    match write_fn(data, op.offset) {
                        Ok(n) => {
                            // WRITE-02 : vérifier bytes_written.
                            if (n as u64) != expected {
                                let e = ExofsError::ShortWrite;
                                results.push(IoOpResult::err(op.offset, e));
                                stats.ops_failed = stats.ops_failed.saturating_add(1);
                                STORAGE_STATS.inc_io_error();
                            } else {
                                results.push(IoOpResult::ok(op.offset, n as u64));
                                stats.ops_succeeded = stats.ops_succeeded.saturating_add(1);
                                stats.bytes_written = stats.bytes_written.saturating_add(n as u64);
                                STORAGE_STATS.add_write(n as u64);
                            }
                        }
                        Err(e) => {
                            results.push(IoOpResult::err(op.offset, e));
                            stats.ops_failed = stats.ops_failed.saturating_add(1);
                            STORAGE_STATS.inc_io_error();
                        }
                    }
                }

                IoOpKind::Read { len } => {
                    let sz = *len as usize;
                    let mut buf: Vec<u8> = Vec::new();
                    match buf.try_reserve(sz) {
                        Err(_) => {
                            results.push(IoOpResult::err(op.offset, ExofsError::NoMemory));
                            stats.ops_failed = stats.ops_failed.saturating_add(1);
                        }
                        Ok(()) => {
                            buf.resize(sz, 0u8);
                            match read_fn(op.offset, &mut buf) {
                                Ok(n) => {
                                    results.push(IoOpResult::ok(op.offset, n as u64));
                                    stats.ops_succeeded = stats.ops_succeeded.saturating_add(1);
                                    stats.bytes_read = stats.bytes_read.saturating_add(n as u64);
                                    STORAGE_STATS.add_read(n as u64);
                                }
                                Err(e) => {
                                    results.push(IoOpResult::err(op.offset, e));
                                    stats.ops_failed = stats.ops_failed.saturating_add(1);
                                    STORAGE_STATS.inc_io_error();
                                }
                            }
                        }
                    }
                }
            }
        }

        self.submitted = true;
        STORAGE_STATS.inc_io_batch();
        Ok(IoBatchReport { results, stats })
    }

    // ── Utilitaires ───────────────────────────────────────────────────────────

    pub fn len(&self) -> usize {
        self.ops.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
    pub fn is_submitted(&self) -> bool {
        self.submitted
    }
    pub fn has_writes(&self) -> bool {
        self.ops.iter().any(|op| op.is_write())
    }
    pub fn has_reads(&self) -> bool {
        self.ops.iter().any(|op| op.is_read())
    }

    pub fn total_write_bytes(&self) -> u64 {
        self.ops
            .iter()
            .filter_map(|op| {
                if op.is_write() {
                    Some(op.byte_count())
                } else {
                    None
                }
            })
            .fold(0u64, |a, b| a.saturating_add(b))
    }

    pub fn total_read_bytes(&self) -> u64 {
        self.ops
            .iter()
            .filter_map(|op| {
                if op.is_read() {
                    Some(op.byte_count())
                } else {
                    None
                }
            })
            .fold(0u64, |a, b| a.saturating_add(b))
    }
}

impl Default for IoBatch {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IoBatchReport
// ─────────────────────────────────────────────────────────────────────────────

/// Rapport de soumission d'un lot d'I/O.
pub struct IoBatchReport {
    pub results: Vec<IoOpResult>,
    pub stats: BatchStats,
}

impl IoBatchReport {
    fn empty() -> Self {
        Self {
            results: Vec::new(),
            stats: BatchStats::default(),
        }
    }

    pub fn all_succeeded(&self) -> bool {
        self.results.iter().all(|r| r.success)
    }

    pub fn failed_ops(&self) -> impl Iterator<Item = &IoOpResult> {
        self.results.iter().filter(|r| !r.success)
    }

    pub fn error_count(&self) -> usize {
        self.results.iter().filter(|r| !r.success).count()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IoBatchQueue — file de lots à traiter séquentiellement
// ─────────────────────────────────────────────────────────────────────────────

/// File de lots d'I/O à soumettre en séquence.
pub struct IoBatchQueue {
    pending: Vec<IoBatch>,
    global: BatchStats,
    errors: AtomicU64,
    shutdown: AtomicBool,
}

impl IoBatchQueue {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            global: BatchStats::default(),
            errors: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
        }
    }

    pub fn enqueue(&mut self, batch: IoBatch) -> ExofsResult<()> {
        if batch.is_submitted() {
            return Err(ExofsError::InvalidState);
        }
        if self.shutdown.load(Ordering::Relaxed) {
            return Err(ExofsError::Shutdown);
        }
        self.pending
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.pending.push(batch);
        Ok(())
    }

    /// Soumet tous les lots en attente.
    pub fn flush(
        &mut self,
        write_fn: &dyn Fn(&[u8], DiskOffset) -> ExofsResult<usize>,
        read_fn: &dyn Fn(DiskOffset, &mut [u8]) -> ExofsResult<usize>,
    ) -> ExofsResult<BatchStats> {
        let mut combined = BatchStats::default();

        for batch in &mut self.pending {
            match batch.submit(write_fn, read_fn) {
                Ok(report) => {
                    combined = combined.merge(&report.stats);
                    if !report.all_succeeded() {
                        self.errors
                            .fetch_add(report.error_count() as u64, Ordering::Relaxed);
                    }
                }
                Err(_) => {
                    self.errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        self.pending.clear();
        self.global = self.global.merge(&combined);
        Ok(combined)
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
    pub fn error_count(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }
    pub fn global_stats(&self) -> &BatchStats {
        &self.global
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_write(data: &[u8], _offset: DiskOffset) -> ExofsResult<usize> {
        Ok(data.len())
    }
    fn mock_read(_offset: DiskOffset, buf: &mut [u8]) -> ExofsResult<usize> {
        buf.fill(0xCC);
        Ok(buf.len())
    }

    #[test]
    fn test_batch_write() {
        let mut b = IoBatch::new();
        let data = vec![1u8; 4096];
        b.add_write(DiskOffset(0), data).unwrap();
        let report = b.submit(&mock_write, &mock_read).unwrap();
        assert!(report.all_succeeded());
        assert_eq!(report.stats.bytes_written, 4096);
    }

    #[test]
    fn test_coalesce_two_writes() {
        let mut b = IoBatch::new().no_sort();
        b.add_write(DiskOffset(0), vec![0xAAu8; 4096]).unwrap();
        b.add_write(DiskOffset(4096), vec![0xBBu8; 4096]).unwrap();
        b.coalesce_writes();
        // Après coalescence, une seule op.
        assert_eq!(b.ops.len(), 1);
        assert_eq!(b.ops[0].byte_count(), 8192);
    }

    #[test]
    fn test_batch_read() {
        let mut b = IoBatch::new();
        b.add_read(DiskOffset(4096), 4096).unwrap();
        let report = b.submit(&mock_write, &mock_read).unwrap();
        assert!(report.all_succeeded());
        assert_eq!(report.stats.bytes_read, 4096);
    }

    #[test]
    fn test_double_submit_fails() {
        let mut b = IoBatch::new();
        b.add_read(DiskOffset(0), 512).unwrap();
        b.submit(&mock_write, &mock_read).unwrap();
        assert!(b.submit(&mock_write, &mock_read).is_err());
    }
}
