//! Interface io_uring-like pour ExoFS Ring 0.
//!
//! Files de soumission (SQ) et complétion (CQ) circulaires lock-free.
//! Utilisées par les syscalls ExoFS pour les IO non-bloquants.
//!
//! RÈGLE 3  : tout unsafe → // SAFETY: <raison>
//! RÈGLE 14 : checked_add pour indices.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::fs::exofs::core::{BlobId, FsError};

/// Taille par défaut des files (doit être une puissance de 2).
pub const QUEUE_DEPTH: usize = 256;

/// Une soumission IO dans la SQ.
#[derive(Clone, Debug)]
pub struct IoUringSubmission {
    pub op: u8,         // 0=read, 1=write, 2=flush
    pub flags: u8,
    pub blob_id: BlobId,
    pub offset: u64,
    pub len: u32,
    /// Buffer kernel (pointeur physique alloué par le driver).
    pub buf_phys: u64,
    pub user_data: u64, // Cookie opaque retourné dans la CQ.
}

/// Un événement de complétion dans la CQ.
#[derive(Clone, Debug, Default)]
pub struct IoUringCompletion {
    pub user_data: u64,
    pub result: i64,  // Bytes transférés (> 0) ou code erreur (< 0).
    pub flags: u32,
}

/// File circulaire lock-free à prodcteur unique / consommateur unique.
///
/// Invariant : `head` ≤ `tail`, les indices sont non-wrappés (masque appliqué à l'accès).
pub struct IoUringQueue {
    entries: Vec<IoUringCompletion>,
    head: AtomicU32,
    tail: AtomicU32,
    mask: u32,
    overflow: AtomicU64,
}

impl IoUringQueue {
    /// Crée une queue de `depth` entrées (arrondi à la puissance de 2 supérieure).
    pub fn new(depth: usize) -> Result<Self, FsError> {
        let depth = depth.next_power_of_two().max(2);
        let mut entries = Vec::new();
        entries
            .try_reserve(depth)
            .map_err(|_| FsError::OutOfMemory)?;
        for _ in 0..depth {
            entries.push(IoUringCompletion::default());
        }
        Ok(Self {
            entries,
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            mask: (depth as u32).wrapping_sub(1),
            overflow: AtomicU64::new(0),
        })
    }

    /// Produit une complétion (appelé par le driver IO).
    /// Retourne `Err` si la queue est pleine (overflow compté).
    pub fn produce(&self, cqe: IoUringCompletion) -> Result<(), FsError> {
        let tail = self.tail.load(Ordering::Acquire);
        let head = self.head.load(Ordering::Acquire);
        let used = tail.wrapping_sub(head);
        if used as usize >= self.entries.len() {
            self.overflow.fetch_add(1, Ordering::Relaxed);
            return Err(FsError::QueueFull);
        }
        let slot = (tail & self.mask) as usize;
        // SAFETY: `slot` est dans les bornes de `entries` car mask = capacity-1.
        unsafe {
            let ptr = self.entries.as_ptr().add(slot) as *mut IoUringCompletion;
            ptr.write(cqe);
        }
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Consomme une complétion (appelé par le syscall handler).
    pub fn consume(&self) -> Option<IoUringCompletion> {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        if head == tail {
            return None; // Vide.
        }
        let slot = (head & self.mask) as usize;
        // SAFETY: slot est dans les bornes, protected par head < tail.
        let cqe = unsafe { self.entries.as_ptr().add(slot).read() };
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Some(cqe)
    }

    /// Nombre d'entrées disponibles.
    pub fn available(&self) -> u32 {
        let t = self.tail.load(Ordering::Acquire);
        let h = self.head.load(Ordering::Acquire);
        t.wrapping_sub(h)
    }

    /// Nombre d'overflows depuis la création.
    pub fn overflow_count(&self) -> u64 {
        self.overflow.load(Ordering::Relaxed)
    }
}
