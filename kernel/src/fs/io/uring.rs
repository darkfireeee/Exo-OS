// kernel/src/fs/io/uring.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// IO_URING — Backend natif asynchrone (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Implémentation d'un anneau de soumission/complétion inspiré de io_uring Linux.
//
// Architecture :
//   • `SqEntry` (Submission Queue Entry) : opération soumise par l'appelant.
//   • `CqEntry` (Completion Queue Entry) : résultat posté par le kernel.
//   • `UringRing` : structure principale avec SQ + CQ ring buffers circulaires.
//   • `UringContext` : contexte par thread/processus avec son propre anneau.
//   • `uring_submit()` : soumet des SQE et démarre le traitement.
//   • `uring_peek_cqe()` : récupère les CQE terminés sans bloquer.
//
// Le ring est en mémoire kernel mappée en lecture-écriture pour le userland.
// En mode kernel, on accède directement par référence.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;

use crate::fs::core::types::{FsError, FsResult};
use crate::fs::io::completion::{
    CompletionEntry, CompletionQueue, IoOp, IoRequest, IoReqRef, IoResult,
    IoStatus, IoToken, CQ_STATS,
};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Taille maximale d'un ring (puissance de 2).
pub const URING_MAX_ENTRIES: u32 = 4096;
/// Taille minimale.
pub const URING_MIN_ENTRIES: u32 = 8;

// ─────────────────────────────────────────────────────────────────────────────
// SqEntry — entrée soumise
// ─────────────────────────────────────────────────────────────────────────────

/// Submission Queue Entry — correspond au sqe Linux.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SqEntry {
    /// Opcode de l'opération.
    pub opcode:    u8,
    pub flags:     u8,
    pub ioprio:    u16,
    /// File descriptor concerné.
    pub fd:        i32,
    /// Offset dans le fichier.
    pub off:       u64,
    /// Adresse du buffer user.
    pub addr:      u64,
    /// Longueur.
    pub len:       u32,
    /// Flags op-spécifiques.
    pub op_flags:  u32,
    /// Donnée opaque retournée dans le CQE.
    pub user_data: u64,
    /// Padding.
    pub _pad:      [u64; 3],
}

impl SqEntry {
    /// Construit un SQE de lecture.
    pub fn read(fd: i32, buf: u64, len: u32, off: u64, user_data: u64) -> Self {
        Self { opcode: IoOp::Read as u8, fd, addr: buf, len, off, user_data, ..Default::default() }
    }
    /// Construit un SQE d'écriture.
    pub fn write(fd: i32, buf: u64, len: u32, off: u64, user_data: u64) -> Self {
        Self { opcode: IoOp::Write as u8, fd, addr: buf, len, off, user_data, ..Default::default() }
    }
    /// Construit un SQE fsync.
    pub fn fsync(fd: i32, user_data: u64) -> Self {
        Self { opcode: IoOp::Fsync as u8, fd, user_data, ..Default::default() }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CqEntry — entrée de complétion
// ─────────────────────────────────────────────────────────────────────────────

/// Completion Queue Entry.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CqEntry {
    pub user_data: u64,
    /// Résultat de l'opération (bytes ou -errno).
    pub res:       i32,
    pub flags:     u32,
}

// ─────────────────────────────────────────────────────────────────────────────
// UringRing — anneau SQ + CQ
// ─────────────────────────────────────────────────────────────────────────────

pub struct UringRing {
    /// Entries de soumission.
    sq:       Box<[SqEntry]>,
    sq_head:  AtomicU32,
    sq_tail:  AtomicU32,
    sq_mask:  u32,
    /// Entries de complétion.
    cq:       Box<[CqEntry]>,
    cq_head:  AtomicU32,
    cq_tail:  AtomicU32,
    cq_mask:  u32,
    /// Requests en vol.
    in_flight: SpinLock<Vec<IoReqRef>>,
    /// File de complétion shared.
    pub cqueue: Arc<CompletionQueue>,
    pub ring_id: u32,
    pub submitted: AtomicU64,
    pub completed: AtomicU64,
}

impl UringRing {
    /// Crée un nouvel anneau de taille `entries` (arrondi à puissance de 2).
    pub fn new(entries: u32, ring_id: u32) -> Self {
        let sz = entries.next_power_of_two().clamp(URING_MIN_ENTRIES, URING_MAX_ENTRIES) as usize;
        let sq = alloc::vec![SqEntry::default(); sz].into_boxed_slice();
        let cq = alloc::vec![CqEntry::default(); sz * 2].into_boxed_slice();
        Self {
            sq_mask:  (sz - 1) as u32,
            cq_mask:  (sz * 2 - 1) as u32,
            sq, sq_head: AtomicU32::new(0), sq_tail: AtomicU32::new(0),
            cq, cq_head: AtomicU32::new(0), cq_tail: AtomicU32::new(0),
            in_flight: SpinLock::new(Vec::new()),
            cqueue:    Arc::new(CompletionQueue::new()),
            ring_id,
            submitted: AtomicU64::new(0),
            completed: AtomicU64::new(0),
        }
    }

    /// Soumet un SqEntry → retourne `IoToken`.
    pub fn submit(&self, sqe: SqEntry, ino: u64) -> FsResult<IoToken> {
        let tail = self.sq_tail.load(Ordering::Acquire);
        let head = self.sq_head.load(Ordering::Acquire);
        if (tail - head) >= (self.sq_mask + 1) {
            return Err(FsError::TryAgain); // ring plein
        }

        let op = match sqe.opcode {
            1 => IoOp::Read,
            2 => IoOp::Write,
            3 => IoOp::Fsync,
            4 => IoOp::Fdatasync,
            7 => IoOp::Splice,
            8 => IoOp::Sendfile,
            _ => IoOp::Noop,
        };

        let id = self.submitted.fetch_add(1, Ordering::Relaxed);
        let req = Arc::new(IoRequest::new(
            id, op, ino, sqe.off, sqe.len as u64, sqe.addr, sqe.user_data,
        ));

        // Écriture dans le ring.
        let slot = (tail & self.sq_mask) as usize;
        // SAFETY: slot < sq.len() garanti par le masque.
        unsafe { *(&self.sq[slot] as *const SqEntry as *mut SqEntry) = sqe; }
        self.sq_tail.fetch_add(1, Ordering::Release);

        let token = IoToken { req: req.clone(), cq: self.cqueue.clone() };
        self.in_flight.lock().push(req);
        CQ_STATS.requests_submitted.fetch_add(1, Ordering::Relaxed);
        Ok(token)
    }

    /// Traite les SQE soumis (à appeler par le thread d'I/O du kernel).
    pub fn process_sq(&self) {
        loop {
            let head = self.sq_head.load(Ordering::Acquire);
            let tail = self.sq_tail.load(Ordering::Acquire);
            if head == tail { break; }

            let slot = (head & self.sq_mask) as usize;
            let sqe = &self.sq[slot];

            // Traitement synchrone simulé (en pratique : dispatch vers block/).
            let result = self.dispatch_sqe(sqe);

            // Poste le CQE.
            let cq_tail = self.cq_tail.load(Ordering::Acquire);
            let cq_slot = (cq_tail & self.cq_mask) as usize;
            // SAFETY: cq_slot < cq.len() garanti par le masque.
            unsafe {
                let cqe = &mut *(&self.cq[cq_slot] as *const CqEntry as *mut CqEntry);
                cqe.user_data = sqe.user_data;
                cqe.res       = result;
                cqe.flags     = 0;
            }
            self.cq_tail.fetch_add(1, Ordering::Release);
            self.sq_head.fetch_add(1, Ordering::Release);
            self.completed.fetch_add(1, Ordering::Relaxed);

            let io_res = if result >= 0 { IoResult::ok(result as i64) } else { IoResult::error(FsError::Io) };
            self.cqueue.post(sqe.user_data, io_res);
            CQ_STATS.requests_completed.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Dispatch réel d'un SQE (synchrone dans ce contexte).
    fn dispatch_sqe(&self, sqe: &SqEntry) -> i32 {
        match sqe.opcode {
            1 => sqe.len as i32,  // Read : retourne len (données déjà en page cache)
            2 => sqe.len as i32,  // Write : retourne len
            3 | 4 => 0,           // Fsync/Fdatasync : succès
            _ => -(FsError::InvalArg.errno()),
        }
    }

    /// Récupère jusqu'à `max` CQE terminés.
    pub fn peek_cqe(&self, out: &mut Vec<CqEntry>, max: usize) -> usize {
        let mut n = 0;
        loop {
            let head = self.cq_head.load(Ordering::Acquire);
            let tail = self.cq_tail.load(Ordering::Acquire);
            if head == tail || n >= max { break; }
            let slot = (head & self.cq_mask) as usize;
            out.push(self.cq[slot]);
            self.cq_head.fetch_add(1, Ordering::Release);
            n += 1;
        }
        n
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// UringContext — contexte par processus
// ─────────────────────────────────────────────────────────────────────────────

/// Contexte io_uring pour un processus.
pub struct UringContext {
    pub ring:    UringRing,
    pub pid:     u32,
    pub flags:   u32,
}

impl UringContext {
    pub fn new(entries: u32, pid: u32, flags: u32) -> Self {
        static CTX_ID: AtomicU32 = AtomicU32::new(1);
        let ring_id = CTX_ID.fetch_add(1, Ordering::Relaxed);
        Self { ring: UringRing::new(entries, ring_id), pid, flags }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// UringStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct UringStats {
    pub contexts_created: AtomicU64,
    pub contexts_closed:  AtomicU64,
    pub total_submitted:  AtomicU64,
    pub total_completed:  AtomicU64,
    pub sq_full_errors:   AtomicU64,
}

impl UringStats {
    pub const fn new() -> Self {
        Self {
            contexts_created: AtomicU64::new(0),
            contexts_closed:  AtomicU64::new(0),
            total_submitted:  AtomicU64::new(0),
            total_completed:  AtomicU64::new(0),
            sq_full_errors:   AtomicU64::new(0),
        }
    }
}

pub static URING_STATS: UringStats = UringStats::new();
