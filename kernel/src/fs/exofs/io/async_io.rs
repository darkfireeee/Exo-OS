//! async_io.rs — File d'opérations IO asynchrones (no_std, sans executor).
//!
//! Ce module fournit :
//!  - `AsyncOpKind`    : Read / Write / Flush / Discard / Sync.
//!  - `AsyncState`     : état d'une opération (Pending/Running/Completed/Failed).
//!  - `AsyncIoHandle`  : handle thread-safe sur une op async (AtomicU8/U64).
//!  - `AsyncIoQueue`   : ring de handles avec spinlock AtomicU64.
//!  - `ASYNC_IO_QUEUE` : singleton global.
//!
//! RECUR-01 : boucles while — aucune récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add/mul.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, AtomicU64, AtomicI32, Ordering};
use core::cell::UnsafeCell;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─── AsyncOpKind ─────────────────────────────────────────────────────────────

/// Genre d'opération IO asynchrone.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum AsyncOpKind {
    Read    = 0,
    Write   = 1,
    Flush   = 2,
    Discard = 3,
    Sync    = 4,
}

impl AsyncOpKind {
    pub fn from_u8(v: u8) -> ExofsResult<Self> {
        match v {
            0 => Ok(AsyncOpKind::Read),
            1 => Ok(AsyncOpKind::Write),
            2 => Ok(AsyncOpKind::Flush),
            3 => Ok(AsyncOpKind::Discard),
            4 => Ok(AsyncOpKind::Sync),
            _ => Err(ExofsError::InvalidArgument),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AsyncOpKind::Read    => "read",
            AsyncOpKind::Write   => "write",
            AsyncOpKind::Flush   => "flush",
            AsyncOpKind::Discard => "discard",
            AsyncOpKind::Sync    => "sync",
        }
    }

    pub fn is_read(self)  -> bool { matches!(self, AsyncOpKind::Read) }
    pub fn is_write(self) -> bool { matches!(self, AsyncOpKind::Write) }
}

// ─── AsyncState ───────────────────────────────────────────────────────────────

/// État d'avancement d'une opération async.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum AsyncState {
    Pending   = 0,
    Running   = 1,
    Completed = 2,
    Failed    = 3,
    Cancelled = 4,
}

impl AsyncState {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => AsyncState::Pending,
            1 => AsyncState::Running,
            2 => AsyncState::Completed,
            3 => AsyncState::Failed,
            4 => AsyncState::Cancelled,
            _ => AsyncState::Failed,
        }
    }

    pub fn is_done(self) -> bool {
        matches!(self, AsyncState::Completed | AsyncState::Failed | AsyncState::Cancelled)
    }

    pub fn is_ok(self) -> bool { matches!(self, AsyncState::Completed) }
}

// ─── AsyncIoHandle ────────────────────────────────────────────────────────────

/// Handle thread-safe sur une opération IO asynchrone.
///
/// Tous les champs sont atomiques — pas d'UnsafeCell nécessaire.
pub struct AsyncIoHandle {
    pub op_id:   u64,
    kind_raw:    u8,                // AsyncOpKind
    state:       AtomicU8,
    result_code: AtomicI32,         // 0=ok, -errno style
    bytes_done:  AtomicU64,
    blob_id:     [u8; 32],
}

// SAFETY: AtomicU8/AtomicU64/AtomicI32 sont Send+Sync ; blob_id est un tableau.
unsafe impl Send for AsyncIoHandle {}
unsafe impl Sync for AsyncIoHandle {}

impl AsyncIoHandle {
    pub fn new(op_id: u64, kind: AsyncOpKind, blob_id: [u8; 32]) -> Self {
        Self {
            op_id,
            kind_raw: kind as u8,
            state: AtomicU8::new(AsyncState::Pending as u8),
            result_code: AtomicI32::new(0),
            bytes_done: AtomicU64::new(0),
            blob_id,
        }
    }

    pub fn kind(&self) -> AsyncOpKind {
        AsyncOpKind::from_u8(self.kind_raw).unwrap_or(AsyncOpKind::Read)
    }

    pub fn state(&self) -> AsyncState {
        AsyncState::from_u8(self.state.load(Ordering::Acquire))
    }

    pub fn bytes_done(&self) -> u64 { self.bytes_done.load(Ordering::Acquire) }
    pub fn result_code(&self) -> i32 { self.result_code.load(Ordering::Acquire) }
    pub fn blob_id(&self) -> &[u8; 32] { &self.blob_id }
    pub fn is_done(&self) -> bool { self.state().is_done() }
    pub fn is_ok(&self) -> bool { self.state().is_ok() }

    /// Transition vers Running.
    pub fn mark_running(&self) -> ExofsResult<()> {
        let prev = self.state.compare_exchange(
            AsyncState::Pending as u8, AsyncState::Running as u8,
            Ordering::AcqRel, Ordering::Acquire,
        ).map_err(|_| ExofsError::InvalidArgument)?;
        let _ = prev;
        Ok(())
    }

    /// Marque l'opération comme terminée avec succès.
    pub fn complete(&self, bytes: u64) {
        self.bytes_done.store(bytes, Ordering::Release);
        self.result_code.store(0, Ordering::Release);
        self.state.store(AsyncState::Completed as u8, Ordering::Release);
    }

    /// Marque l'opération comme échouée.
    pub fn fail(&self, code: i32) {
        self.result_code.store(code, Ordering::Release);
        self.state.store(AsyncState::Failed as u8, Ordering::Release);
    }

    /// Annule l'opération (si encore Pending).
    pub fn cancel(&self) {
        let _ = self.state.compare_exchange(
            AsyncState::Pending as u8, AsyncState::Cancelled as u8,
            Ordering::AcqRel, Ordering::Acquire,
        );
    }
}

// ─── Constante de profondeur de queue ────────────────────────────────────────

pub const ASYNC_IO_QUEUE_DEPTH: usize = 256;

// ─── AsyncIoSlot (slot interne) ────────────────────────────────────────────────

/// Slot interne de la queue async.
#[derive(Clone, Copy)]
struct AsyncIoSlot {
    op_id: u64,
    kind:  u8,
    state: u8,
    bytes: u64,
    blob_id: [u8; 32],
    result: i32,
}

impl AsyncIoSlot {
    const fn empty() -> Self {
        Self { op_id: 0, kind: 0, state: 0, bytes: 0, blob_id: [0u8; 32], result: 0 }
    }
}

// ─── AsyncIoQueue ─────────────────────────────────────────────────────────────

/// Queue async à ring de 256 slots, protégée par un spinlock AtomicU64.
///
/// RECUR-01 : toutes les boucles while, aucune récursion.
pub struct AsyncIoQueue {
    slots:   UnsafeCell<[AsyncIoSlot; ASYNC_IO_QUEUE_DEPTH]>,
    head:    AtomicU64,   // index de lecture (pop)
    tail:    AtomicU64,   // index d'écriture (push)
    count:   AtomicU64,   // nombre de slots occupés
    lock:    AtomicU64,   // spinlock : 0=libre, 1=pris
    next_id: AtomicU64,   // compteur monotone d'op_id
    // stats
    submitted:  AtomicU64,
    completed:  AtomicU64,
    failed:     AtomicU64,
    cancelled:  AtomicU64,
}

// SAFETY: accès sous spinlock exclusif.
unsafe impl Sync for AsyncIoQueue {}
unsafe impl Send for AsyncIoQueue {}

impl AsyncIoQueue {
    const EMPTY_SLOT: AsyncIoSlot = AsyncIoSlot::empty();

    pub const fn new_const() -> Self {
        Self {
            slots:      UnsafeCell::new([Self::EMPTY_SLOT; ASYNC_IO_QUEUE_DEPTH]),
            head:       AtomicU64::new(0),
            tail:       AtomicU64::new(0),
            count:      AtomicU64::new(0),
            lock:       AtomicU64::new(0),
            next_id:    AtomicU64::new(1),
            submitted:  AtomicU64::new(0),
            completed:  AtomicU64::new(0),
            failed:     AtomicU64::new(0),
            cancelled:  AtomicU64::new(0),
        }
    }

    fn acquire(&self) {
        while self.lock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }

    fn release(&self) { self.lock.store(0, Ordering::Release); }

    fn alloc_id(&self) -> u64 { self.next_id.fetch_add(1, Ordering::Relaxed) }

    /// Soumet une opération dans la queue. Retourne l'op_id.
    pub fn submit(&self, kind: AsyncOpKind, blob_id: [u8; 32]) -> ExofsResult<u64> {
        self.acquire();
        let result = (|| {
            if self.count.load(Ordering::Relaxed) as usize >= ASYNC_IO_QUEUE_DEPTH {
                return Err(ExofsError::Resource);
            }
            let op_id = self.alloc_id();
            let tail = self.tail.load(Ordering::Relaxed) as usize % ASYNC_IO_QUEUE_DEPTH;
            // SAFETY: accès sous spinlock exclusif, tail en range.
            unsafe {
                let slots = &mut *self.slots.get();
                slots[tail] = AsyncIoSlot { op_id, kind: kind as u8,
                    state: AsyncState::Pending as u8, bytes: 0, blob_id, result: 0 };
            }
            self.tail.fetch_add(1, Ordering::Relaxed);
            self.count.fetch_add(1, Ordering::Relaxed);
            self.submitted.fetch_add(1, Ordering::Relaxed);
            Ok(op_id)
        })();
        self.release();
        result
    }

    /// Marque une op comme Running (RECUR-01 : while).
    pub fn mark_running(&self, op_id: u64) -> ExofsResult<()> {
        self.acquire();
        let result = (|| {
            let count = self.count.load(Ordering::Relaxed) as usize;
            let head = self.head.load(Ordering::Relaxed) as usize;
            let mut i = 0usize;
            while i < count {
                let idx = head.wrapping_add(i) % ASYNC_IO_QUEUE_DEPTH;
                // SAFETY: sous spinlock, idx en range.
                unsafe {
                    let slots = &mut *self.slots.get();
                    if slots[idx].op_id == op_id {
                        slots[idx].state = AsyncState::Running as u8;
                        return Ok(());
                    }
                }
                i = i.wrapping_add(1);
            }
            Err(ExofsError::ObjectNotFound)
        })();
        self.release();
        result
    }

    /// Complète une op (RECUR-01 : while).
    pub fn complete(&self, op_id: u64, bytes: u64) -> ExofsResult<()> {
        self.acquire();
        let result = (|| {
            let count = self.count.load(Ordering::Relaxed) as usize;
            let head = self.head.load(Ordering::Relaxed) as usize;
            let mut i = 0usize;
            while i < count {
                let idx = head.wrapping_add(i) % ASYNC_IO_QUEUE_DEPTH;
                // SAFETY: sous spinlock, idx en range.
                unsafe {
                    let slots = &mut *self.slots.get();
                    if slots[idx].op_id == op_id {
                        slots[idx].state = AsyncState::Completed as u8;
                        slots[idx].bytes = bytes;
                        self.completed.fetch_add(1, Ordering::Relaxed);
                        return Ok(());
                    }
                }
                i = i.wrapping_add(1);
            }
            Err(ExofsError::ObjectNotFound)
        })();
        self.release();
        result
    }

    /// Marque une op comme Failed (RECUR-01 : while).
    pub fn fail(&self, op_id: u64, code: i32) -> ExofsResult<()> {
        self.acquire();
        let result = (|| {
            let count = self.count.load(Ordering::Relaxed) as usize;
            let head = self.head.load(Ordering::Relaxed) as usize;
            let mut i = 0usize;
            while i < count {
                let idx = head.wrapping_add(i) % ASYNC_IO_QUEUE_DEPTH;
                // SAFETY: sous spinlock, idx en range.
                unsafe {
                    let slots = &mut *self.slots.get();
                    if slots[idx].op_id == op_id {
                        slots[idx].state = AsyncState::Failed as u8;
                        slots[idx].result = code;
                        self.failed.fetch_add(1, Ordering::Relaxed);
                        return Ok(());
                    }
                }
                i = i.wrapping_add(1);
            }
            Err(ExofsError::ObjectNotFound)
        })();
        self.release();
        result
    }

    /// Annule une op Pending (RECUR-01 : while).
    pub fn cancel(&self, op_id: u64) -> ExofsResult<()> {
        self.acquire();
        let result = (|| {
            let count = self.count.load(Ordering::Relaxed) as usize;
            let head = self.head.load(Ordering::Relaxed) as usize;
            let mut i = 0usize;
            while i < count {
                let idx = head.wrapping_add(i) % ASYNC_IO_QUEUE_DEPTH;
                // SAFETY: sous spinlock, idx en range.
                unsafe {
                    let slots = &mut *self.slots.get();
                    if slots[idx].op_id == op_id
                        && slots[idx].state == AsyncState::Pending as u8
                    {
                        slots[idx].state = AsyncState::Cancelled as u8;
                        self.cancelled.fetch_add(1, Ordering::Relaxed);
                        return Ok(());
                    }
                }
                i = i.wrapping_add(1);
            }
            Err(ExofsError::ObjectNotFound)
        })();
        self.release();
        result
    }

    /// Dépile la prochaine op terminée (RECUR-01 : while).
    pub fn pop_done(&self) -> Option<(u64, AsyncState, u64)> {
        self.acquire();
        let result = (|| {
            let count = self.count.load(Ordering::Relaxed) as usize;
            let head = self.head.load(Ordering::Relaxed) as usize;
            let mut i = 0usize;
            while i < count {
                let idx = head.wrapping_add(i) % ASYNC_IO_QUEUE_DEPTH;
                // SAFETY: sous spinlock, idx en range.
                let (op_id, state, bytes) = unsafe {
                    let slots = &*self.slots.get();
                    (slots[idx].op_id, AsyncState::from_u8(slots[idx].state), slots[idx].bytes)
                };
                if state.is_done() {
                    // Déplacer le head si c'est le premier slot
                    if i == 0 {
                        self.head.fetch_add(1, Ordering::Relaxed);
                        self.count.fetch_sub(1, Ordering::Relaxed);
                    }
                    return Some((op_id, state, bytes));
                }
                i = i.wrapping_add(1);
            }
            None
        })();
        self.release();
        result
    }

    pub fn pending_count(&self) -> u64 { self.count.load(Ordering::Relaxed) }
    pub fn submitted_total(&self) -> u64 { self.submitted.load(Ordering::Relaxed) }
    pub fn completed_total(&self) -> u64 { self.completed.load(Ordering::Relaxed) }
    pub fn failed_total(&self) -> u64 { self.failed.load(Ordering::Relaxed) }
    pub fn is_empty(&self) -> bool { self.count.load(Ordering::Relaxed) == 0 }

    pub fn reset_stats(&self) {
        self.submitted.store(0, Ordering::Release);
        self.completed.store(0, Ordering::Release);
        self.failed.store(0, Ordering::Release);
        self.cancelled.store(0, Ordering::Release);
    }
}

/// Singleton global de la queue async io.
pub static ASYNC_IO_QUEUE: AsyncIoQueue = AsyncIoQueue::new_const();

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    fn make_id(n: u8) -> [u8; 32] { let mut id = [0u8; 32]; id[0] = n; id }

    #[test]
    fn test_async_op_kind_from_u8() {
        assert_eq!(AsyncOpKind::from_u8(0).expect("ok"), AsyncOpKind::Read);
        assert_eq!(AsyncOpKind::from_u8(1).expect("ok"), AsyncOpKind::Write);
        assert!(AsyncOpKind::from_u8(99).is_err());
    }

    #[test]
    fn test_async_state_is_done() {
        assert!(!AsyncState::Pending.is_done());
        assert!(!AsyncState::Running.is_done());
        assert!(AsyncState::Completed.is_done());
        assert!(AsyncState::Failed.is_done());
        assert!(AsyncState::Cancelled.is_done());
    }

    #[test]
    fn test_handle_lifecycle() {
        let h = AsyncIoHandle::new(1, AsyncOpKind::Read, make_id(1));
        assert_eq!(h.state(), AsyncState::Pending);
        h.mark_running().expect("ok");
        assert_eq!(h.state(), AsyncState::Running);
        h.complete(512);
        assert!(h.is_ok());
        assert_eq!(h.bytes_done(), 512);
    }

    #[test]
    fn test_handle_fail() {
        let h = AsyncIoHandle::new(2, AsyncOpKind::Write, make_id(2));
        h.fail(-5);
        assert_eq!(h.state(), AsyncState::Failed);
        assert_eq!(h.result_code(), -5);
    }

    #[test]
    fn test_handle_cancel() {
        let h = AsyncIoHandle::new(3, AsyncOpKind::Flush, make_id(3));
        h.cancel();
        assert_eq!(h.state(), AsyncState::Cancelled);
    }

    #[test]
    fn test_queue_submit() {
        let q = AsyncIoQueue::new_const();
        let id = q.submit(AsyncOpKind::Read, make_id(1)).expect("ok");
        assert!(id > 0);
        assert_eq!(q.pending_count(), 1);
    }

    #[test]
    fn test_queue_complete() {
        let q = AsyncIoQueue::new_const();
        let id = q.submit(AsyncOpKind::Write, make_id(2)).expect("ok");
        q.complete(id, 1024).expect("ok");
        assert_eq!(q.completed_total(), 1);
    }

    #[test]
    fn test_queue_fail() {
        let q = AsyncIoQueue::new_const();
        let id = q.submit(AsyncOpKind::Read, make_id(3)).expect("ok");
        q.fail(id, -1).expect("ok");
        assert_eq!(q.failed_total(), 1);
    }

    #[test]
    fn test_queue_cancel() {
        let q = AsyncIoQueue::new_const();
        let id = q.submit(AsyncOpKind::Read, make_id(4)).expect("ok");
        q.cancel(id).expect("ok");
    }

    #[test]
    fn test_queue_pop_done() {
        let q = AsyncIoQueue::new_const();
        let id = q.submit(AsyncOpKind::Read, make_id(5)).expect("ok");
        q.complete(id, 256).expect("ok");
        let done = q.pop_done();
        assert!(done.is_some());
        let (oid, state, bytes) = done.expect("ok");
        assert_eq!(oid, id);
        assert_eq!(state, AsyncState::Completed);
        assert_eq!(bytes, 256);
    }

    #[test]
    fn test_queue_multi_submit() {
        let q = AsyncIoQueue::new_const();
        let mut ids = Vec::new();
        for i in 0u8..10 {
            ids.push(q.submit(AsyncOpKind::Read, make_id(i)).expect("ok"));
        }
        assert_eq!(q.pending_count(), 10);
        for id in &ids {
            q.complete(*id, 64).expect("ok");
        }
        assert_eq!(q.completed_total(), 10);
    }

    #[test]
    fn test_queue_stats_reset() {
        let q = AsyncIoQueue::new_const();
        let id = q.submit(AsyncOpKind::Flush, make_id(0)).expect("ok");
        q.complete(id, 0).expect("ok");
        q.reset_stats();
        assert_eq!(q.submitted_total(), 0);
    }

    #[test]
    fn test_mark_running() {
        let q = AsyncIoQueue::new_const();
        let id = q.submit(AsyncOpKind::Write, make_id(9)).expect("ok");
        q.mark_running(id).expect("ok");
    }
}
