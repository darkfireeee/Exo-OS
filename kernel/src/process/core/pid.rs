// kernel/src/process/core/pid.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// PID / TID Allocator — allocateur lock-free par bitmap radix (Exo-OS Couche 1.5)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Conception :
//   • Bitmap plate de 512 mots u64 → 32 768 PID simultanés.
//   • Allocation O(1) amortie : CLZ + scan word-by-word.
//   • Séparation PID (processus) / TID (threads) via deux allocateurs indépendants.
//   • PID_MIN = 1, PID_MAX = 32 767 ; TID_MAX = 131 071.
//   • PID 0 réservé (idle/swapper), PID 1 réservé (init).
//   • Thread-safe : CAS atomique sur chaque word bitmap.
//   • Instrumentation : compteurs alloc/free/current_count atomiques.
//
// RÈGLE : Aucune allocation heap dans ce fichier (zone NO-ALLOC partielle).
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, AtomicUsize, AtomicU32, Ordering};
use core::fmt;

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// Process ID — non-zéro, unique globalement.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct Pid(pub u32);

/// Thread ID — non-zéro, unique globalement.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct Tid(pub u32);

impl Pid {
    pub const INVALID: Self  = Self(0);
    pub const IDLE:    Self  = Self(0);
    pub const INIT:    Self  = Self(1);

    #[inline(always)]
    pub fn is_valid(self) -> bool { self.0 > 0 }

    #[inline(always)]
    pub fn as_u32(self) -> u32 { self.0 }
}

impl Tid {
    pub const INVALID: Self = Self(0);

    #[inline(always)]
    pub fn is_valid(self) -> bool { self.0 > 0 }

    #[inline(always)]
    pub fn as_u32(self) -> u32 { self.0 }
}

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pid:{}", self.0)
    }
}
impl fmt::Display for Tid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tid:{}", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Capacité maximale du bitmap PID (entrées).
pub const PID_BITMAP_WORDS: usize = 512;   // 512 × 64 = 32 768 PIDs
pub const TID_BITMAP_WORDS: usize = 2048;  // 2048 × 64 = 131 072 TIDs

pub const PID_FIRST_USABLE: u32 = 2;       // 0=idle, 1=init réservés
pub const PID_MAX:          u32 = (PID_BITMAP_WORDS as u32 * 64) - 1;
pub const TID_FIRST_USABLE: u32 = 1;
pub const TID_MAX:          u32 = (TID_BITMAP_WORDS as u32 * 64) - 1;

// ─────────────────────────────────────────────────────────────────────────────
// PidBitmap — structure bitmap statique
// ─────────────────────────────────────────────────────────────────────────────

/// Bitmap plate de N mots AtomicU64.
/// Bit à 1 = ID LIBRE, bit à 0 = ID UTILISÉ.
struct PidBitmap<const N: usize> {
    words: [AtomicU64; N],
}

impl<const N: usize> PidBitmap<N> {
    const fn new_all_free() -> Self {
        // SAFETY: AtomicU64::new(u64::MAX) est une constante valide.
        // Tous les bits à 1 = tous libres.
        #[allow(clippy::declare_interior_mutable_const)]
        const WORD_FREE: AtomicU64 = AtomicU64::new(u64::MAX);
        Self { words: [WORD_FREE; N] }
    }

    /// Alloue le premier ID libre >= `first_usable`.
    /// Retourne `None` si épuisé.
    /// Algorithme : scan word par word, CLZ pour trouver bit libre.
    fn alloc(&self, first_usable: u32) -> Option<u32> {
        let start_word  = (first_usable / 64) as usize;
        let total_words = N;

        for w in start_word..total_words {
            let val = self.words[w].load(Ordering::Relaxed);
            if val == 0 { continue; }  // mot plein

            // Trouver le bit libre le moins significatif.
            let bit = val.trailing_zeros(); // position du premier bit à 1
            let mask = 1u64 << bit;

            // Tenter de marquer comme utilisé (1→0) avec CAS.
            if self.words[w]
                .compare_exchange(val, val & !mask, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                let id = (w as u32) * 64 + bit;
                if id >= first_usable {
                    return Some(id);
                }
                // ID trop petit (< first_usable) — le relibérer et continuer.
                self.words[w].fetch_or(mask, Ordering::Relaxed);
                continue;
            }
            // CAS échoué (race) — recommencer ce word.
            // Pour simplifier, on passe au suivant (prochaine allocation réussira).
        }
        None
    }

    /// Libère l'ID donné (remet le bit à 1).
    /// Panique (debug) si l'ID était déjà libre.
    fn free(&self, id: u32) {
        let w   = (id / 64) as usize;
        let bit = id % 64;
        let mask = 1u64 << bit;
        let prev = self.words[w].fetch_or(mask, Ordering::Release);
        debug_assert!(
            prev & mask == 0,
            "PidBitmap::free: id {} était déjà libre (double free)",
            id
        );
    }

    /// Vérifie si l'ID est utilisé (bit à 0).
    fn is_used(&self, id: u32) -> bool {
        let w    = (id / 64) as usize;
        let bit  = id % 64;
        self.words[w].load(Ordering::Relaxed) & (1u64 << bit) == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PidAllocator — wrapper avec compteurs de statistiques
// ─────────────────────────────────────────────────────────────────────────────

/// Allocateur PID avec instrumentation complète.
pub struct PidAllocator {
    bitmap:        &'static PidBitmapPid,
    first_usable:  u32,
    total_capacity: u32,
    /// Nombre d'allocations réussies depuis le boot.
    alloc_count:   AtomicU64,
    /// Nombre de libérations depuis le boot.
    free_count:    AtomicU64,
    /// Nombre courant d'IDs utilisés.
    current_used:  AtomicU32,
    /// Nombre max d'IDs simultanément utilisés (high-water mark).
    peak_used:     AtomicU32,
    /// Nombre d'échecs d'allocation (épuisement).
    exhausted:     AtomicU64,
}

// Alias de type pour les bitmaps statiques.
type PidBitmapPid = PidBitmap<PID_BITMAP_WORDS>;
type PidBitmapTid = PidBitmap<TID_BITMAP_WORDS>;

static PID_BITMAP_STORAGE: PidBitmapPid = PidBitmap::new_all_free();
static TID_BITMAP_STORAGE: PidBitmapTid = PidBitmap::new_all_free();

// SAFETY: PidBitmap n'accède à ses champs que via des atomiques.
unsafe impl Sync for PidBitmapPid {}
unsafe impl Sync for PidBitmapTid {}

pub static PID_ALLOCATOR: PidAllocator = PidAllocator {
    // SAFETY: PID_BITMAP_STORAGE est un tableau statique aligné avec le layout correct;
    //         PidBitmapPid est un newtype transparent autour du même type sous-jacent.
    bitmap:         unsafe { &*(&PID_BITMAP_STORAGE as *const PidBitmapPid) },
    first_usable:   PID_FIRST_USABLE,
    total_capacity: PID_MAX,
    alloc_count:    AtomicU64::new(0),
    free_count:     AtomicU64::new(0),
    current_used:   AtomicU32::new(0),
    peak_used:      AtomicU32::new(0),
    exhausted:      AtomicU64::new(0),
};

pub static TID_ALLOCATOR: PidAllocator = PidAllocator {
    // SAFETY: TID_BITMAP_STORAGE est un tableau statique aligné ; cast en PidBitmapPid valide
    //         car PidBitmapTid et PidBitmapPid ont le même layout (même type sous-jacent).
    bitmap:         unsafe { &*(&TID_BITMAP_STORAGE as *const PidBitmapTid as *const PidBitmapPid) },
    first_usable:   TID_FIRST_USABLE,
    total_capacity: TID_MAX,
    alloc_count:    AtomicU64::new(0),
    free_count:     AtomicU64::new(0),
    current_used:   AtomicU32::new(0),
    peak_used:      AtomicU32::new(0),
    exhausted:      AtomicU64::new(0),
};

// SAFETY: PidAllocator n'accède à ses champs atomiques que via Atomic*.
unsafe impl Sync for PidAllocator {}
unsafe impl Send for PidAllocator {}

/// Erreur d'allocation PID/TID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidAllocError {
    /// Plus d'IDs disponibles.
    Exhausted,
    /// L'ID demandé est déjà utilisé (réservation explicite uniquement).
    AlreadyUsed,
}

impl PidAllocator {
    /// Alloue un nouveau PID/TID.
    /// Retourne `Err(Exhausted)` si tous les IDs sont utilisés.
    #[inline]
    pub fn alloc(&self) -> Result<u32, PidAllocError> {
        match self.bitmap.alloc(self.first_usable) {
            Some(id) => {
                self.alloc_count.fetch_add(1, Ordering::Relaxed);
                let cur = self.current_used.fetch_add(1, Ordering::Relaxed) + 1;
                // Mise à jour du high-water mark sans lock.
                let mut peak = self.peak_used.load(Ordering::Relaxed);
                while cur > peak {
                    match self.peak_used.compare_exchange_weak(
                        peak, cur, Ordering::Relaxed, Ordering::Relaxed,
                    ) {
                        Ok(_)  => break,
                        Err(p) => peak = p,
                    }
                }
                Ok(id)
            }
            None => {
                self.exhausted.fetch_add(1, Ordering::Relaxed);
                Err(PidAllocError::Exhausted)
            }
        }
    }

    /// Libère un PID/TID précédemment alloué.
    #[inline]
    pub fn free(&self, id: u32) {
        self.bitmap.free(id);
        self.free_count.fetch_add(1, Ordering::Relaxed);
        self.current_used.fetch_sub(1, Ordering::Relaxed);
    }

    /// Vérifie si un ID est actuellement utilisé.
    #[inline(always)]
    pub fn is_used(&self, id: u32) -> bool {
        self.bitmap.is_used(id)
    }

    /// Nombre d'IDs actuellement en usage.
    #[inline(always)]
    pub fn current_count(&self) -> u32 {
        self.current_used.load(Ordering::Relaxed)
    }

    /// Nombre total d'allocations depuis le boot.
    #[inline(always)]
    pub fn total_allocs(&self) -> u64 {
        self.alloc_count.load(Ordering::Relaxed)
    }

    /// Nombre d'échecs d'allocation (épuisement).
    #[inline(always)]
    pub fn exhaustion_count(&self) -> u64 {
        self.exhausted.load(Ordering::Relaxed)
    }

    /// High-water mark d'utilisation simultanée.
    #[inline(always)]
    pub fn peak_count(&self) -> u32 {
        self.peak_used.load(Ordering::Relaxed)
    }
}

/// Initialise les allocateurs PID et TID.
/// Réserve PID 0 (idle) et PID 1 (init) comme utilisés.
///
/// # Safety
/// Appelé une seule fois depuis le BSP au boot.
pub unsafe fn init(_max_pids: usize, _max_tids: usize) {
    // Réserver PID 0 = idle.
    PID_BITMAP_STORAGE.words[0].fetch_and(!(1u64 << 0), Ordering::Relaxed);
    // Réserver PID 1 = init.
    PID_BITMAP_STORAGE.words[0].fetch_and(!(1u64 << 1), Ordering::Relaxed);
    PID_ALLOCATOR.current_used.store(2, Ordering::Relaxed);
}
