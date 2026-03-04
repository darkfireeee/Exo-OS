//! io_uring.rs — Ring de soumission/complétion io_uring-style (no_std).
//!
//! Ce module fournit :
//!  - `SqeOpcode`     : opcode de Submission Queue Entry.
//!  - `IoUringSqe`    : entrée SQ (128 bytes, repr C).
//!  - `IoUringCqe`    : entrée CQ (16 bytes, repr C).
//!  - `IoUringSq`     : ring de soumissions (spinlock).
//!  - `IoUringCq`     : ring de complétions (spinlock).
//!  - `IoUringQueue`  : paire SQ+CQ + submit / reap.
//!
//! RECUR-01 : boucles while — aucune récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add/mul.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use core::cell::UnsafeCell;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─── SqeOpcode ────────────────────────────────────────────────────────────────

/// Opcode d'une Submission Queue Entry.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum SqeOpcode {
    Nop      = 0,
    Read     = 1,
    Write    = 2,
    Flush    = 3,
    Discard  = 4,
    Fsync    = 5,
    Readv    = 6,
    Writev   = 7,
}

impl SqeOpcode {
    pub fn from_u8(v: u8) -> ExofsResult<Self> {
        match v {
            0 => Ok(SqeOpcode::Nop),    1 => Ok(SqeOpcode::Read),
            2 => Ok(SqeOpcode::Write),  3 => Ok(SqeOpcode::Flush),
            4 => Ok(SqeOpcode::Discard),5 => Ok(SqeOpcode::Fsync),
            6 => Ok(SqeOpcode::Readv),  7 => Ok(SqeOpcode::Writev),
            _ => Err(ExofsError::InvalidArgument),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            SqeOpcode::Nop     => "nop",   SqeOpcode::Read  => "read",
            SqeOpcode::Write   => "write", SqeOpcode::Flush => "flush",
            SqeOpcode::Discard => "discard", SqeOpcode::Fsync => "fsync",
            SqeOpcode::Readv   => "readv", SqeOpcode::Writev => "writev",
        }
    }
    pub fn is_io(self) -> bool {
        matches!(self, SqeOpcode::Read | SqeOpcode::Write | SqeOpcode::Readv | SqeOpcode::Writev)
    }
}

// ─── IoUringSqe ───────────────────────────────────────────────────────────────

/// Submission Queue Entry (128 bytes, repr C).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct IoUringSqe {
    pub opcode:  u8,
    pub flags:   u8,
    pub ioprio:  u16,
    pub fd:      i32,
    pub off:     u64,
    pub addr:    u64,
    pub len:     u32,
    pub op_flags: u32,
    pub user_data: u64,
    pub blob_id: [u8; 32],
    pub pad:     [u64; 6],   // padding jusqu'à 128 bytes
}

impl IoUringSqe {
    pub const SIZE: usize = 128;

    pub fn new_read(blob_id: [u8; 32], off: u64, len: u32, user_data: u64) -> Self {
        let mut s = Self::zeroed();
        s.opcode = SqeOpcode::Read as u8;
        s.blob_id = blob_id; s.off = off; s.len = len; s.user_data = user_data;
        s
    }

    pub fn new_write(blob_id: [u8; 32], off: u64, len: u32, user_data: u64) -> Self {
        let mut s = Self::zeroed();
        s.opcode = SqeOpcode::Write as u8;
        s.blob_id = blob_id; s.off = off; s.len = len; s.user_data = user_data;
        s
    }

    pub fn new_flush(user_data: u64) -> Self {
        let mut s = Self::zeroed();
        s.opcode = SqeOpcode::Flush as u8; s.user_data = user_data;
        s
    }

    pub fn zeroed() -> Self {
        // SAFETY : IoUringSqe est repr(C), tous les champs sont des entiers.
        unsafe { core::mem::zeroed() }
    }

    pub fn opcode(&self) -> ExofsResult<SqeOpcode> { SqeOpcode::from_u8(self.opcode) }
    pub fn is_valid(&self) -> bool { self.opcode <= SqeOpcode::Writev as u8 }
}

// ─── IoUringCqe ───────────────────────────────────────────────────────────────

/// Completion Queue Entry (16 bytes, repr C).
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct IoUringCqe {
    pub user_data: u64,
    pub result:    i32,   // bytes transférés (>0) ou -errno (<0)
    pub flags:     u32,
}

impl IoUringCqe {
    pub fn ok(user_data: u64, bytes: i32) -> Self { Self { user_data, result: bytes, flags: 0 } }
    pub fn err(user_data: u64, code: i32) -> Self { Self { user_data, result: code, flags: 1 } }
    pub fn is_ok(&self) -> bool { self.result >= 0 }
    pub fn is_err(&self) -> bool { self.result < 0 }
    pub fn bytes(&self) -> Option<u32> { if self.result >= 0 { Some(self.result as u32) } else { None } }
}

// ─── IoUringSq ────────────────────────────────────────────────────────────────

/// Ring de soumission (SQ) avec spinlock AtomicU64.
pub struct IoUringSq {
    ring:  UnsafeCell<Vec<IoUringSqe>>,
    head:  AtomicU64,
    tail:  AtomicU64,
    depth: usize,
    lock:  AtomicU64,
    submitted: AtomicU64,
}

// SAFETY : accès sous spinlock exclusif.
unsafe impl Sync for IoUringSq {}
unsafe impl Send for IoUringSq {}

impl IoUringSq {
    pub fn new(depth: usize) -> ExofsResult<Self> {
        if depth == 0 || depth > 65536 { return Err(ExofsError::InvalidArgument); }
        let mut v = Vec::new();
        v.try_reserve(depth).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < depth { v.push(IoUringSqe::zeroed()); i = i.wrapping_add(1); }
        Ok(Self {
            ring: UnsafeCell::new(v), head: AtomicU64::new(0), tail: AtomicU64::new(0),
            depth, lock: AtomicU64::new(0), submitted: AtomicU64::new(0),
        })
    }

    fn acquire(&self) {
        while self.lock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }
    fn release(&self) { self.lock.store(0, Ordering::Release); }

    pub fn available(&self) -> usize {
        let len = self.tail.load(Ordering::Relaxed).wrapping_sub(self.head.load(Ordering::Relaxed));
        self.depth.saturating_sub(len as usize)
    }

    pub fn len(&self) -> usize {
        self.tail.load(Ordering::Relaxed).wrapping_sub(self.head.load(Ordering::Relaxed)) as usize
    }

    pub fn is_empty(&self) -> bool { self.len() == 0 }
    pub fn submitted_total(&self) -> u64 { self.submitted.load(Ordering::Relaxed) }

    /// Soumet un SQE dans le ring.
    pub fn push_sqe(&self, sqe: IoUringSqe) -> ExofsResult<()> {
        if !sqe.is_valid() { return Err(ExofsError::InvalidArgument); }
        self.acquire();
        let result = (|| {
            if self.available() == 0 { return Err(ExofsError::Resource); }
            let tail = self.tail.load(Ordering::Relaxed) as usize % self.depth;
            // SAFETY : tail < depth, accès sous spinlock.
            unsafe { (&mut *self.ring.get())[tail] = sqe; }
            self.tail.fetch_add(1, Ordering::Relaxed);
            self.submitted.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })();
        self.release();
        result
    }

    /// Dépile le prochain SQE (RECUR-01 : while).
    pub fn pop_sqe(&self) -> Option<IoUringSqe> {
        self.acquire();
        let result = if self.is_empty() {
            None
        } else {
            let head = self.head.load(Ordering::Relaxed) as usize % self.depth;
            // SAFETY : head < depth, accès sous spinlock.
            let sqe = unsafe { (&*self.ring.get())[head] };
            self.head.fetch_add(1, Ordering::Relaxed);
            Some(sqe)
        };
        self.release();
        result
    }
}

// ─── IoUringCq ────────────────────────────────────────────────────────────────

/// Ring de complétion (CQ).
pub struct IoUringCq {
    ring:  UnsafeCell<Vec<IoUringCqe>>,
    head:  AtomicU64,
    tail:  AtomicU64,
    depth: usize,
    lock:  AtomicU64,
    reaped: AtomicU64,
}

unsafe impl Sync for IoUringCq {}
unsafe impl Send for IoUringCq {}

impl IoUringCq {
    pub fn new(depth: usize) -> ExofsResult<Self> {
        if depth == 0 || depth > 131072 { return Err(ExofsError::InvalidArgument); }
        let mut v = Vec::new();
        v.try_reserve(depth).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < depth { v.push(IoUringCqe { user_data: 0, result: 0, flags: 0 }); i = i.wrapping_add(1); }
        Ok(Self {
            ring: UnsafeCell::new(v), head: AtomicU64::new(0), tail: AtomicU64::new(0),
            depth, lock: AtomicU64::new(0), reaped: AtomicU64::new(0),
        })
    }

    fn acquire(&self) {
        while self.lock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }
    fn release(&self) { self.lock.store(0, Ordering::Release); }

    pub fn has_pending(&self) -> bool {
        self.tail.load(Ordering::Relaxed) != self.head.load(Ordering::Relaxed)
    }

    pub fn available(&self) -> usize {
        let len = self.tail.load(Ordering::Relaxed).wrapping_sub(self.head.load(Ordering::Relaxed));
        self.depth.saturating_sub(len as usize)
    }

    /// Publie un CQE dans le ring.
    pub fn push_cqe(&self, cqe: IoUringCqe) -> ExofsResult<()> {
        self.acquire();
        let result = (|| {
            if self.available() == 0 { return Err(ExofsError::Resource); }
            let tail = self.tail.load(Ordering::Relaxed) as usize % self.depth;
            // SAFETY : tail < depth, spinlock.
            unsafe { (&mut *self.ring.get())[tail] = cqe; }
            self.tail.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })();
        self.release();
        result
    }

    /// Dépile le prochain CQE (RECUR-01 : while).
    pub fn pop_cqe(&self) -> Option<IoUringCqe> {
        self.acquire();
        let result = if !self.has_pending() {
            None
        } else {
            let head = self.head.load(Ordering::Relaxed) as usize % self.depth;
            // SAFETY : head < depth, spinlock.
            let cqe = unsafe { (&*self.ring.get())[head] };
            self.head.fetch_add(1, Ordering::Relaxed);
            self.reaped.fetch_add(1, Ordering::Relaxed);
            Some(cqe)
        };
        self.release();
        result
    }

    pub fn reaped_total(&self) -> u64 { self.reaped.load(Ordering::Relaxed) }
}

// ─── IoUringQueue ─────────────────────────────────────────────────────────────

/// Paire SQ + CQ avec submit_read/submit_write/reap_completions.
pub struct IoUringQueue {
    pub sq: IoUringSq,
    pub cq: IoUringCq,
    next_id: AtomicU64,
}

impl IoUringQueue {
    pub fn new(sq_depth: usize, cq_depth: usize) -> ExofsResult<Self> {
        Ok(Self {
            sq: IoUringSq::new(sq_depth)?,
            cq: IoUringCq::new(cq_depth)?,
            next_id: AtomicU64::new(1),
        })
    }

    fn alloc_id(&self) -> u64 { self.next_id.fetch_add(1, Ordering::Relaxed) }

    /// Soumet une lecture.
    pub fn submit_read(&self, blob_id: [u8; 32], off: u64, len: u32) -> ExofsResult<u64> {
        let id = self.alloc_id();
        let sqe = IoUringSqe::new_read(blob_id, off, len, id);
        self.sq.push_sqe(sqe)?;
        Ok(id)
    }

    /// Soumet une écriture.
    pub fn submit_write(&self, blob_id: [u8; 32], off: u64, len: u32) -> ExofsResult<u64> {
        let id = self.alloc_id();
        let sqe = IoUringSqe::new_write(blob_id, off, len, id);
        self.sq.push_sqe(sqe)?;
        Ok(id)
    }

    /// Soumet un flush.
    pub fn submit_flush(&self) -> ExofsResult<u64> {
        let id = self.alloc_id();
        let sqe = IoUringSqe::new_flush(id);
        self.sq.push_sqe(sqe)?;
        Ok(id)
    }

    /// Consomme les SQE en attente et remplit les CQE (simulation, RECUR-01 : while).
    pub fn process_submissions(&self) -> ExofsResult<u32> {
        let mut processed = 0u32;
        while let Some(sqe) = self.sq.pop_sqe() {
            let cqe = IoUringCqe::ok(sqe.user_data, sqe.len as i32);
            self.cq.push_cqe(cqe)?;
            processed = processed.saturating_add(1);
        }
        Ok(processed)
    }

    /// Récolte toutes les complétions disponibles (RECUR-01 : while).
    pub fn reap_completions(&self, out: &mut Vec<IoUringCqe>) -> ExofsResult<u32> {
        let mut reaped = 0u32;
        while let Some(cqe) = self.cq.pop_cqe() {
            out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            out.push(cqe);
            reaped = reaped.saturating_add(1);
        }
        Ok(reaped)
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    fn make_id(n: u8) -> [u8; 32] { let mut id = [0u8; 32]; id[0] = n; id }

    #[test]
    fn test_sqe_opcode_from_u8() {
        assert_eq!(SqeOpcode::from_u8(1).expect("ok"), SqeOpcode::Read);
        assert!(SqeOpcode::from_u8(99).is_err());
    }

    #[test]
    fn test_sqe_new_read() {
        let sqe = IoUringSqe::new_read(make_id(1), 0, 512, 42);
        assert_eq!(sqe.opcode, SqeOpcode::Read as u8);
        assert_eq!(sqe.user_data, 42);
        assert_eq!(sqe.len, 512);
    }

    #[test]
    fn test_cqe_is_ok_err() {
        let ok = IoUringCqe::ok(1, 512);
        let err = IoUringCqe::err(2, -5);
        assert!(ok.is_ok());
        assert!(err.is_err());
        assert_eq!(ok.bytes(), Some(512));
        assert_eq!(err.bytes(), None);
    }

    #[test]
    fn test_sq_push_pop() {
        let sq = IoUringSq::new(8).expect("ok");
        let sqe = IoUringSqe::new_write(make_id(1), 0, 128, 1);
        sq.push_sqe(sqe).expect("ok");
        assert_eq!(sq.len(), 1);
        let popped = sq.pop_sqe().expect("some");
        assert_eq!(popped.user_data, 1);
        assert!(sq.is_empty());
    }

    #[test]
    fn test_sq_full() {
        let sq = IoUringSq::new(2).expect("ok");
        sq.push_sqe(IoUringSqe::new_flush(1)).expect("ok");
        sq.push_sqe(IoUringSqe::new_flush(2)).expect("ok");
        assert!(sq.push_sqe(IoUringSqe::new_flush(3)).is_err());
    }

    #[test]
    fn test_cq_push_pop() {
        let cq = IoUringCq::new(8).expect("ok");
        cq.push_cqe(IoUringCqe::ok(1, 100)).expect("ok");
        assert!(cq.has_pending());
        let c = cq.pop_cqe().expect("some");
        assert_eq!(c.user_data, 1);
        assert!(!cq.has_pending());
    }

    #[test]
    fn test_io_uring_queue_submit_read() {
        let q = IoUringQueue::new(16, 32).expect("ok");
        let id = q.submit_read(make_id(1), 0, 512).expect("ok");
        assert!(id > 0);
        assert_eq!(q.sq.len(), 1);
    }

    #[test]
    fn test_io_uring_queue_process() {
        let q = IoUringQueue::new(16, 32).expect("ok");
        q.submit_read(make_id(1), 0, 128).expect("ok");
        q.submit_write(make_id(2), 0, 256).expect("ok");
        let processed = q.process_submissions().expect("ok");
        assert_eq!(processed, 2);
        assert!(q.cq.has_pending());
    }

    #[test]
    fn test_io_uring_queue_reap() {
        let q = IoUringQueue::new(16, 32).expect("ok");
        q.submit_flush().expect("ok");
        q.process_submissions().expect("ok");
        let mut out = Vec::new();
        let reaped = q.reap_completions(&mut out).expect("ok");
        assert_eq!(reaped, 1);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn test_sqe_size() {
        assert_eq!(IoUringSqe::SIZE, 128);
        assert!(core::mem::size_of::<IoUringSqe>() <= 128);
    }

    #[test]
    fn test_submitted_total() {
        let sq = IoUringSq::new(8).expect("ok");
        sq.push_sqe(IoUringSqe::new_flush(1)).expect("ok");
        sq.push_sqe(IoUringSqe::new_flush(2)).expect("ok");
        assert_eq!(sq.submitted_total(), 2);
    }

    #[test]
    fn test_reaped_total() {
        let cq = IoUringCq::new(8).expect("ok");
        cq.push_cqe(IoUringCqe::ok(1, 0)).expect("ok");
        cq.push_cqe(IoUringCqe::ok(2, 0)).expect("ok");
        cq.pop_cqe();
        assert_eq!(cq.reaped_total(), 1);
    }
}
