// kernel/src/process/namespace/pid_ns.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// PID namespace (CLONE_NEWPID) — Exo-OS Layer 1.5
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::sync::spinlock::SpinLock;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[allow(dead_code)]
const MAX_PID_NS: usize = 64;
/// Max PIDs per namespace: 1..32767.
const NS_PID_MAX: u32 = 32767;
/// Local bitmap size in u64 words.
const NS_PID_WORDS: usize = 512; // 512 × 64 = 32768

/// Local PID bitmap for the namespace (AtomicU64 words, bit=1 → free).
pub struct NsPidBitmap {
    words: [AtomicU64; NS_PID_WORDS],
}

impl Default for NsPidBitmap {
    fn default() -> Self {
        Self::new()
    }
}

impl NsPidBitmap {
    pub const fn new() -> Self {
        Self {
            words: [const { AtomicU64::new(u64::MAX) }; NS_PID_WORDS],
        }
    }

    /// Allocates first free PID ≥ `min`.
    pub fn alloc(&self, min: u32) -> Option<u32> {
        let start = (min / 64) as usize;
        for w in start..NS_PID_WORDS {
            let val = self.words[w].load(Ordering::Relaxed);
            if val == 0 {
                continue;
            }
            let bit = val.trailing_zeros();
            let mask = 1u64 << bit;
            if self.words[w]
                .compare_exchange(val, val & !mask, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                let id = w as u32 * 64 + bit;
                if id >= min && id <= NS_PID_MAX {
                    return Some(id);
                }
                self.words[w].fetch_or(mask, Ordering::Relaxed);
            }
        }
        None
    }

    /// Frees a PID.
    pub fn free(&self, pid: u32) {
        let w = (pid / 64) as usize;
        let bit = pid % 64;
        self.words[w].fetch_or(1u64 << bit, Ordering::Release);
    }
}

// SAFETY: NsPidBitmap contains only AtomicU64.
unsafe impl Sync for NsPidBitmap {}

/// PID namespace with its own allocator.
#[repr(C)]
pub struct PidNamespace {
    /// Unique namespace index.
    pub id: u32,
    /// Depth (0 = root).
    pub level: u32,
    /// Parent PID of namespace (PID 1 in this namespace).
    pub init_pid: AtomicU32,
    /// Number of living processes.
    pub pop: AtomicU32,
    /// Reference counter.
    pub refcount: AtomicU32,
    /// Validity.
    pub valid: AtomicU32,
    /// Local PID allocator.
    pub bitmap: NsPidBitmap,
    pub alloc_lock: SpinLock<()>,
}

impl PidNamespace {
    const fn new_root() -> Self {
        Self {
            id: 0,
            level: 0,
            init_pid: AtomicU32::new(1),
            pop: AtomicU32::new(0),
            refcount: AtomicU32::new(1),
            valid: AtomicU32::new(1),
            bitmap: NsPidBitmap::new(),
            alloc_lock: SpinLock::new(()),
        }
    }

    /// Allocates a local PID in this namespace.
    pub fn alloc_local_pid(&self) -> Option<u32> {
        let _guard = self.alloc_lock.lock();
        self.bitmap.alloc(2).inspect(|_| {
            self.pop.fetch_add(1, Ordering::Relaxed);
        })
    }

    /// Frees a local PID.
    pub fn free_local_pid(&self, pid: u32) {
        let _guard = self.alloc_lock.lock();
        self.bitmap.free(pid);
        self.pop.fetch_sub(1, Ordering::Relaxed);
    }
}

// SAFETY: atomics + SpinLock only.
unsafe impl Sync for PidNamespace {}

/// Root PID namespace (global).
pub static ROOT_PID_NS: PidNamespace = PidNamespace::new_root();
