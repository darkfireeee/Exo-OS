//! IO asynchrone ExoFS — files de soumission/complétion Ring 0.
//!
//! Modèle simplifié inspiré d'io_uring : soumission non-bloquante,
//! completion via polling ou callback kernel.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

use crate::fs::exofs::core::{BlobId, FsError};
use crate::scheduler::sync::spinlock::SpinLock;

/// Type d'opération asynchrone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AsyncOpKind {
    Read  = 0,
    Write = 1,
    Flush = 2,
}

/// État d'un handle asynchrone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AsyncState {
    Pending   = 0,
    Running   = 1,
    Completed = 2,
    Failed    = 3,
}

/// Handle d'une opération IO asynchrone.
pub struct AsyncIoHandle {
    pub id: u64,
    pub kind: AsyncOpKind,
    pub blob_id: BlobId,
    pub offset: u64,
    pub len: usize,
    state: AtomicU8,
    pub result_len: AtomicU64,
}

impl AsyncIoHandle {
    pub fn new(id: u64, kind: AsyncOpKind, blob_id: BlobId, offset: u64, len: usize) -> Self {
        Self {
            id,
            kind,
            blob_id,
            offset,
            len,
            state: AtomicU8::new(AsyncState::Pending as u8),
            result_len: AtomicU64::new(0),
        }
    }

    pub fn state(&self) -> AsyncState {
        match self.state.load(Ordering::Acquire) {
            1 => AsyncState::Running,
            2 => AsyncState::Completed,
            3 => AsyncState::Failed,
            _ => AsyncState::Pending,
        }
    }

    pub fn set_state(&self, s: AsyncState) {
        self.state.store(s as u8, Ordering::Release);
    }

    pub fn set_result(&self, len: u64) {
        self.result_len.store(len, Ordering::Release);
    }

    pub fn is_done(&self) -> bool {
        let s = self.state();
        s == AsyncState::Completed || s == AsyncState::Failed
    }
}

/// File de soumission/complétion d'opérations IO asynchrones.
pub struct AsyncIoQueue {
    pending: SpinLock<VecDeque<u64>>,  // IDs des handles en attente
    next_id: AtomicU64,
}

impl AsyncIoQueue {
    pub const fn new() -> Self {
        Self {
            pending: SpinLock::new(VecDeque::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// Génère un nouvel ID d'opération unique.
    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Soumet une opération (enfile son ID).
    pub fn submit(&self, id: u64) -> Result<(), FsError> {
        let mut q = self.pending.lock();
        q.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        q.push_back(id);
        Ok(())
    }

    /// Récupère jusqu'à `max` IDs d'opérations à traiter.
    pub fn drain(&self, max: usize) -> Result<Vec<u64>, FsError> {
        let mut q = self.pending.lock();
        let n = q.len().min(max);
        let mut out = Vec::new();
        out.try_reserve(n).map_err(|_| FsError::OutOfMemory)?;
        for _ in 0..n {
            if let Some(id) = q.pop_front() {
                out.push(id);
            }
        }
        Ok(out)
    }

    /// Nombre d'opérations en attente.
    pub fn pending_count(&self) -> usize {
        self.pending.lock().len()
    }
}

/// Queue global d'IO asynchrone ExoFS.
pub static ASYNC_IO_QUEUE: AsyncIoQueue = AsyncIoQueue::new();
