// kernel/src/fs/block/bio.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// BIO — Block I/O Request (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Structure fondamentale de toute requête d'I/O bloc.
//
// Architecture :
//   • `BioOp` : type de l'opération (Read / Write / Discard / Flush).
//   • `BioFlags` : modificateurs (DIRECT, FUA, SYNC, etc.).
//   • `BioVec` : fragment scatter/gather (buf + len).
//   • `Bio` : requête d'I/O complète avec vecteur de BioVec.
//   • `BioChain` : chaîne de Bio liés (pour multi-page).
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use alloc::vec::Vec;

use crate::fs::core::types::FsError;
use crate::memory::core::types::PhysAddr;

// ─────────────────────────────────────────────────────────────────────────────
// BioOp
// ─────────────────────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BioOp {
    Read    = 0,
    Write   = 1,
    Discard = 2,
    Flush   = 3,
    WriteZeroes = 4,
}

// ─────────────────────────────────────────────────────────────────────────────
// BioFlags
// ─────────────────────────────────────────────────────────────────────────────

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct BioFlags: u32 {
        /// O_DIRECT — bypass page cache.
        const DIRECT   = 0x01;
        /// Force Unit Access — écriture garantie sur media.
        const FUA      = 0x02;
        /// Synchrone — attend la complétion.
        const SYNC     = 0x04;
        /// Metadata I/O.
        const META     = 0x08;
        /// Priorité haute.
        const PRIO_HI  = 0x10;
        /// Read-ahead.
        const RAHEAD   = 0x20;
        /// Barrière d’écriture — ordonne les écritures précédentes.
        const BARRIER  = 0x40;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BioStatus
// ─────────────────────────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BioStatus {
    Pending   = 0,
    Submitted = 1,
    Success   = 2,
    Error     = 3,
}

// ─────────────────────────────────────────────────────────────────────────────
// BioVec — fragment scatter/gather
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct BioVec {
    /// Adresse physique du buffer.
    pub phys:   PhysAddr,
    /// Adresse virtuelle (kernel mapping).
    pub virt:   u64,
    /// Longueur en octets.
    pub len:    u32,
    /// Offset dans la page.
    pub offset: u32,
}

impl BioVec {
    pub fn new(phys: PhysAddr, virt: u64, len: u32, offset: u32) -> Self {
        Self { phys, virt, len, offset }
    }
    pub fn from_virt(virt: u64, len: u32) -> Self {
        Self { phys: PhysAddr::new(0), virt, len, offset: 0 }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Bio
// ─────────────────────────────────────────────────────────────────────────────

/// Requête d'I/O bloc.
pub struct Bio {
    /// ID unique.
    pub id:       u64,
    /// Opération.
    pub op:       BioOp,
    /// Device (DevId ou majeur/mineur packed).
    pub dev:      u64,
    /// Secteur de début (512-byte sectors).
    pub sector:   u64,
    /// Vecteur de fragments.
    pub vecs:     Vec<BioVec>,
    /// Flags.
    pub flags:    BioFlags,
    /// État.
    pub status:   AtomicU8,
    /// Bytes transférés.
    pub bytes:    AtomicU64,
    /// Callback opaque (pointeur de fonction kernel).
    pub callback: Option<fn(u64, BioStatus)>,
    /// Donnée privée du callback.
    pub cb_data:  u64,
}

static BIO_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

impl Bio {
    /// Crée un Bio simple (un seul fragment).
    pub fn new(op: BioOp, dev: u64, sector: u64, buf: u64, len: u32, flags: BioFlags) -> Self {
        let id = BIO_ID.fetch_add(1, Ordering::Relaxed);
        let mut vecs = Vec::new();
        vecs.push(BioVec::from_virt(buf, len));
        Self {
            id, op, dev, sector, vecs, flags,
            status:   AtomicU8::new(BioStatus::Pending as u8),
            bytes:    AtomicU64::new(0),
            callback: None,
            cb_data:  0,
        }
    }

    /// Ajoute un fragment.
    pub fn add_vec(&mut self, vec: BioVec) {
        self.vecs.push(vec);
    }

    /// Longueur totale en bytes.
    pub fn total_len(&self) -> u64 {
        self.vecs.iter().map(|v| v.len as u64).sum()
    }

    pub fn status(&self) -> BioStatus {
        match self.status.load(Ordering::Acquire) {
            0 => BioStatus::Pending,
            1 => BioStatus::Submitted,
            2 => BioStatus::Success,
            3 => BioStatus::Error,
            _ => BioStatus::Error,
        }
    }

    pub fn complete(&self, ok: bool) {
        let st = if ok { BioStatus::Success } else { BioStatus::Error };
        self.status.store(st as u8, Ordering::Release);
        if ok { self.bytes.store(self.total_len(), Ordering::Release); }
        if let Some(cb) = self.callback {
            cb(self.cb_data, st);
        }
    }

    pub fn is_write(&self) -> bool {
        matches!(self.op, BioOp::Write | BioOp::WriteZeroes)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BioStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct BioStats {
    pub submitted: core::sync::atomic::AtomicU64,
    pub completed: core::sync::atomic::AtomicU64,
    pub errors:    core::sync::atomic::AtomicU64,
    pub bytes_read:    core::sync::atomic::AtomicU64,
    pub bytes_written: core::sync::atomic::AtomicU64,
}

impl BioStats {
    pub const fn new() -> Self {
        Self {
            submitted:     AtomicU64::new(0),
            completed:     AtomicU64::new(0),
            errors:        AtomicU64::new(0),
            bytes_read:    AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
        }
    }
}

pub static BIO_STATS: BioStats = BioStats::new();
