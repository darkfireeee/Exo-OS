// kernel/src/process/namespace/pid_ns.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Espace de noms PID (CLONE_NEWPID) — Exo-OS Couche 1.5
// ═══════════════════════════════════════════════════════════════════════════════
#![allow(dead_code)]

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;

const MAX_PID_NS: usize = 64;
/// PIDs max par namespace : 1..32767.
const NS_PID_MAX: u32 = 32767;
/// Taille du bitmap local en mots u64.
const NS_PID_WORDS: usize = 512; // 512 × 64 = 32768

/// Bitmap PID local au namespace (mots AtomicU64, bit=1 → libre).
pub struct NsPidBitmap {
    words: [AtomicU64; NS_PID_WORDS],
}

impl NsPidBitmap {
    #[allow(clippy::declare_interior_mutable_const)]
    const FREE_WORD: AtomicU64 = AtomicU64::new(u64::MAX);

    pub const fn new() -> Self {
        Self { words: [Self::FREE_WORD; NS_PID_WORDS] }
    }

    /// Alloue le premier PID libre ≥ `min`.
    pub fn alloc(&self, min: u32) -> Option<u32> {
        let start = (min / 64) as usize;
        for w in start..NS_PID_WORDS {
            let val = self.words[w].load(Ordering::Relaxed);
            if val == 0 { continue; }
            let bit = val.trailing_zeros();
            let mask = 1u64 << bit;
            if self.words[w]
                .compare_exchange(val, val & !mask, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                let id = w as u32 * 64 + bit;
                if id >= min && id <= NS_PID_MAX { return Some(id); }
                self.words[w].fetch_or(mask, Ordering::Relaxed);
            }
        }
        None
    }

    /// Libère un PID.
    pub fn free(&self, pid: u32) {
        let w = (pid / 64) as usize;
        let bit = pid % 64;
        self.words[w].fetch_or(1u64 << bit, Ordering::Release);
    }
}

// SAFETY : NsPidBitmap ne contient que des AtomicU64.
unsafe impl Sync for NsPidBitmap {}

/// Espace de noms PID avec son propre allocateur.
#[repr(C)]
pub struct PidNamespace {
    /// Index unique du namespace.
    pub id:         u32,
    /// Profondeur (0 = racine).
    pub level:      u32,
    /// PID parent du namespace (PID 1 dans ce namespace).
    pub init_pid:   AtomicU32,
    /// Nombre de processus vivants.
    pub pop:        AtomicU32,
    /// Compteur de références.
    pub refcount:   AtomicU32,
    /// Validité.
    pub valid:      AtomicU32,
    /// Allocateur de PIDs local.
    pub bitmap:     NsPidBitmap,
    pub alloc_lock: SpinLock<()>,
}

impl PidNamespace {
    const fn new_root() -> Self {
        Self {
            id:         0,
            level:      0,
            init_pid:   AtomicU32::new(1),
            pop:        AtomicU32::new(0),
            refcount:   AtomicU32::new(1),
            valid:      AtomicU32::new(1),
            bitmap:     NsPidBitmap::new(),
            alloc_lock: SpinLock::new(()),
        }
    }

    /// Alloue un PID local dans ce namespace.
    pub fn alloc_local_pid(&self) -> Option<u32> {
        let _guard = self.alloc_lock.lock();
        self.bitmap.alloc(2).map(|n| {
            self.pop.fetch_add(1, Ordering::Relaxed);
            n
        })
    }

    /// Libère un PID local.
    pub fn free_local_pid(&self, pid: u32) {
        let _guard = self.alloc_lock.lock();
        self.bitmap.free(pid);
        self.pop.fetch_sub(1, Ordering::Relaxed);
    }
}

// SAFETY : seuls des atomiques + SpinLock.
unsafe impl Sync for PidNamespace {}

/// Namespace PID racine (global).
pub static ROOT_PID_NS: PidNamespace = PidNamespace::new_root();
