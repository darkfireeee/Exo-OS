// kernel/src/fs/io/completion.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// I/O COMPLETION — Queues de complétion asynchrone (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Fournit les types fondamentaux des opérations I/O asynchrones :
//   • `IoOp` : opération demandée (READ / WRITE / FSYNC / FALLOCATE / NOOP)
//   • `IoStatus` : résultat d'une opération (Pending / Ready / Error)
//   • `IoRequest` : requête complète avec callback de complétion
//   • `CompletionQueue` : file MPSC des résultats à renvoyer à l'appelant
//   • `IoToken` : handle léger retourné à l'initiateur pour attendre/annuler
//
// Toutes les structures sont lock-free ou protégées par SpinLock léger.
// Les callbacks sont des pointeurs de fonction nuls en mode kernel (pas de closure
// pour éviter l'allocation inutile — on utilise un u64 cookie).
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::core::types::{FsError, FsResult};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// IoOp — type de l'opération
// ─────────────────────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoOp {
    Noop     = 0,
    Read     = 1,
    Write    = 2,
    Fsync    = 3,
    Fdatasync= 4,
    Fallocate= 5,
    Ftruncate= 6,
    /// Opération splice (zero-copy entre deux fds).
    Splice   = 7,
    /// Opération sendfile.
    Sendfile = 8,
}

// ─────────────────────────────────────────────────────────────────────────────
// IoStatus — état de la requête
// ─────────────────────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoStatus {
    /// En attente (ne pas lire res).
    Pending   = 0,
    /// Soumis au block device ou au scheduler.
    Submitted = 1,
    /// Complété avec succès (`res` = bytes transférés).
    Success   = 2,
    /// Complété avec erreur (`err` contient le code).
    Error     = 3,
    /// Annulé.
    Cancelled = 4,
}

// ─────────────────────────────────────────────────────────────────────────────
// IoResult — résultat d'une requête
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct IoResult {
    pub status: IoStatus,
    /// Bytes transférés si succès.
    pub res:    i64,
    /// Code d'erreur FsError::errno() si échec.
    pub err:    i32,
}

impl IoResult {
    pub const PENDING: Self = Self { status: IoStatus::Pending, res: 0, err: 0 };

    pub fn ok(bytes: i64) -> Self {
        Self { status: IoStatus::Success, res: bytes, err: 0 }
    }
    pub fn error(e: FsError) -> Self {
        Self { status: IoStatus::Error, res: -1, err: e.errno() }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IoRequest — requête I/O complète
// ─────────────────────────────────────────────────────────────────────────────

/// Requête I/O asynchrone — envoyée vers un backend (uring, aio, direct).
pub struct IoRequest {
    /// Identifiant unique de la requête.
    pub id:       u64,
    /// Opération demandée.
    pub op:       IoOp,
    /// Inode concerné.
    pub ino:      u64,
    /// Offset dans le fichier.
    pub offset:   u64,
    /// Longueur en octets.
    pub len:      u64,
    /// Buffer userspace (adresse virtuelle kernel — NULL pour opérations sans données).
    pub buf:      u64,
    /// Cookie opaque de l'appelant (utilisé pour match à la complétion).
    pub user_data: u64,
    /// État courant.
    pub status:   AtomicU8,
    /// Résultat.
    pub result:   AtomicU64, // encode IoResult dans un u64
}

impl IoRequest {
    pub fn new(id: u64, op: IoOp, ino: u64, offset: u64, len: u64, buf: u64, user_data: u64) -> Self {
        Self {
            id, op, ino, offset, len, buf, user_data,
            status: AtomicU8::new(IoStatus::Pending as u8),
            result: AtomicU64::new(0),
        }
    }

    pub fn complete(&self, res: IoResult) {
        let packed = ((res.status as u64) << 32) | ((res.err as u32) as u64) << 16 | (res.res as u16) as u64;
        self.result.store(packed, Ordering::Release);
        self.status.store(res.status as u8, Ordering::Release);
    }

    pub fn status(&self) -> IoStatus {
        match self.status.load(Ordering::Acquire) {
            0 => IoStatus::Pending,
            1 => IoStatus::Submitted,
            2 => IoStatus::Success,
            3 => IoStatus::Error,
            4 => IoStatus::Cancelled,
            _ => IoStatus::Error,
        }
    }

    pub fn cancel(&self) {
        let _ = self.status.compare_exchange(
            IoStatus::Pending as u8, IoStatus::Cancelled as u8,
            Ordering::AcqRel, Ordering::Relaxed
        );
    }
}

pub type IoReqRef = Arc<IoRequest>;

// ─────────────────────────────────────────────────────────────────────────────
// CompletionQueue — file MPSC des complétions
// ─────────────────────────────────────────────────────────────────────────────

/// File de complétion partagée entre le backend I/O et le waiter.
pub struct CompletionQueue {
    /// Entrées complétées.
    entries: SpinLock<Vec<CompletionEntry>>,
    /// Total complétions postées.
    pub posted:    AtomicU64,
    /// Total complétions consommées.
    pub consumed:  AtomicU64,
}

#[derive(Clone)]
pub struct CompletionEntry {
    pub user_data: u64,
    pub result:    IoResult,
}

impl CompletionQueue {
    pub const fn new() -> Self {
        Self {
            entries:  SpinLock::new(Vec::new()),
            posted:   AtomicU64::new(0),
            consumed: AtomicU64::new(0),
        }
    }

    /// Poste une complétion.
    pub fn post(&self, user_data: u64, result: IoResult) {
        let mut entries = self.entries.lock();
        entries.push(CompletionEntry { user_data, result });
        self.posted.fetch_add(1, Ordering::Relaxed);
    }

    /// Consomme jusqu'à `max` complétions dans `out`.
    pub fn drain(&self, out: &mut Vec<CompletionEntry>, max: usize) -> usize {
        let mut entries = self.entries.lock();
        let n = max.min(entries.len());
        for e in entries.drain(..n) { out.push(e); }
        self.consumed.fetch_add(n as u64, Ordering::Relaxed);
        n
    }

    pub fn pending(&self) -> usize {
        self.entries.lock().len()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IoToken — handle léger pour l'initiateur
// ─────────────────────────────────────────────────────────────────────────────

/// Handle retourné au code appelant lors d'une soumission I/O async.
#[derive(Clone)]
pub struct IoToken {
    pub req: IoReqRef,
    pub cq:  Arc<CompletionQueue>,
}

impl IoToken {
    /// Vérifie si l'opération est terminée.
    pub fn is_done(&self) -> bool {
        matches!(self.req.status(), IoStatus::Success | IoStatus::Error | IoStatus::Cancelled)
    }

    /// Annule si encore en attente.
    pub fn cancel(&self) {
        self.req.cancel();
    }

    /// Attend de façon spin (à utiliser avec précaution dans le kernel).
    pub fn spin_wait(&self) {
        while !self.is_done() {
            core::hint::spin_loop();
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CqStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct CqStats {
    pub requests_submitted: AtomicU64,
    pub requests_completed: AtomicU64,
    pub requests_cancelled: AtomicU64,
    pub requests_errored:   AtomicU64,
}

impl CqStats {
    pub const fn new() -> Self {
        Self {
            requests_submitted: AtomicU64::new(0),
            requests_completed: AtomicU64::new(0),
            requests_cancelled: AtomicU64::new(0),
            requests_errored:   AtomicU64::new(0),
        }
    }
}

pub static CQ_STATS: CqStats = CqStats::new();
